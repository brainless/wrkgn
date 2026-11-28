//! Monthly orchestration over the existing local pipeline, on the shared run
//! driver (`crate::run::driver`).
use crate::cli::{Status, failure_summary};
use crate::collector::{Collector, HnApi};
use crate::processing::{self, Model};
use crate::run::driver::{
    self, ChunkCx, DriverConfig, DriverResult, ItemOutcome, ProgressEvent, SeedReport, Workload,
};
use crate::run::status::{Clock, StatusLine};
use crate::run::{MAX_UNIT_ATTEMPTS, RunSession};
use crate::store::{QueueKind, UnitState};
use crate::{publication, store::Store, workflow};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Recent months re-walked fully on every run (`--months`).
pub const DEFAULT_HEAD_MONTHS: usize = 2;
/// Uncovered older months taken per run (`--backfill-months`).
pub const DEFAULT_BACKFILL_MONTHS: usize = 1;
/// Threads collected and processed per run by resume and backfill together
/// (`--max-items`). Head threads do not count.
pub const DEFAULT_MAX_ITEMS: usize = 5;

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
pub const HELP: &str = "Usage: wrkgn-monthly [--months N] [--backfill-months N] [--max-items N]
    [--concurrency N] [--db PATH] [--export DIRECTORY]
       wrkgn-monthly --from YYYY-MM [--through YYYY-MM] [--concurrency N] [--db PATH] [--export DIRECTORY]
       wrkgn-monthly --through YYYY-MM [--concurrency N] [--db PATH] [--export DIRECTORY]

With no range, each run is small and resumable; run it daily or from cron:
  1. Resume: finish months a previous run left pending, in progress or failed.
  2. Head: re-walk the current and previous UTC months (--months, default 2)
     completely; only changed conversation groups are reprocessed. Head threads
     do not count against --max-items.
  3. Backfill: take the newest uncovered months older than the head
     (--backfill-months, default 1), newest first, back to 2007-02. Empty months
     count and are marked done.
--max-items (default 5) caps threads collected and processed by resume and
backfill together; the run stops at a thread boundary and the next run continues.
Coverage, the thread queue and runs live in the SQLite database; there are no log
files. On the first run, months of threads already stored are recorded from their
titles and not searched again. A month with a failure is retried on later runs up
to 3 attempts, then listed for manual action. Comments, edits or deletions that
arrive after a thread leaves the head are picked up only by an explicit
wrkgn collect --thread followed by wrkgn process --thread, or by re-running that
month with --from M --through M.

--from/--through process exactly that range, newest first (--from alone: 24 months
ending there; --through alone: from the current month), bypassing head, backfill
and limits, and re-walking months already done. They cannot be combined with
--months, --backfill-months or --max-items.

Each thread is collected, processed, automatically approved and exported before
the next. Approval is automatic for each successfully processed thread; this
command bypasses manual review. A live status line and an end-of-run Markdown
report go to stderr. The header says which limit applied (thread cap, more
backfill remaining, or both). Exit code 0: completed or limit reached without
failures; 1: failures (including units still needing manual action from earlier
runs; the header counts them) or a setup error; 2: stopped after 3 consecutive failed calls to one
API (LLM provider, HN, Algolia); 130: interrupted (Ctrl-C or SIGTERM stops at the
next thread, exports, and reports; a second Ctrl-C exits at once). A second
instance exits immediately while one is running, so overlapping cron runs are safe.

Cron example (cron starts in the home directory; .env and the default
local-data/ paths are relative to the working directory). The report goes to
stderr and arrives as cron mail:
  17 3 * * * cd /path/to/wrkgn && ./target/release/wrkgn-monthly
