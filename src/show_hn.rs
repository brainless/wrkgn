//! Show HN on the shared run driver (`crate::run::driver`): UTC-day units,
//! `Batches` discovery within a day, a story queue and extraction in chunks.
use crate::collector::{Collector, HnApi};
use crate::processing::{self, Model};
use crate::publication;
use crate::run::driver::{
    self, ChunkCx, DriverConfig, DriverResult, ItemError, ItemOutcome, ProgressEvent, Workload,
};
use crate::run::status::{Clock, StatusLine};
use crate::run::{MAX_UNIT_ATTEMPTS, RunSession};
use crate::show_extraction;
use crate::show_hn_discovery::{
    Batches, DEFAULT_BATCH_SIZE, DateRange, Discovery, MAX_BATCH_SIZE, UtcDay, matches_title,
};
use crate::store::{ExtractionMode, QueueKind, Store};
use crate::workflow::{self, ApprovalPolicy, Event};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;

/// Recent days re-walked fully on every run (`--days`).
pub const DEFAULT_HEAD_DAYS: usize = 3;
/// Uncovered older days taken per run (`--backfill-days`).
pub const DEFAULT_BACKFILL_DAYS: usize = 3;
/// Stories collected and processed per run by resume and backfill together
/// (`--max-items`). Head stories do not count.
pub const DEFAULT_MAX_ITEMS: usize = 100;

pub const HELP: &str = "Usage: wrkgn-show-hn [--days N] [--backfill-days N] [--max-items N]
    [--batch-size N] [--auto-approve] [--reprocess] [--concurrency N] [--db PATH]
    [--export DIRECTORY]
       wrkgn-show-hn --from YYYY-MM-DD --through YYYY-MM-DD [--batch-size N] [--auto-approve]
    [--reprocess] [--concurrency N] [--db PATH] [--export DIRECTORY]
       wrkgn-show-hn --thread ID [--auto-approve] [--reprocess] [--concurrency N]
    [--db PATH] [--export DIRECTORY]

With no --thread or range, each run is small and resumable; run it daily or from
cron. Coverage is one UTC day at a time:
  1. Resume: finish days a previous run left pending, in progress or failed
     (stories already discovered are not searched for again).
  2. Head: re-walk today and the previous days (--days, default 3) completely;
     only changed stories are re-extracted. Head stories do not count against
     --max-items.
  3. Backfill: take the newest uncovered days older than the head
     (--backfill-days, default 3), newest first, back to 2007-02-01. Empty days
     count and are marked done.
--max-items (default 100) caps stories collected and processed by resume and
backfill together; the run stops at a story boundary and the next run continues.
A day cut short by the cap is resumed on the next run from its queued stories
without rediscovery; pass --reprocess again then if extraction should be forced
for the rest of it. A day is discovered in Algolia searches of --batch-size
stories (default 100, at most 1000) using a timestamp cursor; any discovery
failure fails that day, which is retried on later runs up to 3 attempts and then
listed for manual action (resolve with `wrkgn runs retry|acknowledge`). Coverage, the story queue and runs live in the SQLite
database; there are no log files. Days that already had Show HN data processed
before coverage existed are not recorded as covered, so backfill may collect
them again; unchanged extraction is skipped. Stories posted after a day leaves
the head window (late comments, edits, deletions or Algolia indexing) are picked
up only by --from/--through over that day or by --thread.

--from/--through (both required, inclusive UTC dates) process exactly those days,
newest first, bypassing head, backfill and limits and re-walking days already done;
each day costs at least one Algolia request. They cannot be combined with --days,
--backfill-days, --max-items or --thread. --thread ID collects and processes that
one story (unqualified or monthly-assigned items fail), changes no day coverage,
and exits 1 if it fails; it cannot be combined with --batch-size, --days,
--backfill-days or --max-items.

