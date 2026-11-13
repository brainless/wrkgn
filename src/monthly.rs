//! Sequential monthly orchestration over the existing local pipeline.
use crate::cli::{Status, failure_summary};
use crate::collector::{CollectionConfig, Collector, HnApi};
use crate::processing::{self, Model, RigModel};
use crate::{store::Store, workflow};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const HELP: &str = "Usage: wrkgn-monthly [--from YYYY-MM] [--through YYYY-MM]
    [--concurrency N] [--db PATH] [--export DIRECTORY]

Defaults: current UTC month through 23 months earlier (24 months inclusive).
Search every month newest first, collecting, processing, automatically approving,
and exporting each matching thread before starting the next. Missing months are
skipped. Rerun to refresh source and resume unchanged successful groups.
Uses the same .env, provider, database and export settings as wrkgn.
This command bypasses manual review. Website staging remains explicit.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Month {
    pub year: u16,
    pub month: u8,
}

impl Month {
    pub fn parse(value: &str) -> Result<Self, String> {
        let invalid = || format!("invalid month {value:?}; expected YYYY-MM (year 0001–9999)");
        if !value.is_ascii() || value.len() != 7 || value.as_bytes()[4] != b'-' {
            return Err(invalid());
        }
        if !value[..4].bytes().all(|c| c.is_ascii_digit())
            || !value[5..].bytes().all(|c| c.is_ascii_digit())
        {
            return Err(invalid());
        }
        let year = value[..4].parse().map_err(|_| invalid())?;
        let month = value[5..].parse().map_err(|_| invalid())?;
        if year == 0 || !(1..=12).contains(&month) {
            return Err(invalid());
        }
        Ok(Self { year, month })
    }

    pub fn previous(self) -> Option<Self> {
        if self.month > 1 {
            Some(Self {
                month: self.month - 1,
                ..self
            })
        } else {
            self.year
                .checked_sub(1)
                .filter(|y| *y > 0)
                .map(|year| Self { year, month: 12 })
        }
    }

    fn current() -> Result<Self, String> {
        // SQLite supplies the UTC calendar date without another date dependency.
        let date: String = rusqlite::Connection::open_in_memory()
            .and_then(|c| c.query_row("SELECT strftime('%Y-%m','now')", [], |r| r.get(0)))
            .map_err(|e| e.to_string())?;
        Self::parse(&date)
    }

    pub fn matches_title(self, title: &str) -> bool {
        let title = title.to_lowercase();
        let title = title
            .trim()
            .strip_prefix("ask hn:")
            .unwrap_or(title.trim())
            .trim();
        let normalized = title.replace('?', "");
        let full = MONTHS[self.month as usize - 1].to_lowercase();
        let short = &full[..3];
        [
            full.as_str(),
            short,
            if self.month == 9 { "sept" } else { short },
        ]
        .iter()
        .any(|name| normalized == format!("what are you working on ({name} {})", self.year))
    }
}

impl std::fmt::Display for Month {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

pub trait Discovery {
    fn threads(&self, month: Month) -> Result<Vec<i64>, String>;
}

pub struct HttpDiscovery {
    agent: ureq::Agent,
}

impl Default for HttpDiscovery {
    fn default() -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(30)))
                .build()
                .new_agent(),
        }
    }
}

#[derive(Deserialize)]
struct SearchHit {
    #[serde(rename = "objectID")]
    id: String,
    title: Option<String>,
}
#[derive(Deserialize)]
struct SearchPage {
    hits: Vec<SearchHit>,
    #[serde(rename = "nbPages")]
    pages: usize,
    #[serde(rename = "exhaustiveNbHits")]
    exhaustive: Option<bool>,
}