Uses the same .env, provider, database and export settings as wrkgn.
Publication to the server remains explicit.";

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

    /// The start of HN's monthly threads; backfill never goes older.
    pub const FLOOR: Month = Month {
        year: 2007,
        month: 2,
    };

    pub fn next(self) -> Option<Self> {
        if self.month < 12 {
            Some(Self {
                month: self.month + 1,
                ..self
            })
        } else {
            self.year.checked_add(1).map(|year| Self { year, month: 1 })
        }
    }

    /// The UTC month containing the Unix time.
    pub fn at(unix: i64) -> Option<Self> {
        // Civil-from-days (Howard Hinnant).
        let z = unix.div_euclid(86_400) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        Some(Self {
            year: u16::try_from(year).ok().filter(|y| *y > 0)?,
            month: u8::try_from(month).ok()?,
        })
    }

    /// The current UTC month.
    pub fn current() -> Result<Self, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs();
        Self::at(i64::try_from(now).map_err(|e| e.to_string())?)
            .ok_or_else(|| "the system clock is out of range".to_string())
    }

    /// The month a thread title names: `What are you working on (<month> <year>)`
    /// with a full or three-letter month name (or `Sept`), an optional
    /// `Ask HN:` prefix and optional question marks, any letter case.
    pub fn from_title(title: &str) -> Option<Self> {
        let title = title.to_lowercase();
        let title = title
            .trim()
            .strip_prefix("ask hn:")
            .unwrap_or(title.trim())
            .trim();
        let normalized = title.replace('?', "");
        let inner = normalized
            .strip_prefix("what are you working on (")?
            .strip_suffix(')')?;
        let (name, year) = inner.split_once(' ')?;
        let year: u16 = year.parse().ok().filter(|y| *y > 0)?;
        if year.to_string() != inner[name.len() + 1..] {
            return None;
        }
        let month = (1..=12u8).find(|month| {
            let full = MONTHS[usize::from(*month) - 1].to_lowercase();
            name == full || name == &full[..3] || (*month == 9 && name == "sept")
        })?;
        Some(Self { year, month })
    }

    pub fn matches_title(self, title: &str) -> bool {
        Self::from_title(title) == Some(self)
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
    control: Option<std::sync::Arc<crate::run::RunControl>>,
}

impl Default for HttpDiscovery {
    fn default() -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(30)))
                .build()
                .new_agent(),
            control: None,
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
    /// Counts Algolia calls against the shared circuit breaker.
    pub fn with_control(mut self, control: std::sync::Arc<crate::run::RunControl>) -> Self {
        self.control = Some(control);
        self
    }

    fn count(&self, success: bool) {
        if let Some(control) = &self.control {
            if success {
                control.breaker.record_success(crate::run::Api::Algolia);
            } else {
                control.breaker.record_failure(crate::run::Api::Algolia);
            }
        }
    }
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
                    self.count(true);
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
                        // Only an exhausted retryable error counts as a failure.
                        if retry {
                            self.count(false);
                        }
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
        Err(workflow::Error::Stopped) => {
            status.finish();
            Err("collection stopped before the tree was fully walked; rerun to resume".into())
        }
    }
}

#[derive(Clone)]
pub struct Args {
    /// `--from/--through` override: months newest first. `None` is a default
    /// run (resume, head, backfill).
    pub range: Option<(Month, Month)>,
    pub head_months: usize,
    pub backfill_months: usize,
    pub max_items: usize,
    pub db: Option<PathBuf>,
    pub export: PathBuf,
    pub options: processing::Options,
}

impl Args {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut from = None;
        let mut through = None;
        let mut db = None;
        let mut export = std::env::var_os("WRKGN_EXPORT_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("local-data/public-export"));
        let mut options = processing::Options::default();
        let mut head_months = DEFAULT_HEAD_MONTHS;
        let mut backfill_months = DEFAULT_BACKFILL_MONTHS;
        let mut max_items = DEFAULT_MAX_ITEMS;
        let mut limit_flags = false;
        let mut seen = BTreeSet::new();
        let mut args = args.iter();
        let positive = |flag: &str, value: &str| {
            value
                .parse::<usize>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("{flag} requires a positive integer"))
        };
        while let Some(flag) = args.next() {
            if ![
                "--from",
                "--through",
                "--db",
                "--export",
                "--concurrency",
                "--months",
                "--backfill-months",
                "--max-items",
            ]
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
                "--concurrency" => options.concurrency = positive(flag, value)?,
                "--months" => {
                    head_months = positive(flag, value)?;
                    limit_flags = true;
                }
                "--backfill-months" => {
                    backfill_months = positive(flag, value)?;
                    limit_flags = true;
                }
                "--max-items" => {
                    max_items = positive(flag, value)?;
                    limit_flags = true;
                }
                _ => unreachable!(),
            }
        }
        let range = if from.is_some() || through.is_some() {
            if limit_flags {
                return Err(
                    "--from/--through cannot be combined with --months, --backfill-months or --max-items"
                        .into(),
                );
            }
            let from = from.map(Ok).unwrap_or_else(Month::current)?;
            let through =
                through.unwrap_or_else(|| (0..23).fold(from, |m, _| m.previous().unwrap_or(m)));
            if through > from {
                return Err("--through must be the same month as --from or earlier".into());
            }
            Some((from, through))
        } else {
            None
        };
        Ok(Self {
            range,
            head_months,
            backfill_months,
            max_items,
            db,
            export,
            options,
        })
    }

    /// The months an override processes, newest first.
    fn override_months(&self) -> Option<Vec<Month>> {
        let (from, through) = self.range?;
        let mut months = vec![from];
        let mut month = from;
        while month > through {
            month = month.previous()?;
            months.push(month);
        }
        Some(months)
    }
}