Results await manual review unless --auto-approve is supplied (it authorizes
story-scoped approval for each successfully processed story). --reprocess
forces extraction again. Each chunk of up to --batch-size stories (collected one
by one, then extracted with up to --concurrency parallel model calls) is
exported once. A failed story is reported with its full message and the run
continues. A live status line and an end-of-run Markdown report go to stderr.
The header says which limit applied (story cap, more backfill remaining, or
both). Exit code 0: completed or limit reached without failures; 1: failures
(including units still needing manual action from earlier runs; the header
counts them) or a setup error; 2: stopped after 3 consecutive failed calls to one API (LLM provider, HN,
Algolia); 130: interrupted (Ctrl-C or SIGTERM stops at the next story, exports,
and reports; a second Ctrl-C exits at once). A second instance exits immediately
while one is running, so overlapping cron runs are safe.

Cron example (cron starts in the home directory; .env and the default
local-data/ paths are relative to the working directory). The report goes to
stderr and arrives as cron mail; add --auto-approve only to publish without review:
  37 3 * * * cd /path/to/wrkgn && ./target/release/wrkgn-show-hn
Uses the same .env, provider, database and export settings as wrkgn. Publication to the server remains explicit.";

#[derive(Clone)]
pub struct Args {
    /// `--thread`: one explicit story, no day coverage.
    pub thread: Option<i64>,
    /// `--from/--through`, inclusive and ordered `(from, through)`.
    pub range: Option<(UtcDay, UtcDay)>,
    pub head_days: usize,
    pub backfill_days: usize,
    pub max_items: usize,
    pub db: Option<PathBuf>,
    pub export: PathBuf,
    pub options: processing::Options,
    pub reprocess: bool,
    pub approval: ApprovalPolicy,
    pub batch_size: usize,
}

impl Args {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut thread = None;
        let mut from = None;
        let mut through = None;
        let mut db = None;
        let mut export = std::env::var_os("WRKGN_EXPORT_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("local-data/public-export"));
        let mut options = processing::Options::default();
        let mut reprocess = false;
        let mut approval = ApprovalPolicy::Manual;
        let mut batch_size = None;
        let mut head_days = DEFAULT_HEAD_DAYS;
        let mut backfill_days = DEFAULT_BACKFILL_DAYS;
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
            match flag.as_str() {
                "--auto-approve" | "--reprocess" => {
                    if !seen.insert(flag) {
                        return Err(format!("duplicate argument {flag}"));
                    }
                    if flag == "--auto-approve" {
                        approval = ApprovalPolicy::ApproveThread;
                    } else {
                        reprocess = true;
                    }
                    continue;
                }
                "--thread" | "--from" | "--through" | "--db" | "--export" | "--concurrency"
                | "--batch-size" | "--days" | "--backfill-days" | "--max-items" => {}
                _ => return Err(format!("unknown argument {flag}\n{HELP}")),
            }
            if !seen.insert(flag) {
                return Err(format!("duplicate argument {flag}"));
            }
            let value = args
                .next()
                .filter(|v| !v.starts_with("--"))
                .ok_or_else(|| format!("{flag} requires a value"))?;
            match flag.as_str() {
                "--thread" => {
                    thread = Some(
                        value
                            .parse::<i64>()
                            .ok()
                            .filter(|id| *id > 0)
                            .ok_or("--thread requires a positive item ID")?,
                    )
                }
                "--from" => from = Some(UtcDay::parse(value)?),
                "--through" => through = Some(UtcDay::parse(value)?),
                "--db" => db = Some(PathBuf::from(value)),
                "--export" => export = PathBuf::from(value),
                "--concurrency" => options.concurrency = positive(flag, value)?,
                "--batch-size" => {
                    batch_size = Some(
                        value
                            .parse::<usize>()
                            .ok()
                            .filter(|n| (1..=MAX_BATCH_SIZE).contains(n))
                            .ok_or_else(|| {
                                format!(
                                    "--batch-size requires an integer from 1 to {MAX_BATCH_SIZE}"
                                )
                            })?,
                    )
                }
                "--days" => {
                    head_days = positive(flag, value)?;
                    limit_flags = true;
                }
                "--backfill-days" => {
                    backfill_days = positive(flag, value)?;
                    limit_flags = true;
                }
                "--max-items" => {
                    max_items = positive(flag, value)?;
                    limit_flags = true;
                }
                _ => unreachable!(),
            }
        }
        if thread.is_some() && batch_size.is_some() {
            return Err("--batch-size applies only to default runs and --from/--through".into());
        }
        if thread.is_some() && (from.is_some() || through.is_some()) {
            return Err("--thread cannot be combined with --from/--through".into());
        }
        let range = match (from, through) {
            (None, None) => None,
            (Some(from), Some(through)) => {
                if from > through {
                    return Err("--from must be the same date as --through or earlier".into());
                }
                Some((from, through))
            }
            _ => return Err("--from and --through must be supplied together".into()),
        };
        if (thread.is_some() || range.is_some()) && limit_flags {
            return Err(
                "--thread and --from/--through cannot be combined with --days, --backfill-days or --max-items"
                    .into(),
            );
        }
        Ok(Self {
            thread,
            range,
            head_days,
            backfill_days,
            max_items,
            db,
            export,
            options,
            reprocess,
            approval,
            batch_size: batch_size.unwrap_or(DEFAULT_BATCH_SIZE),
        })
    }

    /// The days an override processes, newest first.
    fn override_days(&self) -> Option<Vec<UtcDay>> {
        let (from, through) = self.range?;
        let mut days = vec![through];
        let mut day = through;
        while day > from {
            day = day.previous()?;
            days.push(day);
        }
        Some(days)
    }
}