// Search broadly by title/year so abbreviations do not depend on typo matching.
// Fetch all pages, then validate the complete title locally.
fn discover_pages(
    month: Month,
    mut fetch: impl FnMut(usize) -> Result<SearchPage, String>,
) -> Result<Vec<i64>, String> {
    let mut ids = BTreeSet::new();
    let mut page = 0;
    loop {
        let result = fetch(page)?;
        if result.exhaustive == Some(false) {
            return Err(format!("search results for {month} are not exhaustive"));
        }
        for hit in result.hits {
            if hit
                .title
                .as_deref()
                .is_some_and(|title| month.matches_title(title))
            {
                let id = hit
                    .id
                    .parse::<i64>()
                    .ok()
                    .filter(|id| *id > 0)
                    .ok_or("search returned an invalid HN item ID")?;
                ids.insert(id);
            }
        }
        page += 1;
        if page >= result.pages {
            break;
        }
    }
    Ok(ids.into_iter().rev().collect())
}

impl Discovery for HttpDiscovery {
    fn threads(&self, month: Month) -> Result<Vec<i64>, String> {
        discover_pages(month, |page| {
            let mut url = url::Url::parse("https://hn.algolia.com/api/v1/search").unwrap();
            url.query_pairs_mut()
                .append_pair("query", &format!("What are you working on {}", month.year))
                .append_pair("tags", "story")
                .append_pair("restrictSearchableAttributes", "title")
                .append_pair("typoTolerance", "false")
                .append_pair("hitsPerPage", "100")
                .append_pair("page", &page.to_string());
            for attempt in 0..3 {
                match self.agent.get(url.as_str()).call() {
                    Ok(mut response) => {
                        return response
                            .body_mut()
                            .read_json::<SearchPage>()
                            .map_err(|e| format!("invalid discovery response for {month}: {e}"));
                    }
                    Err(error) => {
                        let retry = matches!(
                            error,
                            ureq::Error::StatusCode(429 | 500..=599)
                                | ureq::Error::Timeout(_)
                                | ureq::Error::Io(_)
                                | ureq::Error::HostNotFound
                        );
                        if !retry || attempt == 2 {
                            return Err(format!("thread discovery failed for {month}: {error}"));
                        }
                        std::thread::sleep(Duration::from_secs(1 << attempt));
                    }
                }
            }
            unreachable!()
        })
    }
}

/// A complete thread is processed before approval; failures stop traversal so a
/// rerun can resume using the existing successful-group fingerprints.
pub fn sync_thread<C: HnApi, M: Model + Sync>(
    store: &mut Store,
    collector: &Collector<C>,
    model: &M,
    thread: i64,
    options: &processing::Options,
    destination: &Path,
    out: &mut impl Write,
) -> Result<(), String> {
    if store.extraction_mode(thread)? == Some(crate::store::ExtractionMode::ShowHn) {
        return Err(format!(
            "thread {thread} is assigned to show_hn extraction; monthly workflow cannot process it"
        ));
    }
    let mut status = Status::new();
    let result = workflow::sync_thread_checked(
        store,
        collector,
        model,
        thread,
        &workflow::ThreadOptions {
            processing: options,
            reprocess: false,
            approval: workflow::ApprovalPolicy::ApproveThread,
            destination,
        },
        |store, thread| {
            store.assign_extraction_mode(thread, crate::store::ExtractionMode::Monthly)?;
            Ok(true)
        },
        |event| {
            match event {
                workflow::Event::Collected(collection) => writeln!(
                    out,
                    "Thread {thread}: collected {} items, {} changed.",
                    collection.items_fetched,
                    collection.changed_item_ids.len()
                ),
                workflow::Event::Progress(progress) => {
                    status.update(progress);
                    Ok(())
                }
                workflow::Event::Processed(report) => {
                    status.finish();
                    writeln!(
                        out,
                        "Thread {thread}: {} processed, {} unchanged, {} failed, {} not attempted.",
                        report.processed,
                        report.skipped,
                        report.failed.len(),
                        report.not_attempted
                    )
                }
                workflow::Event::Exported { approved, version } => writeln!(
                    out,
                    "Thread {thread}: auto-approved {} mentions. Public snapshot: {version}",
                    approved.expect("monthly workflow authorizes thread approval")
                ),
                workflow::Event::Skipped { .. } => {
                    unreachable!("monthly processing is not skipped")
                }
            }
            .map_err(|e| e.to_string())
        },
    );
    match result {
        Ok(()) => out.flush().map_err(|e| e.to_string()),
        Err(workflow::Error::ProcessingIncomplete(report)) => Err(failure_summary(&report)),
        Err(workflow::Error::Operation(error)) => {
            status.finish();
            Err(error)
        }
    }
}

