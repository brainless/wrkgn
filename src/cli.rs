//! Small CLI surface for local collection. `process` remains incomplete until
//! extraction and public export are implemented in a later epic.

use std::io::{self, Write};
use std::path::PathBuf;

use crate::collector::{CollectionConfig, Collector, HnApi};
use crate::store::Store;

const USAGE: &str = "Usage:
  wrkgn collect --thread <hn-item-id> [--db <path>]
  wrkgn process --thread <hn-item-id> [--db <path>]
  wrkgn --help

The database defaults to local-data/wrkgn.sqlite; WRKGN_DB_PATH overrides it.
collect fetches the thread and replies only. process currently collects, then
reports that extraction and export are unavailable.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Collect,
    Process,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Invocation {
    command: Command,
    thread_id: i64,
    db_path: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Option<Invocation>, String> {
    if args == ["--help"] || args == ["collect", "--help"] || args == ["process", "--help"] {
        return Ok(None);
    }
    let command = match args.first().map(String::as_str) {
        Some("collect") => Command::Collect,
        Some("process") => Command::Process,
        Some(other) => return Err(format!("unknown command: {other}")),
        None => return Err("missing command".into()),
    };
    let mut thread_id = None;
    let mut db_path = None;
    let mut remaining = args[1..].iter();
    while let Some(option) = remaining.next() {
        match option.as_str() {
            "--thread" if thread_id.is_none() => {
                let value = remaining.next().ok_or("--thread requires an item ID")?;
                let parsed = value
                    .parse::<i64>()
                    .map_err(|_| "thread ID must be a positive integer")?;
                if parsed <= 0 {
                    return Err("thread ID must be a positive integer".into());
                }
                thread_id = Some(parsed);
            }
            "--db" if db_path.is_none() => {
                let value = remaining.next().ok_or("--db requires a path")?;
                if value.is_empty() || value.starts_with("--") {
                    return Err("--db requires a path".into());
                }
                db_path = Some(PathBuf::from(value));
            }
            _ => return Err(format!("unexpected or repeated option: {option}")),
        }
    }
    Ok(Some(Invocation {
        command,
        thread_id: thread_id.ok_or("--thread is required")?,
        db_path,
    }))
}

/// Runs the CLI against an injectable HN transport. Exit status 0 means a
/// complete collection, 1 an incomplete/failed collection, and 2 a usage error
/// or a `process` run whose extraction/export stages are unavailable.
pub fn run_with_api<C: HnApi>(
    args: &[String],
    api: C,
    out: &mut impl Write,
    err: &mut impl Write,
) -> io::Result<u8> {
    let invocation = match parse(args) {
        Ok(Some(invocation)) => invocation,
        Ok(None) => {
            writeln!(out, "{USAGE}")?;
            return Ok(0);
        }
        Err(message) => {
            writeln!(err, "error: {message}\n{USAGE}")?;
            return Ok(2);
        }
    };
    let opened = match invocation.db_path {
        Some(path) => Store::open(path),
        None => Store::open_configured(),
    };
    let mut store = match opened {
        Ok(store) => store,
        Err(error) => {
            writeln!(err, "database error: {error}")?;
            return Ok(1);
        }
    };
    let collector = Collector::new(api, CollectionConfig::default());
    let report = match collector.collect(&mut store, invocation.thread_id) {
        Ok(report) => report,
        Err(error) => {
            writeln!(err, "collection error: {error}")?;
            return Ok(1);
        }
    };

    writeln!(
        out,
        "Collection run {}: fetched {} item(s), changed {} item(s).",
        report.run_id,
        report.items_fetched,
        report.changed_item_ids.len()
    )?;
    if let Some(prior_run_id) = report.resumed_from_run_id {
        writeln!(
            out,
            "Retried after incomplete or interrupted run {prior_run_id}; refreshed the full thread."
        )?;
    }
    if report.complete {
        writeln!(out, "Collection complete for thread {}.", report.thread_id)?;
    } else {
        writeln!(
            err,
            "Collection incomplete for thread {}: {} failed item(s).",
            report.thread_id,
            report.failures.len()
        )?;
        for failure in &report.failures {
            writeln!(
                err,
                "  item {}: {}: {}",
                failure.item_id, failure.kind, failure.detail
            )?;
        }
    }
    if invocation.command == Command::Process {
        writeln!(
            err,
            "Extraction and public export are not implemented; the processing pipeline is incomplete."
        )?;
        return Ok(2);
    }
    Ok(u8::from(!report.complete))
}