/// Monthly discussions on the shared run driver.
pub struct MonthlyWorkload<'a, C, D, M> {
    collector: &'a Collector<C>,
    model: &'a M,
    discovery: &'a D,
    options: processing::Options,
    export: PathBuf,
    head_months: usize,
}

fn thread_prepare(store: &mut Store, thread: i64) -> Result<bool, String> {
    if store.extraction_mode(thread)? == Some(crate::store::ExtractionMode::ShowHn) {
        return Err(format!(
            "thread {thread} is assigned to show_hn extraction; monthly workflow cannot process it"
        ));
    }
    store.assign_extraction_mode(thread, crate::store::ExtractionMode::Monthly)?;
    Ok(true)
}

impl<C: HnApi, D: Discovery, M: Model + Sync> Workload for MonthlyWorkload<'_, C, D, M> {
    type Unit = Month;
    const COMMAND: &'static str = "monthly";
    const ITEM_KIND: QueueKind = QueueKind::Thread;
    const ITEM_LABEL: &'static str = "thread";
    const UNIT_LABEL: &'static str = "month";

    fn parse_unit(&self, key: &str) -> Option<Month> {
        Month::parse(key).ok()
    }

    fn head_units(&self, now: i64, n: usize) -> Vec<Month> {
        let mut units = Vec::new();
        let mut month = Month::at(now);
        while let Some(current) = month {
            if units.len() >= n || current < Month::FLOOR {
                break;
            }
            units.push(current);
            month = current.previous();
        }
        units
    }

    fn floor(&self) -> Month {
        Month::FLOOR
    }

    fn previous(&self, unit: Month) -> Option<Month> {
        unit.previous()
    }

    fn discover(&self, unit: Month) -> Result<Vec<i64>, String> {
        self.discovery.threads(unit)
    }

    fn process_chunk(
        &self,
        cx: &mut ChunkCx<'_>,
        _unit: Option<Month>,
        ids: &[i64],
    ) -> Vec<(i64, ItemOutcome)> {
        let mut options = self.options.clone();
        options.control = Some(cx.control.clone());
        let mut outcomes = Vec::new();
        for &thread in ids {
            if cx.control.should_stop() {
                outcomes.push((thread, ItemOutcome::Stopped));
                break;
            }
            (cx.progress)(ProgressEvent::Item(thread));
            let mut first_failure = None;
            let result = workflow::stage_thread(
                cx.store,
                self.collector,
                self.model,
                thread,
                &workflow::StageOptions {
                    processing: &options,
                    reprocess: false,
                    approval: workflow::ApprovalPolicy::ApproveThread,
                },
                thread_prepare,
                |event| {
                    match event {
                        workflow::Event::Collected(collection) => {
                            (cx.progress)(ProgressEvent::Collected);
                            first_failure = collection
                                .failures
                                .first()
                                .map(|f| format!("item {}: {}", f.item_id, f.detail));
                        }
                        workflow::Event::Progress(progress) => {
                            (cx.progress)(ProgressEvent::Extract(progress.done, progress.total));
                        }
                        _ => {}
                    }
                    Ok(())
                },
            );
            let outcome = match result {
                Ok(workflow::Staged::Ready { approved }) => ItemOutcome::Done {
                    approved: approved.unwrap_or(0),
                },
                Ok(workflow::Staged::Withheld) => ItemOutcome::Done { approved: 0 },
                Err(error) => {
                    driver::outcome_from_error(error, cx.control, cx.errors, thread, first_failure)
                }
            };
            let stopped = outcome == ItemOutcome::Stopped;
            outcomes.push((thread, outcome));
            if stopped {
                break;
            }
        }
        outcomes
    }

    fn export(&self, store: &mut Store) -> Result<String, String> {
        publication::export(store, &self.export)
    }

    /// Records which month each stored monthly thread belongs to, from its
    /// title, so existing data is not searched or collected again. Months that
    /// already have coverage are left alone; a month is `done` only when all its
    /// stored threads were completely collected and processed and it is older
    /// than the head window.
    fn seed(&self, store: &mut Store, now: i64) -> Result<SeedReport, String> {
        let threads: Vec<(i64, Option<String>, bool)> = {
            let mut statement = store
                .connection()
                .prepare(
                    "SELECT s.item_id, json_extract(s.raw_payload,'$.title'),
                        COALESCE((SELECT status = 'complete' FROM collection_runs
                                  WHERE thread_id = s.item_id ORDER BY id DESC LIMIT 1), 0)
                        AND COALESCE((SELECT status = 'complete' FROM processing_runs
                                  WHERE thread_id = s.item_id ORDER BY id DESC LIMIT 1), 0)
                     FROM source_items s
                     JOIN thread_extraction_modes m ON m.thread_id = s.item_id
                     WHERE m.mode = 'monthly' AND s.item_id = s.thread_id
                     ORDER BY s.item_id DESC",
                )
                .map_err(|e| e.to_string())?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| e.to_string())?
        };
        let mut by_month: BTreeMap<Month, Vec<(i64, bool)>> = BTreeMap::new();
        let mut report = SeedReport::default();
        let mut unparsable = Vec::new();
        for (thread, title, complete) in threads {
            match title.as_deref().and_then(Month::from_title) {
                Some(month) => by_month.entry(month).or_default().push((thread, complete)),
                None => unparsable.push(thread),
            }
        }
        if !unparsable.is_empty() {
            report.notes.push(format!(
                "{} stored monthly thread(s) have no parsable month title and were not seeded: {:?}",
                unparsable.len(),
                &unparsable[..unparsable.len().min(10)]
            ));
        }
        let head: BTreeSet<Month> = self.head_units(now, self.head_months).into_iter().collect();
        for (month, items) in by_month {
            let done = items.iter().all(|(_, complete)| *complete) && !head.contains(&month);
            let state = if done {
                UnitState::Done
            } else {
                UnitState::Pending
            };
            if store
                .seed_unit(
                    Self::COMMAND,
                    QueueKind::Thread,
                    &month.to_string(),
                    state,
                    &items,
                    now,
                )
                .map_err(|e| e.to_string())?
            {
                report.units += 1;
                report.items += items.len();
            }
        }
        Ok(report)
    }
}

