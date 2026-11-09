//! Explicit local pipeline and review commands.
use crate::collector::{CollectionConfig, Collector, HnApi};
use crate::processing::{self, RigModel};
use crate::publication;
use crate::store::Store;
use std::io::{self, Write};
use std::path::PathBuf;
const USAGE:&str="Usage:
  wrkgn collect --thread <id> [--db <path>]
  wrkgn process --thread <id> [--reprocess] [--db <path>] [--export <directory>]
  wrkgn review pending [--db <path>]
  wrkgn review approve <mention-id> [--db <path>]
  wrkgn review correct <mention-key> <name|description|problem|audience|builder_username> <value> [--db <path>]
  wrkgn review suppress-source <source-id> <reason> [--db <path>]
  wrkgn review suppress-project <project-id> [--db <path>]
  wrkgn review match <from-project-id> <to-project-id> [--db <path>]
  wrkgn export [--db <path>] [--export <directory>]
  wrkgn --help

WRKGN_DB_PATH defaults to local-data/wrkgn.sqlite. WRKGN_EXPORT_PATH defaults to local-data/public-export.
process uses WRKGN_MODEL and OPENAI_API_KEY. Review and export make no network calls.";
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
        if ["--db", "--export", "--thread"].contains(&args[i].as_str()) {
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
    }
    let mut store = open(args)?;
    match args[0].as_str() {
        "collect" | "process" => {
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
            if args[0] == "process" {
                let model = RigModel::from_env()?;
                let summary = processing::process_thread(
                    &mut store,
                    id,
                    &model,
                    args.iter().any(|a| a == "--reprocess"),
                )?;
                writeln!(
                    out,
                    "Processing: {} group(s) processed, {} skipped, {} failed.",
                    summary.processed,
                    summary.skipped,
                    summary.failed.len()
                )
                .map_err(|e| e.to_string())?;
                let version = publication::export(&mut store, &export_path(args)?)?;
                writeln!(out, "Public snapshot: {version}").map_err(|e| e.to_string())?;
                if !summary.failed.is_empty() {
                    return Err(format!("processing incomplete: {:?}", summary.failed));
                }
            }
        }
        "review" => {
            let p = positional(args);
            match p.as_slice() {
                ["review", "pending"] => {
                    for value in publication::pending(&mut store)? {
                        writeln!(out, "{value}").map_err(|e| e.to_string())?;
                    }
                }
                ["review", "approve", id] => {
                    publication::approve(&mut store, parse_id(id)?)?;
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
                _ => return Err(USAGE.into()),
            }
            let version = publication::export(&mut store, &export_path(args)?)?;
            writeln!(out, "Public snapshot: {version}").map_err(|e| e.to_string())?;
        }
        "export" => {
            let version = publication::export(&mut store, &export_path(args)?)?;
            writeln!(out, "Public snapshot: {version}").map_err(|e| e.to_string())?;
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
