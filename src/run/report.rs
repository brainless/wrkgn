//! End-of-run report: pasteable Markdown, one per run, whatever the ending.
//! It contains only normalized signatures and redacted samples: no payloads,
//! prompts or credentials.

use std::collections::BTreeMap;
use std::io::{self, Write};

use super::MAX_UNIT_ATTEMPTS;
use super::control::{Api, BREAKER_THRESHOLD};
use super::errors::ErrorGroup;

/// One failed queue item of a unit (`id` is `None` for a unit-level error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedItem {
    pub id: Option<i64>,
    pub error: String,
}

const ERROR_SHOWN_CHARS: usize = 120;
const ITEMS_SHOWN_PER_UNIT: usize = 5;

/// Single-line, truncated error text for a manual-action list.
pub fn short_error(error: &str) -> String {
    let flat = error.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > ERROR_SHOWN_CHARS {
        let cut: String = flat.chars().take(ERROR_SHOWN_CHARS).collect();
        format!("{cut}...")
    } else {
        flat
    }
}

/// One `{unit}: {label} {id}: {error}` line per failed item (at most five per
/// unit, then a count of the rest), each prefixed with `indent` and `- `.
pub fn failure_lines(unit: &str, label: &str, failures: &[FailedItem], indent: &str) -> String {
    let mut out = String::new();
    if failures.is_empty() {
        out.push_str(&format!("{indent}- {unit}\n"));
    }
    for failure in failures.iter().take(ITEMS_SHOWN_PER_UNIT) {
        let what = failure
            .id
            .map_or_else(String::new, |id| format!("{label} {id}: "));
        out.push_str(&format!(
            "{indent}- {unit}: {what}{}\n",
            inline(&short_error(&failure.error))
        ));
    }
    if failures.len() > ITEMS_SHOWN_PER_UNIT {
        out.push_str(&format!(
            "{indent}- {unit}: and {} more failed {label}(s) (`wrkgn runs list`)\n",
            failures.len() - ITEMS_SHOWN_PER_UNIT
        ));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Outcome {
    #[default]
    Completed,
    LimitReached,
    Interrupted,
    StoppedOnFailure,
}

impl Outcome {
    /// The value stored in `driver_runs.status`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::LimitReached => "limit_reached",
            Self::Interrupted => "interrupted",
            Self::StoppedOnFailure => "stopped_on_failure",
        }
    }

    /// 0 for completed or limit reached without failures, 1 with failures,
    /// 2 when the circuit breaker stopped the run, 130 when interrupted.
    pub fn exit_code(&self, failures: bool) -> u8 {
        match self {
            Self::Completed | Self::LimitReached => u8::from(failures),
            Self::StoppedOnFailure => 2,
            Self::Interrupted => 130,
        }
    }
}

/// Why a run ended as `limit_reached`: either condition, or both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LimitInfo {
    /// The per-run item cap was hit: (items used, cap, item label).
    pub cap: Option<(usize, usize, String)>,
    /// Older uncovered units remain beyond this run's backfill; the next one.
    pub backfill_next: Option<String>,
}