/// Runs the monthly workload on the shared driver. The collector should carry
/// `session.control` (`Collector::with_control`) so fetches reach the breaker.
#[allow(clippy::too_many_arguments)]
pub fn execute_driver<C: HnApi, D: Discovery, M: Model + Sync, O: Write, K: Clock>(
    store: &mut Store,
    session: &RunSession,
    collector: &Collector<C>,
    model: &M,
    discovery: &D,
    args: &Args,
    now: &dyn Fn() -> i64,
    status: &mut StatusLine<O, K>,
    err: &mut impl Write,
) -> DriverResult {
    let workload = MonthlyWorkload {
        collector,
        model,
        discovery,
        options: args.options.clone(),
        export: args.export.clone(),
        head_months: args.head_months,
    };
    let config = DriverConfig {
        head: args.head_months,
        backfill_units: args.backfill_months,
        max_items: args.max_items,
        max_attempts: MAX_UNIT_ATTEMPTS,
        range: args.override_months(),
        items: None,
    };
    driver::run(store, session, &workload, &config, now, status, err)
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
        assert_eq!(
            parsed.range,
            Some((
                Month::parse("2026-11").unwrap(),
                Month::parse("2024-12").unwrap()
            ))
        );
        assert_eq!(parsed.override_months().unwrap().len(), 24);
        let parsed = Args::parse(&args("--through 2026-09")).unwrap();
        assert_eq!(parsed.range.unwrap().1, Month::parse("2026-09").unwrap());
        let defaults = Args::parse(&[]).unwrap();
        assert!(defaults.range.is_none() && defaults.override_months().is_none());
        assert_eq!(
            (
                defaults.head_months,
                defaults.backfill_months,
                defaults.max_items
            ),
            (2, 1, 5)
        );
        let tuned = Args::parse(&args("--months 4 --backfill-months 2 --max-items 9")).unwrap();
        assert_eq!(
            (tuned.head_months, tuned.backfill_months, tuned.max_items),
            (4, 2, 9)
        );
        for invalid in [
            "--from",
            "--from 2026-13",
            "--from 2026-11 --through 2026-12",
            "--concurrency 0",
            "--concurrency nope",
            "--unknown thing",
            "--db a --db b",
            "--months 0",
            "--backfill-months x",
            "--max-items 0",
            "--max-items 1 --max-items 2",
            "--from 2026-11 --months 2",
            "--through 2026-11 --backfill-months 2",
            "--from 2026-11 --max-items 2",
        ] {
            assert!(Args::parse(&args(invalid)).is_err(), "{invalid}");
        }
    }

    #[test]
    fn title_parsing_agrees_with_matching() {
        let spellings = |month: u8| -> Vec<String> {
            let full = MONTHS[usize::from(month) - 1];
            let mut names = vec![full.to_string(), full[..3].to_string()];
            if month == 9 {
                names.push("Sept".into());
            }
            names
        };
        for number in 1..=12u8 {
            let month = Month {
                year: 2024,
                month: number,
            };
            for name in spellings(number) {
                for case in [name.clone(), name.to_lowercase(), name.to_uppercase()] {
                    for prefix in ["", "Ask HN: ", "ask hn:  "] {
                        for (open, close) in [("", ""), ("?", ""), ("", "?")] {
                            let title = format!(
                                "{prefix}What are you working on{open} ({case} 2024){close}"
                            );
                            assert_eq!(Month::from_title(&title), Some(month), "{title}");
                            assert!(month.matches_title(&title), "{title}");
                            let other = Month {
                                month: number % 12 + 1,
                                ..month
                            };
                            assert!(!other.matches_title(&title), "{title}");
                        }
                    }
                }
            }
        }
        for title in [
            "What are you working on (Sept 2026)",
            "What are you working on (Sep 2026)",
        ] {
            assert_eq!(Month::from_title(title), Month::parse("2026-09").ok());
        }
        for title in [
            "What are you working on (Janu 2026)",
            "What are you working on (Mar 026)",
            "What are you working on (Mar 02026)",
            "What are you working on (Mar 2026) now",
            "Poll: What are you working on (Mar 2026)",
            "Who is hiring? (March 2026)",
            "What are you working on (Marchh 2026)",
            "What are you working on (Mar  2026)",
            "What are you working on (Mar)",
            "",
        ] {
            assert_eq!(Month::from_title(title), None, "{title}");
        }
        // Sept is only September.
        assert_eq!(
            Month::from_title("What are you working on (Sept 2026)").map(|m| m.month),
            Some(9)
        );
    }

    #[test]
    fn month_arithmetic() {
        let m = |s: &str| Month::parse(s).unwrap();
        assert_eq!(m("2026-12").next(), Some(m("2027-01")));
        assert_eq!(m("2026-03").next(), Some(m("2026-04")));
        assert_eq!(m("2026-01").previous(), Some(m("2025-12")));
        assert_eq!(Month::at(0), Some(m("1970-01")));
        assert_eq!(Month::at(1_794_700_800), Some(m("2026-11")));
        assert_eq!(Month::at(951_782_400), Some(m("2000-02")));
        assert_eq!(Month::at(-1), Some(m("1969-12")));
        assert!(Month::current().unwrap() > m("2026-01"));
    }
}
