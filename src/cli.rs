//! Explicit local pipeline and review commands.
use crate::category_migration;
use crate::collector::{CollectionConfig, Collector, HnApi};
use crate::processing::{self, RigModel};
use crate::publication;
use crate::review_grid;
use crate::store::Store;
use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
const USAGE:&str="Usage:
  wrkgn collect --thread <id> [--db <path>]
  wrkgn process --thread <id> [--reprocess] [--concurrency <n>] [--db <path>] [--export <directory>]
  wrkgn review pending [--json] [--db <path>]
  wrkgn review approve <mention-id|first-last>... [--db <path>]
  wrkgn review correct <mention-key> <name|description|problem|audience|builder_username> <value> [--db <path>]
  wrkgn review suppress-source <source-id> <reason> [--db <path>]
  wrkgn review suppress-project <project-id> [--db <path>]
  wrkgn review match <from-project-id> <to-project-id> [--db <path>]
  wrkgn export [--db <path>] [--export <directory>]
  wrkgn categories normalize [--db <path>]
  wrkgn --help

WRKGN_DB_PATH defaults to local-data/wrkgn.sqlite. WRKGN_EXPORT_PATH defaults to local-data/public-export.
The CLI loads .env from the current directory; shell variables take precedence.
process reads stored source from the latest complete collect and makes no HN calls;
it calls the LLM provider, using WRKGN_PROVIDER=openai (default) with WRKGN_MODEL and OPENAI_API_KEY,
or WRKGN_PROVIDER=neuralwatt with NEURALWATT_API_KEY and default model glm-5.3-flash.
NEURALWATT_BASE_URL optionally overrides https://api.neuralwatt.com/v1.
--concurrency sets how many conversation groups are extracted in parallel (default 10).
collect is the only command that fetches from HN.
Review, category normalization, and export make no network calls.";
fn path_arg(args: &[String], flag: &str) -> Result<Option<PathBuf>, String> {
    let mut value = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            let next = args
                .get(i + 1)
                .ok_or_else(|| format!("{flag} requires a path"))?;
            if next.starts_with("--") {
                return Err(format!("{flag} requires a path"));
            }
            value = Some(PathBuf::from(next));
            i += 2
        } else {
            i += 1
        }
    }
    Ok(value)
}
fn positional(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if ["--db", "--export", "--thread", "--concurrency"].contains(&args[i].as_str()) {
            i += 2
        } else if args[i] == "--reprocess" {
            i += 1
        } else {
            out.push(args[i].as_str());
            i += 1
        }
    }
    out
}
fn open(args: &[String]) -> Result<Store, String> {
    match path_arg(args, "--db")? {
        Some(path) => Store::open(path),
        None => Store::open_configured(),
    }
    .map_err(|e| format!("database error: {e}"))
}
fn export_path(args: &[String]) -> Result<PathBuf, String> {
    Ok(path_arg(args, "--export")?
        .or_else(|| std::env::var_os("WRKGN_EXPORT_PATH").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("local-data/public-export")))
}
fn concurrency(args: &[String]) -> Result<usize, String> {
    let Some(pos) = args.iter().position(|s| s == "--concurrency") else {
        return Ok(processing::DEFAULT_CONCURRENCY);
    };
    args.get(pos + 1)
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| "--concurrency requires a positive integer".into())
}
fn thread_id(args: &[String]) -> Result<i64, String> {
    let p = positional(args);
    let pos = args
        .iter()
        .position(|s| s == "--thread")
        .ok_or("--thread is required")?;
    let id = args
        .get(pos + 1)
        .ok_or("--thread requires an ID")?
        .parse::<i64>()
        .map_err(|_| "invalid thread ID")?;
    if id <= 0 || p.len() != 1 {
        return Err("thread ID must be a positive integer".into());
    }
    Ok(id)
}
/// One progress line on stderr: rewritten in place on a terminal, otherwise
/// printed at each 10% step.
pub(crate) struct Status {
    tty: bool,
    shown: Option<usize>,
}
impl Status {
    pub(crate) fn new() -> Self {
        Self {
            tty: io::stderr().is_terminal(),
            shown: None,
        }
    }
    pub(crate) fn update(&mut self, p: processing::Progress) {
        if p.total == 0 {
            return;
        }
        let pct = p.done * 100 / p.total;
        let step = if self.tty { p.done } else { pct / 10 };
        if self.shown == Some(step) {
            return;
        }
        self.shown = Some(step);
        let line = format!(
            "Extracting {}/{} groups ({pct}%), {} failed",
            p.done, p.total, p.failed
        );
        let mut err = io::stderr();
        let _ = if self.tty {
            write!(err, "\r\x1b[2K{line}")
        } else {
            writeln!(err, "{line}")
        };
        let _ = err.flush();
    }
    pub(crate) fn finish(&self) {
        if self.tty && self.shown.is_some() {
            let _ = writeln!(io::stderr());
        }
    }
}
/// Counts failures by kind with one example group each, instead of every error.
pub(crate) fn failure_summary(report: &processing::Report) -> String {
    let mut kinds: BTreeMap<String, (usize, i64)> = BTreeMap::new();
    for (root, error) in &report.failed {
        kinds
            .entry(processing::failure_kind(error))
            .or_insert((0, *root))
            .0 += 1;
    }
    let mut kinds: Vec<_> = kinds.into_iter().collect();
    kinds.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));
    let mut message = format!(
        "processing incomplete (run {}): {} group(s) failed, {} not attempted",
        report.run_id,
        report.failed.len(),
        report.not_attempted
    );
    for (kind, (count, root)) in kinds {
        message.push_str(&format!("\n  {count} x {kind} (e.g. group {root})"));
    }
    if report.stopped_early {
        message.push_str("\nStopped early after repeated rate-limit or connection errors. Rerun without --reprocess to resume; completed groups are skipped.");
    }
    message.push_str(&format!(
        "\nFull errors: sqlite3 <db> \"SELECT root_item_id,error FROM processing_failures WHERE run_id={}\"",
        report.run_id
    ));
    message
}
pub fn run_with_api<C: HnApi>(
    args: &[String],
    api: C,
    out: &mut impl Write,
    err: &mut impl Write,
) -> io::Result<u8> {
    if args.is_empty() || args == ["--help"] || args.iter().any(|a| a == "--help") {
        writeln!(out, "{USAGE}")?;
        return Ok(0);
    }
    let result = run(args, api, out);
    match result {
        Ok(()) => Ok(0),
        Err(message) => {
            writeln!(err, "error: {message}")?;
            Ok(1)
        }
    }
}
fn run<C: HnApi>(args: &[String], api: C, out: &mut impl Write) -> Result<(), String> {
    if args[0] == "collect" || args[0] == "process" {
        let _ = thread_id(args)?;
        let _ = concurrency(args)?;
    }
    let mut store = open(args)?;
    match args[0].as_str() {
        "collect" => {
            let id = thread_id(args)?;
            let report = Collector::new(api, CollectionConfig::default())
                .collect(&mut store, id)
                .map_err(|e| e.to_string())?;
            writeln!(
                out,
                "Collection run {}: fetched {} item(s), changed {} item(s).",
                report.run_id,
                report.items_fetched,
                report.changed_item_ids.len()
            )
            .map_err(|e| e.to_string())?;
            if !report.complete {
                return Err(format!(
                    "Collection incomplete for thread {id}: {} failed item(s): {}",
                    report.failures.len(),
                    report
                        .failures
                        .iter()
                        .map(|f| format!("item {}: {}: {}", f.item_id, f.kind, f.detail))
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
            writeln!(out, "Collection complete for thread {id}.").map_err(|e| e.to_string())?;
        }
        "process" => {
            // Processing reads only stored source; `collect` is the explicit HN refresh.
            let id = thread_id(args)?;
            let run = store
                .latest_run_for_thread(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| {
                    format!("thread {id} has not been collected; run `wrkgn collect --thread {id}` first")
                })?;
            if run.status != "complete" {
                return Err(format!(
                    "latest collection of thread {id} (run {}) is {}; run `wrkgn collect --thread {id}` again first",
                    run.id, run.status
                ));
            }
            writeln!(
                out,
                "Using collection run {} of thread {id}: {} item(s).",
                run.id, run.items_fetched
            )
            .map_err(|e| e.to_string())?;
            let model = RigModel::from_env()?;
            let options = processing::Options {
                concurrency: concurrency(args)?,
                ..processing::Options::default()
            };
            let mut status = Status::new();
            let summary = processing::process_thread_with(
                &mut store,
                id,
                &model,
                args.iter().any(|a| a == "--reprocess"),
                &options,
                |p| status.update(p),
            );
            status.finish();
            let summary = summary?;
            writeln!(
                out,
                "Processing: {} group(s) processed, {} skipped, {} failed, {} not attempted.",
                summary.processed,
                summary.skipped,
                summary.failed.len(),
                summary.not_attempted
            )
            .map_err(|e| e.to_string())?;
            let version = publication::export(&mut store, &export_path(args)?)?;
            writeln!(out, "Public snapshot: {version}").map_err(|e| e.to_string())?;
            if !summary.failed.is_empty() || summary.not_attempted > 0 {
                return Err(failure_summary(&summary));
            }
        }
        "review" => {
            let p = positional(args);
            match p.as_slice() {
                ["review", "pending"] => {
                    let width = std::env::var("COLUMNS")
                        .ok()
                        .and_then(|c| c.parse().ok())
                        .unwrap_or(100);
                    let mentions = publication::pending_mentions(&mut store)?;
                    write!(out, "{}", review_grid::render(&mentions, width))
                        .map_err(|e| e.to_string())?;
                    return Ok(());
                }
                ["review", "pending", "--json"] => {
                    for value in publication::pending(&mut store)? {
                        writeln!(out, "{value}").map_err(|e| e.to_string())?;
                    }
                    return Ok(());
                }
                ["review", "approve", ranges @ ..] => {
                    let n = publication::approve_ranges(
                        &mut store,
                        &review_grid::parse_ranges(ranges)?,
                    )?;
                    writeln!(out, "Approved {n} mention(s).").map_err(|e| e.to_string())?;
                }
                ["review", "correct", key, field, value] => {
                    publication::correct(&mut store, key, field, Some(value))?;
                }
                ["review", "suppress-source", id, reason] => {
                    publication::suppress_source(&mut store, parse_id(id)?, reason)?;
                }
                ["review", "suppress-project", id] => {
                    publication::suppress_project(&mut store, parse_id(id)?)?;
                }
                ["review", "match", from, to] => {
                    publication::match_projects(&mut store, parse_id(from)?, parse_id(to)?)?;
                }
                _ => return Err(USAGE.into()),
            }
            let version = publication::export(&mut store, &export_path(args)?)?;
            writeln!(out, "Public snapshot: {version}").map_err(|e| e.to_string())?;
        }
        "export" => {
            let version = publication::export(&mut store, &export_path(args)?)?;
            writeln!(out, "Public snapshot: {version}").map_err(|e| e.to_string())?;
        }
        "categories" if positional(args) == ["categories", "normalize"] => {
            let (old, dropped) = category_migration::normalize(&mut store)?;
            writeln!(
                out,
                "Normalized {old} category assignments; dropped {dropped} generic assignments."
            )
            .map_err(|e| e.to_string())?;
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}
fn parse_id(s: &str) -> Result<i64, String> {
    let id = s.parse::<i64>().map_err(|_| "invalid ID")?;
    if id <= 0 {
        Err("invalid ID".into())
    } else {
        Ok(id)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failure_summary_groups_errors_by_kind() {
        let long = format!(
            "ProviderResponseError: status 429 Too Many Requests: {}",
            "x".repeat(700)
        );
        let report = processing::Report {
            run_id: 3,
            failed: vec![
                (1, long.clone()),
                (2, long),
                (3, "HttpError: error sending request for url".into()),
                (4, "URL absent from source".into()),
            ],
            not_attempted: 5,
            stopped_early: true,
            ..processing::Report::default()
        };
        let message = failure_summary(&report);
        assert!(
            message
                .starts_with("processing incomplete (run 3): 4 group(s) failed, 5 not attempted")
        );
        assert!(message.contains("\n  2 x rate limited (429) (e.g. group 1)"));
        assert!(message.contains("\n  1 x connection or provider error (e.g. group 3)"));
        assert!(message.contains("\n  1 x URL absent from source (e.g. group 4)"));
        assert!(message.contains("Stopped early"));
        assert!(message.len() < 600);
    }
}