impl LimitInfo {
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some((used, max, label)) = &self.cap {
            parts.push(format!("{label} cap reached ({used}/{max})"));
        }
        if let Some(next) = &self.backfill_next {
            parts.push(format!("more backfill remains (next: {next})"));
        }
        if parts.is_empty() {
            "run limit reached".into()
        } else {
            parts.join("; ")
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RunSummary {
    pub run_id: i64,
    pub command: String,
    pub outcome: Outcome,
    /// Unix seconds.
    pub started_at: i64,
    pub finished_at: i64,
    pub errors: Vec<ErrorGroup>,
    pub units_done: Vec<String>,
    pub items_collected: usize,
    pub items_processed: usize,
    pub items_approved: usize,
    pub export: Option<String>,
    pub pending_units: Vec<String>,
    /// Failed units that will be retried, with attempts used.
    pub failed_units: Vec<(String, u32)>,
    /// Failed units whose attempts are used up.
    pub exhausted_units: Vec<String>,
    /// Failed items (or the unit error) per failed or exhausted unit.
    pub unit_failures: BTreeMap<String, Vec<FailedItem>>,
    /// What the command's queue items are called (`thread`, `story`).
    pub item_label: String,
    /// Failed units the maintainer acknowledged: skipped, not failures, and
    /// never hidden.
    pub acknowledged_units: Vec<String>,
    /// Units whose attempt in this run ended failed (a subset of the failed
    /// and exhausted lists); the rest of those lists carry over from earlier runs.
    pub failed_this_run: Vec<String>,
    /// Set for `limit_reached` runs.
    pub limit: LimitInfo,
    pub killed_runs: Vec<i64>,
    pub took_over_pid: Option<u32>,
    pub tripped_api: Option<Api>,
    pub next_run: String,
}

impl RunSummary {
    pub fn has_failures(&self) -> bool {
        !self.errors.is_empty() || !self.failed_units.is_empty() || !self.exhausted_units.is_empty()
    }

    pub fn exit_code(&self) -> u8 {
        self.outcome.exit_code(self.has_failures())
    }

    /// Number of errors recorded in this run (sum over groups).
    pub fn error_count(&self) -> usize {
        self.errors.iter().map(|group| group.count).sum()
    }

    /// Exhausted units from earlier runs that this run did not touch.
    pub fn carried_exhausted(&self) -> Vec<&String> {
        self.exhausted_units
            .iter()
            .filter(|unit| !self.failed_this_run.contains(unit))
            .collect()
    }

    /// Why the exit code is 1, so a nonzero code never looks unexplained.
    fn failure_explanation(&self) -> Option<String> {
        if self.exit_code() != 1 {
            return None;
        }
        let mut parts = Vec::new();
        let carried = self.carried_exhausted().len();
        let exhausted = self.exhausted_units.len();
        if exhausted > 0 {
            let noun = if exhausted == 1 {
                "unit needs"
            } else {
                "units need"
            };
            let origin = if carried == exhausted {
                "all from earlier runs".to_owned()
            } else if carried == 0 {
                "all exhausted this run".to_owned()
            } else {
                format!(
                    "{} exhausted this run, {carried} from earlier runs",
                    exhausted - carried
                )
            };
            parts.push(format!("{exhausted} {noun} manual action, {origin}"));
        }
        if !self.failed_units.is_empty() {
            let noun = if self.failed_units.len() == 1 {
                "unit"
            } else {
                "units"
            };
            parts.push(format!(
                "{} failed {noun} will be retried next run",
                self.failed_units.len()
            ));
        }
        let errors = self.error_count();
        parts.push(format!(
            "{errors} {} this run",
            if errors == 1 { "error" } else { "errors" }
        ));
        Some(parts.join("; "))
    }

    fn reason(&self) -> Option<String> {
        match self.outcome {
            Outcome::Completed => None,
            Outcome::LimitReached => Some(self.limit.describe()),
            Outcome::Interrupted => Some("interrupt received".into()),
            Outcome::StoppedOnFailure => Some(match self.tripped_api {
                Some(api) => format!("{BREAKER_THRESHOLD} consecutive {} failures", api.label()),
                None => "circuit breaker tripped".into(),
            }),
        }
    }
}

fn list(items: &[String]) -> String {
    if items.is_empty() {
        "None".into()
    } else {
        items.join(", ")
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` for Unix seconds.
fn iso(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

fn item(id: Option<i64>) -> String {
    id.map_or_else(|| "None".into(), |id| id.to_string())
}

/// A fenced block whose fence is longer than any backtick run in `text`.
fn fenced(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}\n{text}\n{fence}")
}

/// Signatures are single-line; keep them safe inside inline code.
fn inline(text: &str) -> String {
    text.replace('`', "'")
}

pub fn render_report(s: &RunSummary) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# {} run {}: {}\n\n",
        s.command,
        s.run_id,
        s.outcome.as_str()
    ));
    match s.reason() {
        Some(reason) => out.push_str(&format!("- Outcome: {} ({reason})\n", s.outcome.as_str())),
        None => out.push_str(&format!("- Outcome: {}\n", s.outcome.as_str())),
    }
    match s.failure_explanation() {
        Some(why) => out.push_str(&format!("- Exit code: {} ({why})\n", s.exit_code())),
        None => out.push_str(&format!("- Exit code: {}\n", s.exit_code())),
    }
    out.push_str(&format!(
        "- Started: {}  Finished: {}\n\n",
        iso(s.started_at),
        iso(s.finished_at)
    ));

