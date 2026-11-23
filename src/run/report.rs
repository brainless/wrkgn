//! End-of-run report: pasteable Markdown, one per run, whatever the ending.
//! It contains only normalized signatures and redacted samples: no payloads,
//! prompts or credentials.

use std::io::{self, Write};

use super::MAX_UNIT_ATTEMPTS;
use super::control::{Api, BREAKER_THRESHOLD};
use super::errors::ErrorGroup;

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

    fn reason(&self) -> Option<String> {
        match self.outcome {
            Outcome::Completed => None,
            Outcome::LimitReached => Some("run limit reached".into()),
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
    out.push_str(&format!("- Exit code: {}\n", s.exit_code()));
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
    if s.exhausted_units.is_empty() {
        out.push_str("- Needs manual action (attempts exhausted): None\n");
    } else {
        out.push_str(&format!(
            "- Needs manual action (attempts exhausted): {} (try `wrkgn collect --thread <id>` then `wrkgn process --thread <id>`)\n",
            list(&s.exhausted_units)
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