/// Decides after collection whether the story may be extracted and assigns its mode.
fn qualify(
    store: &mut Store,
    thread: i64,
    range: Option<DateRange>,
    direct: bool,
) -> Result<bool, String> {
    let item = store
        .get_item(thread)
        .map_err(|e| e.to_string())?
        .ok_or("collected story is missing")?;
    let source = &item.source;
    if source.deleted || source.dead {
        return Ok(false);
    }
    let qualifies = source.raw_payload.get("type").and_then(|v| v.as_str()) == Some("story")
        && source
            .raw_payload
            .get("title")
            .and_then(|v| v.as_str())
            .is_some_and(matches_title)
        && source
            .author
            .as_deref()
            .is_some_and(|a| !a.trim().is_empty())
        && source
            .created_at
            .is_some_and(|date| range.is_none_or(|r| r.contains(date)));
    if !qualifies {
        if direct {
            return Err(format!(
                "item {thread} must be a live Show HN: story with usable author and date"
            ));
        }
        return Ok(false);
    }
    store.assign_extraction_mode(thread, ExtractionMode::ShowHn)?;
    Ok(true)
}

fn monthly_conflict(thread: i64) -> String {
    format!("thread {thread} is assigned to monthly extraction; Show HN workflow cannot process it")
}

/// Show HN stories on the shared run driver.
pub struct ShowHnWorkload<'a, C, D, M> {
    collector: &'a Collector<C>,
    model: &'a M,
    discovery: &'a D,
    options: processing::Options,
    export: PathBuf,
    reprocess: bool,
    approval: ApprovalPolicy,
    batch_size: usize,
}

fn qualify_error(message: String) -> ItemOutcome {
    let kind = if message.contains("must be a live Show HN") {
        "not_show_hn"
    } else if message.contains("assigned to") {
        "mode_conflict"
    } else {
        "store"
    };
    ItemOutcome::Failed {
        errors: vec![ItemError {
            stage: "qualify",
            kind: kind.into(),
            message,
        }],
    }
}