    out.push_str(&format!("## Errors ({} groups)\n\n", s.errors.len()));
    if s.errors.is_empty() {
        out.push_str("None\n\n");
    }
    let mut errors: Vec<&ErrorGroup> = s.errors.iter().collect();
    errors.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.signature.cmp(&b.signature))
    });
    for (index, group) in errors.iter().enumerate() {
        out.push_str(&format!(
            "### {}. {} x `{}`\n- Stage: {}  Kind: {}\n- First item: {}  Last item: {}\n- Sample:\n\n{}\n\n",
            index + 1,
            group.count,
            inline(&group.signature),
            group.stage,
            group.kind,
            item(group.first_item),
            item(group.last_item),
            fenced(&group.sample)
        ));
    }

    out.push_str("## Work done\n");
    out.push_str(&format!(
        "- Units covered: {} ({})\n",
        s.units_done.len(),
        list(&s.units_done)
    ));
    out.push_str(&format!(
        "- Items collected: {}, processed: {}, approved: {}\n",
        s.items_collected, s.items_processed, s.items_approved
    ));
    out.push_str(&format!(
        "- Export: {}\n\n",
        s.export.as_deref().unwrap_or("None")
    ));

    out.push_str("## What remains\n");
    out.push_str(&format!("- Pending units: {}\n", list(&s.pending_units)));
    let failed: Vec<String> = s
        .failed_units
        .iter()
        .map(|(unit, attempts)| format!("{unit} ({attempts}/{MAX_UNIT_ATTEMPTS} attempts)"))
        .collect();
    out.push_str(&format!(
        "- Failed units (retry next run): {}\n",
        list(&failed)
    ));
    let label = if s.item_label.is_empty() {
        "item"
    } else {
        &s.item_label
    };
    for (unit, _) in &s.failed_units {
        if let Some(failures) = s.unit_failures.get(unit) {
            out.push_str(&failure_lines(unit, label, failures, "  "));
        }
    }
    if s.exhausted_units.is_empty() {
        out.push_str("- Needs manual action (attempts exhausted): None\n");
    } else {
        let label = if s.item_label.is_empty() {
            "item"
        } else {
            &s.item_label
        };
        out.push_str("- Needs manual action (attempts exhausted):\n");
        for unit in &s.exhausted_units {
            let failures = s.unit_failures.get(unit).map_or(&[][..], Vec::as_slice);
            out.push_str(&failure_lines(unit, label, failures, "  "));
        }
        out.push_str(&format!(
            "  Fix a {label} with `wrkgn collect --thread <id>` then `wrkgn process --thread <id>` (the next run detects a complete processing run after the failure), or use `wrkgn runs retry` or `wrkgn runs acknowledge`.\n"
        ));
    }
    if !s.acknowledged_units.is_empty() {
        let count = s.acknowledged_units.len();
        out.push_str(&format!(
            "- Acknowledged (skipped): {count} {}: {} (`wrkgn runs list` shows notes; `unacknowledge` or `retry` reopens)\n",
            if count == 1 { "unit" } else { "units" },
            list(&s.acknowledged_units)
        ));
    }
    out.push_str(&format!("- Next run will: {}\n", s.next_run));
    if !s.killed_runs.is_empty() {
        let ids: Vec<String> = s.killed_runs.iter().map(i64::to_string).collect();
        let pid = s
            .took_over_pid
            .map(|pid| format!(", PID {pid}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "- Note: previous run {} was found killed (stale lock{pid}).\n",
            ids.join(", ")
        ));
    }
    out
}

pub fn write_report(out: &mut impl Write, s: &RunSummary) -> io::Result<()> {
    out.write_all(render_report(s).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_formats_unix_time() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn fence_outgrows_backticks() {
        assert!(fenced("a ```` b").starts_with("`````\n"));
        assert!(fenced("plain").starts_with("```\n"));
    }
}
