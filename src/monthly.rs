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
skipped. Thread and discovery failures are logged while remaining work continues;
the final summary exits nonzero if any failed. Rerun to resume successful groups.
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

    fn search_queries(self) -> Vec<String> {
        let full = MONTHS[self.month as usize - 1];
        let mut names = BTreeSet::from([full, &full[..3]]);
        if self.month == 9 {
            names.insert("Sept");
        }
        names
            .into_iter()
            .map(|name| format!("What are you working on {name} {}", self.year))
            .collect()
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
    #[serde(rename = "nbHits")]
    total: usize,
    #[serde(rename = "hitsPerPage")]
    per_page: usize,
}

// Fetch every advertised page and check the result window, then validate titles.
// exhaustiveNbHits=false describes an estimated count, not pagination failure.
fn discover_pages(
    month: Month,
    mut fetch: impl FnMut(usize) -> Result<SearchPage, String>,
) -> Result<Vec<i64>, String> {
    let mut ids = BTreeSet::new();
    let mut page = 0;
    loop {
        let result = fetch(page)?;
        if result.per_page == 0
            || result.total > result.pages.saturating_mul(result.per_page)
            || (result.total > 0 && result.hits.is_empty())
        {
            return Err(format!(
                "search results for {month} exceed the available pagination window or returned an incomplete page"
            ));
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
        let mut ids = BTreeSet::new();
        for query in month.search_queries() {
            ids.extend(discover_pages(month, |page| {
                self.search_page(month, &query, page)
            })?);
        }
        Ok(ids.into_iter().rev().collect())
    }
}

impl HttpDiscovery {
    fn search_page(&self, month: Month, query: &str, page: usize) -> Result<SearchPage, String> {
        let mut url = url::Url::parse("https://hn.algolia.com/api/v1/search").unwrap();
        url.query_pairs_mut()
            .append_pair("query", query)
            .append_pair("tags", "story")
            .append_pair("restrictSearchableAttributes", "title")
            .append_pair("typoTolerance", "false")
            .append_pair("queryType", "prefixNone")
            .append_pair("removeWordsIfNoResults", "none")
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
    }
}

/// A complete thread is processed before approval. A failed thread retains its
/// successful groups so a later run can resume without approving partial work.
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
    let mut failures = Vec::new();
    let mut succeeded = 0;
    let mut failed_threads = 0;
    let mut failed_months = 0;
    loop {
        let threads = match discovery.threads(month) {
            Ok(threads) => {
                writeln!(out, "{month}: {} matching threads.", threads.len())
                    .map_err(|e| e.to_string())?;
                threads
            }
            Err(error) => {
                let failure = format!("{month} discovery failed: {error}");
                writeln!(out, "{failure}").map_err(|e| e.to_string())?;
                out.flush().map_err(|e| e.to_string())?;
                failures.push(failure);
                failed_months += 1;
                vec![]
            }
        };
        // Deduplicate discovery responses before doing any collection or model work.
        for thread in threads
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .rev()
        {
            let mut output = ObservedWriter {
                inner: out,
                failed: false,
            };
            let result = sync_thread(
                store,
                collector,
                model,
                thread,
                &args.options,
                &args.export,
                &mut output,
            );
            if output.failed {
                return Err(result
                    .err()
                    .unwrap_or_else(|| "monthly output failed".into()));
            }
            match result {
                Ok(()) => succeeded += 1,
                Err(error) => {
                    let failure = format!("Thread {thread} ({month}) failed: {error}");
                    writeln!(out, "{failure}").map_err(|e| e.to_string())?;
                    out.flush().map_err(|e| e.to_string())?;
                    failures.push(failure);
                    failed_threads += 1;
                }
            }
        }
        if month == args.through {
            break;
        }
        month = month.previous().ok_or("month range underflow")?;
    }
    writeln!(out, "Monthly summary: {succeeded} thread(s) succeeded, {failed_threads} thread(s) failed, {failed_months} month discovery failure(s).")
        .map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "monthly run completed with {failed_threads} failed thread(s) and {failed_months} month discovery failure(s):\n{}\nRerun the same command to retry failures and skip unchanged successful groups.",
            failures.join("\n")
        ))
    }
}