struct Args {
    from: Month,
    through: Month,
    db: Option<PathBuf>,
    export: PathBuf,
    options: processing::Options,
}

impl Args {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut from = None;
        let mut through = None;
        let mut db = None;
        let mut export = std::env::var_os("WRKGN_EXPORT_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("local-data/public-export"));
        let mut options = processing::Options::default();
        let mut seen = BTreeSet::new();
        let mut args = args.iter();
        while let Some(flag) = args.next() {
            if !["--from", "--through", "--db", "--export", "--concurrency"]
                .contains(&flag.as_str())
            {
                return Err(format!("unknown argument {flag}\n{HELP}"));
            }
            if !seen.insert(flag) {
                return Err(format!("duplicate argument {flag}"));
            }
            let value = args
                .next()
                .filter(|v| !v.starts_with("--"))
                .ok_or_else(|| format!("{flag} requires a value"))?;
            match flag.as_str() {
                "--from" => from = Some(Month::parse(value)?),
                "--through" => through = Some(Month::parse(value)?),
                "--db" => db = Some(PathBuf::from(value)),
                "--export" => export = PathBuf::from(value),
                "--concurrency" => {
                    options.concurrency = value
                        .parse::<usize>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or("--concurrency requires a positive integer")?
                }
                _ => unreachable!(),
            }
        }
        let from = from.map(Ok).unwrap_or_else(Month::current)?;
        let through =
            through.unwrap_or_else(|| (0..23).fold(from, |m, _| m.previous().unwrap_or(m)));
        if through > from {
            return Err("--through must be the same month as --from or earlier".into());
        }
        Ok(Self {
            from,
            through,
            db,
            export,
            options,
        })
    }
}

pub fn run<C: HnApi, D: Discovery>(
    args: &[String],
    api: C,
    discovery: D,
    out: &mut impl Write,
) -> Result<(), String> {
    if args.iter().any(|a| a == "--help") {
        return writeln!(out, "{HELP}").map_err(|e| e.to_string());
    }
    let args = Args::parse(args)?;
    let model = RigModel::from_env()?;
    let mut store = match &args.db {
        Some(path) => Store::open(path),
        None => Store::open_configured(),
    }
    .map_err(|e| e.to_string())?;
    let collector = Collector::new(api, CollectionConfig::default());
    execute(&mut store, &collector, &model, &discovery, &args, out)
}