impl<C: HnApi, D: Discovery, M: Model + Sync> Workload for ShowHnWorkload<'_, C, D, M> {
    type Unit = UtcDay;
    const COMMAND: &'static str = "show_hn";
    const ITEM_KIND: QueueKind = QueueKind::Story;
    const ITEM_LABEL: &'static str = "story";
    const UNIT_LABEL: &'static str = "day";

    fn parse_unit(&self, key: &str) -> Option<UtcDay> {
        UtcDay::parse(key).ok()
    }

    fn head_units(&self, now: i64, n: usize) -> Vec<UtcDay> {
        let mut units = Vec::new();
        let mut day = UtcDay::at(now);
        while let Some(current) = day {
            if units.len() >= n || current < UtcDay::FLOOR {
                break;
            }
            units.push(current);
            day = current.previous();
        }
        units
    }

    fn floor(&self) -> UtcDay {
        UtcDay::FLOOR
    }

    fn previous(&self, unit: UtcDay) -> Option<UtcDay> {
        unit.previous()
    }

    /// Every search batch of the day; any failure fails the whole day.
    fn discover(&self, unit: UtcDay) -> Result<Vec<i64>, String> {
        let mut batches = Batches::new(self.discovery, unit.range(), self.batch_size)?;
        let mut ids = Vec::new();
        while !batches.finished() {
            match batches.next_batch()? {
                Some(stories) => ids.extend(stories.iter().map(|story| story.id)),
                None => break,
            }
        }
        Ok(ids)
    }

    fn chunk_size(&self) -> usize {
        self.batch_size
    }

    fn process_chunk(
        &self,
        cx: &mut ChunkCx<'_>,
        unit: Option<UtcDay>,
        ids: &[i64],
    ) -> Vec<(i64, ItemOutcome)> {
        let mut options = self.options.clone();
        options.control = Some(cx.control.clone());
        let range: Option<DateRange> = unit.map(UtcDay::range);
        let direct = unit.is_none();
        let mut outcomes: Vec<(i64, ItemOutcome)> = Vec::new();
        let mut ready: Vec<i64> = Vec::new();
        let mut stopped = false;

        // Collect sequentially; extraction of the ready stories follows.
        for &story in ids {
            if cx.control.should_stop() {
                outcomes.push((story, ItemOutcome::Stopped));
                stopped = true;
                break;
            }
            (cx.progress)(ProgressEvent::Item(story));
            let mut first_failure = None;
            let mut prepare_failed = false;
            let result = match cx.store.extraction_mode(story) {
                Ok(Some(ExtractionMode::Monthly)) => {
                    outcomes.push((story, qualify_error(monthly_conflict(story))));
                    continue;
                }
                Err(error) => Err(workflow::Error::Operation(error.to_string())),
                Ok(_) => workflow::collect_prepare(
                    cx.store,
                    self.collector,
                    story,
                    |store, id| {
                        let result = qualify(store, id, range, direct);
                        prepare_failed = result.is_err();
                        result
                    },
                    |event| {
                        if let Event::Collected(collection) = event {
                            (cx.progress)(ProgressEvent::Collected);
                            first_failure = collection
                                .failures
                                .first()
                                .map(|f| format!("item {}: {}", f.item_id, f.detail));
                        }
                        Ok(())
                    },
                ),
            };
            let outcome = match result {
                Ok(true) => {
                    ready.push(story);
                    continue;
                }
                Ok(false) => ItemOutcome::Done { approved: 0 },
                Err(workflow::Error::Operation(message)) if prepare_failed => {
                    qualify_error(message)
                }
                Err(error) => {
                    driver::outcome_from_error(error, cx.control, cx.errors, story, first_failure)
                }
            };
            stopped = outcome == ItemOutcome::Stopped;
            outcomes.push((story, outcome));
            if stopped {
                break;
            }
        }

        if stopped || cx.control.should_stop() {
            // Collected but not extracted stories stay pending.
            outcomes.extend(ready.into_iter().map(|id| (id, ItemOutcome::Stopped)));
            return outcomes;
        }
        if ready.is_empty() {
            return outcomes;
        }

        let results = show_extraction::process_batch(
            cx.store,
            &ready,
            self.model,
            self.reprocess,
            &options,
            |progress| (cx.progress)(ProgressEvent::Extract(progress.done, progress.total)),
        );
        for (story, result) in ready.into_iter().zip(results) {
            let outcome = match result {
                Ok(report) => {
                    match workflow::conclude(cx.store, story, report, self.approval, None) {
                        Ok(workflow::Staged::Ready { approved }) => ItemOutcome::Done {
                            approved: approved.unwrap_or(0),
                        },
                        Ok(workflow::Staged::Withheld) => ItemOutcome::Done { approved: 0 },
                        Err(error) => {
                            driver::outcome_from_error(error, cx.control, cx.errors, story, None)
                        }
                    }
                }
                Err(message) => ItemOutcome::Failed {
                    errors: vec![ItemError {
                        stage: "process",
                        kind: processing::failure_kind(&message),
                        message,
                    }],
                },
            };
            outcomes.push((story, outcome));
        }
        outcomes
    }

    fn export(&self, store: &mut Store) -> Result<String, String> {
        publication::export(store, &self.export)
    }
}