/// A presentation failure must stop the command even though workflow operation
/// errors otherwise remain local to the thread being handled.
struct ObservedWriter<'a, W> {
    inner: &'a mut W,
    failed: bool,
}
impl<W: Write> Write for ObservedWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let result = self.inner.write(buf);
        self.failed |= result.is_err();
        result
    }
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let result = self.inner.write_all(buf);
        self.failed |= result.is_err();
        result
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let result = self.inner.flush();
        self.failed |= result.is_err();
        result
    }
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
                "nbPages":2, "nbHits":6, "hitsPerPage":3, "exhaustiveNbHits":false,
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
                hits: vec![SearchHit {
                    id: "100".into(),
                    title: Some("What are you working on (Nov 2026)".into()),
                }],
                pages: 1,
                total: 200,
                per_page: 100,
            }))
            .is_err()
        );
    }

    #[test]
    fn estimated_empty_results_are_valid_and_queries_cover_month_spellings() {
        let ids = discover_pages(Month::parse("2026-11").unwrap(), |_| {
            serde_json::from_value(serde_json::json!({
                "hits":[], "nbPages":0, "nbHits":0, "hitsPerPage":100,
                "exhaustiveNbHits":false
            }))
            .map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(ids.is_empty());
        let month = Month::parse("2026-11").unwrap();
        assert_eq!(
            month.search_queries(),
            [
                "What are you working on Nov 2026",
                "What are you working on November 2026"
            ]
        );
        assert_eq!(Month::parse("2026-05").unwrap().search_queries().len(), 1);
        assert_eq!(Month::parse("2026-09").unwrap().search_queries().len(), 3);
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
    fn traversal_continues_same_month_and_older_months_after_failures() {
        use crate::collector::FetchError;
        use crate::extraction::Extraction;
        use std::sync::Mutex;
        struct Fixture(Mutex<Vec<String>>, bool, bool);
        impl Discovery for &Fixture {
            fn threads(&self, month: Month) -> Result<Vec<i64>, String> {
                self.0.lock().unwrap().push(format!("search {month}"));
                if self.2 && month.month == 12 {
                    return Err("fixture discovery unavailable".into());
                }
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
                Ok(if self.1 && id == 200 {
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
        for (fail_thread, fail_discovery) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let dir = tempfile::tempdir().unwrap();
            let mut store = Store::open(dir.path().join("db")).unwrap();
            let fixture = Fixture(Mutex::new(vec![]), fail_thread, fail_discovery);
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
            assert_eq!(result.is_err(), fail_thread || fail_discovery);
            let calls = fixture.0.lock().unwrap();
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
            let output = String::from_utf8(output).unwrap();
            assert_eq!(
                output.matches("Public snapshot:").count(),
                if fail_thread { 2 } else { 3 }
            );
            if fail_thread {
                let failure = "Thread 200 (2026-01) failed:";
                assert!(output.find(failure).unwrap() < output.find("Thread 100:").unwrap());
                assert!(result.as_ref().unwrap_err().contains(failure));
            }
            if fail_discovery {
                assert!(output.contains("2025-12 discovery failed: fixture discovery unavailable"));
                assert!(!output.contains("2025-12: 0 matching threads"));
                assert!(
                    result
                        .as_ref()
                        .unwrap_err()
                        .contains("2025-12 discovery failed")
                );
            }
            assert!(output.contains("Monthly summary:"));
        }
    }

    #[test]
    fn processing_failure_retains_pending_work_while_later_threads_publish_and_resume() {
        use crate::collector::FetchError;
        use crate::extraction::{Extraction, Mention};
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Api;
        impl HnApi for Api {
            fn fetch_item(&self, id: i64) -> Result<Option<serde_json::Value>, FetchError> {
                Ok(Some(if id % 100 == 0 {
                    serde_json::json!({"id":id,"kids":if id == 200 {vec![201,202]} else {vec![id+1]}})
                } else {
                    serde_json::json!({"id":id,"parent":id/100*100,"by":"alice","time":id,
                        "text":format!("Project {id} https://example.test/p{id}")})
                }))
            }
        }
        struct Search;
        impl Discovery for Search {
            fn threads(&self, month: Month) -> Result<Vec<i64>, String> {
                Ok(if month.month == 1 {
                    vec![100, 200]
                } else {
                    vec![300]
                })
            }
        }
        struct Fixture {
            fail: AtomicBool,
            calls: Mutex<Vec<i64>>,
        }
        impl Model for Fixture {
            fn name(&self) -> &str {
                "continue-fixture"
            }
            fn extract(&self, prompt: &str) -> Result<Extraction, String> {
                let records: serde_json::Value = serde_json::from_str(
                    prompt.strip_prefix("Source records (JSON data): ").unwrap(),
                )
                .unwrap();
                let id = records[0]["id"].as_i64().unwrap();
                self.calls.lock().unwrap().push(id);
                if id == 201 && self.fail.load(Ordering::SeqCst) {
                    return Err("fixture extraction failure".into());
                }
                Ok(Extraction {
                    mentions: vec![Mention {
                        source_item_id: id,
                        name: Some(format!("Project {id}")),
                        urls: vec![format!("https://example.test/p{id}")],
                        description: format!("Project {id}"),
                        problem: None,
                        audience: None,
                        source_excerpt: format!("Project {id}"),
                        builders: vec![],
                        categories: vec![],
                        audience_labels: vec![],
                    }],
                })
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("db")).unwrap();
        let collector = Collector::new(Api, CollectionConfig::default());
        let model = Fixture {
            fail: AtomicBool::new(true),
            calls: Mutex::new(vec![]),
        };
        let args = Args {
            from: Month::parse("2026-01").unwrap(),
            through: Month::parse("2025-12").unwrap(),
            db: None,
            export: dir.path().join("export"),
            options: processing::Options {
                concurrency: 1,
                ..processing::Options::default()
            },
        };
        let mut output = vec![];
        let error =
            execute(&mut store, &collector, &model, &Search, &args, &mut output).unwrap_err();
        assert!(error.contains("Thread 200 (2026-01) failed: processing incomplete (run 1)"));
        assert!(error.contains("WHERE run_id=1"));
        let pending = crate::publication::pending_mentions(&mut store).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].source_item_id, 202);
        let approved: i64 = store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM mentions WHERE review_state='approved'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(approved, 2);
        assert_eq!(*model.calls.lock().unwrap(), [201, 202, 101, 301]);
        model.fail.store(false, Ordering::SeqCst);
        execute(&mut store, &collector, &model, &Search, &args, &mut vec![]).unwrap();
        assert!(
            crate::publication::pending_mentions(&mut store)
                .unwrap()
                .is_empty()
        );
        assert_eq!(*model.calls.lock().unwrap(), [201, 202, 101, 301, 201]);

        // An output error is fatal even if the writer would recover on its
        // next call: otherwise the failure could be mislabeled as a bad thread.
        struct FailOnce(bool);
        impl Write for FailOnce {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if !self.0 && bytes.starts_with(b"Thread ") {
                    self.0 = true;
                    Err(std::io::Error::other("fixture output failure"))
                } else {
                    Ok(bytes.len())
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut store = Store::open(dir.path().join("output-db")).unwrap();
        let error = execute(
            &mut store,
            &collector,
            &model,
            &Search,
            &args,
            &mut FailOnce(false),
        )
        .unwrap_err();
        assert!(error.contains("fixture output failure"));
        assert!(store.get_item(100).unwrap().is_none());
    }
}