fn execute<C: HnApi, M: Model + Sync, D: Discovery>(
    store: &mut Store,
    collector: &Collector<C>,
    model: &M,
    discovery: &D,
    args: &Args,
    out: &mut impl Write,
) -> Result<(), String> {
    let mut month = args.from;
    loop {
        let threads = discovery.threads(month)?;
        writeln!(out, "{month}: {} matching threads.", threads.len()).map_err(|e| e.to_string())?;
        // Deduplicate discovery responses before doing any collection or model work.
        for thread in threads
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .rev()
        {
            sync_thread(
                store,
                collector,
                model,
                thread,
                &args.options,
                &args.export,
                out,
            )?;
        }
        if month == args.through {
            break;
        }
        month = month.previous().ok_or("month range underflow")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_title_variants() {
        let month = Month::parse("2026-11").unwrap();
        for title in [
            "Ask HN: What are you working on? (November 2026)",
            "What are you working on (Nov 2026)?",
            "Ask HN: What are you working on (nov 2026)",
        ] {
            assert!(month.matches_title(title), "{title}");
        }
        for title in [
            "What are you working on (November 2025)",
            "What are you working on (October 2026)",
            "Poll: What are you working on (Nov 2026)",
        ] {
            assert!(!month.matches_title(title));
        }
        assert!(
            Month::parse("2026-09")
                .unwrap()
                .matches_title("What are you working on (Sept 2026)")
        );
        assert_eq!(
            Month::parse("2026-01").unwrap().previous(),
            Some(Month::parse("2025-12").unwrap())
        );
        for invalid in ["2026-00", "2026-13", "2026-1", "0000-01", "abcd-01"] {
            assert!(Month::parse(invalid).is_err());
        }
    }

    #[test]
    fn discovery_paginates_filters_and_deduplicates() {
        let mut calls = Vec::new();
        let ids = discover_pages(Month::parse("2026-11").unwrap(), |page| {
            calls.push(page);
            serde_json::from_value(serde_json::json!({
                "nbPages":2, "exhaustiveNbHits":true,
                "hits": [
                    {"objectID":"100", "title":"Ask HN: What are you working on? (November 2026)"},
                    {"objectID":if page == 0 {"200"} else {"300"}, "title":"What are you working on (Nov 2026)?"},
                    {"objectID":"999", "title":"What are you working on (October 2026)"}
                ]
            })).map_err(|e| e.to_string())
        }).unwrap();
        assert_eq!(calls, vec![0, 1]);
        assert_eq!(ids, vec![300, 200, 100]);
        assert!(
            discover_pages(Month::parse("2026-11").unwrap(), |_| Ok(SearchPage {
                hits: vec![],
                pages: 1,
                exhaustive: Some(false),
            }))
            .is_err()
        );
    }

    #[test]
    fn arguments_validate_before_work() {
        let args = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        let parsed = Args::parse(&args("--from 2026-11")).unwrap();
        assert_eq!(parsed.through, Month::parse("2024-12").unwrap());
        for invalid in [
            "--from",
            "--from 2026-13",
            "--from 2026-11 --through 2026-12",
            "--concurrency 0",
            "--concurrency nope",
            "--unknown thing",
            "--db a --db b",
        ] {
            assert!(Args::parse(&args(invalid)).is_err(), "{invalid}");
        }
    }

    #[test]
    fn traversal_finishes_every_thread_before_older_months_and_stops_on_failure() {
        use crate::collector::FetchError;
        use crate::extraction::Extraction;
        use std::sync::Mutex;
        struct Fixture(Mutex<Vec<String>>, bool);
        impl Discovery for &Fixture {
            fn threads(&self, month: Month) -> Result<Vec<i64>, String> {
                self.0.lock().unwrap().push(format!("search {month}"));
                Ok(match month.month {
                    1 => vec![100, 200, 100],
                    12 => vec![],
                    11 => vec![300],
                    _ => unreachable!(),
                })
            }
        }
        impl HnApi for &Fixture {
            fn fetch_item(&self, id: i64) -> Result<Option<serde_json::Value>, FetchError> {
                self.0.lock().unwrap().push(format!("fetch {id}"));
                Ok(if self.1 && id == 100 {
                    None
                } else {
                    Some(serde_json::json!({"id":id,"kids":[]}))
                })
            }
        }
        struct NoComments;
        impl Model for NoComments {
            fn name(&self) -> &str {
                "fixture"
            }
            fn extract(&self, _: &str) -> Result<Extraction, String> {
                panic!("no comments to process")
            }
        }
        for fail in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = Store::open(dir.path().join("db")).unwrap();
            let fixture = Fixture(Mutex::new(vec![]), fail);
            let args = Args {
                from: Month::parse("2026-01").unwrap(),
                through: Month::parse("2025-11").unwrap(),
                db: None,
                export: dir.path().join("export"),
                options: processing::Options::default(),
            };
            let collector = Collector::new(&fixture, CollectionConfig::default());
            let mut output = Vec::new();
            let result = execute(
                &mut store,
                &collector,
                &NoComments,
                &&fixture,
                &args,
                &mut output,
            );
            assert_eq!(result.is_err(), fail);
            let calls = fixture.0.lock().unwrap();
            if fail {
                assert_eq!(*calls, ["search 2026-01", "fetch 200", "fetch 100"]);
            } else {
                assert_eq!(
                    *calls,
                    [
                        "search 2026-01",
                        "fetch 200",
                        "fetch 100",
                        "search 2025-12",
                        "search 2025-11",
                        "fetch 300"
                    ]
                );
                assert_eq!(
                    String::from_utf8(output)
                        .unwrap()
                        .matches("Public snapshot:")
                        .count(),
                    3
                );
            }
        }
    }
}