/// Runs Show HN on the shared driver. The collector should carry
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
    let workload = ShowHnWorkload {
        collector,
        model,
        discovery,
        options: args.options.clone(),
        export: args.export.clone(),
        reprocess: args.reprocess,
        approval: args.approval,
        batch_size: args.batch_size,
    };
    let config = DriverConfig {
        head: args.head_days,
        backfill_units: args.backfill_days,
        max_items: args.max_items,
        max_attempts: MAX_UNIT_ATTEMPTS,
        range: args.override_days(),
        items: args.thread.map(|thread| vec![thread]),
    };
    driver::run(store, session, &workload, &config, now, status, err)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arguments_validate_before_work() {
        let args = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        for invalid in [
            "--thread 0",
            "--thread 1 --from 2026-11-01 --through 2026-11-02",
            "--from 2026-11-01",
            "--through 2026-11-01",
            "--from 2026-11-02 --through 2026-11-01",
            "--from 2026-02-30 --through 2026-03-01",
            "--thread 1 --concurrency 0",
            "--thread 1 --batch-size 10",
            "--from 2026-11-01 --through 2026-11-02 --batch-size 0",
            "--from 2026-11-01 --through 2026-11-02 --batch-size 1001",
            "--thread 1 --auto-approve --auto-approve",
            "--unknown",
            "--days 0",
            "--backfill-days x",
            "--max-items 0",
            "--max-items 1 --max-items 2",
            "--thread 1 --days 2",
            "--thread 1 --backfill-days 2",
            "--thread 1 --max-items 2",
            "--from 2026-11-01 --through 2026-11-02 --days 2",
            "--from 2026-11-01 --through 2026-11-02 --max-items 2",
        ] {
            assert!(Args::parse(&args(invalid)).is_err(), "{invalid}");
        }
        let defaults = Args::parse(&[]).unwrap();
        assert!(defaults.thread.is_none() && defaults.range.is_none());
        assert_eq!(
            (
                defaults.head_days,
                defaults.backfill_days,
                defaults.max_items,
                defaults.batch_size
            ),
            (3, 3, 100, 100)
        );
        assert_eq!(defaults.approval, ApprovalPolicy::Manual);
        assert!(!defaults.reprocess && defaults.override_days().is_none());
        let tuned = Args::parse(&args(
            "--days 4 --backfill-days 2 --max-items 9 --auto-approve --reprocess --batch-size 7",
        ))
        .unwrap();
        assert_eq!(
            (tuned.head_days, tuned.backfill_days, tuned.max_items),
            (4, 2, 9)
        );
        assert_eq!(tuned.approval, ApprovalPolicy::ApproveThread);
        assert!(tuned.reprocess);
        let thread = Args::parse(&args("--thread 5 --reprocess")).unwrap();
        assert_eq!(thread.thread, Some(5));
        let range = Args::parse(&args("--from 2026-11-01 --through 2026-11-03")).unwrap();
        let days: Vec<String> = range
            .override_days()
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(days, ["2026-11-03", "2026-11-02", "2026-11-01"]);
        let one = Args::parse(&args("--from 2026-11-01 --through 2026-11-01")).unwrap();
        assert_eq!(one.override_days().unwrap().len(), 1);
    }
}
