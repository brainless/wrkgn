//! Monthly on the shared run driver, offline: fake HN, discovery and model.
mod common;

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;
use wrkgn::collector::{CollectionConfig, Collector};
use wrkgn::extraction::{Extraction, Mention};
use wrkgn::monthly::{self, Args};
use wrkgn::processing::Model;
use wrkgn::run::driver::{DriverResult, unit_states};
use wrkgn::run::manage;
use wrkgn::run::status::StatusLine;
use wrkgn::run::{self, RunControl, RunSession};
use wrkgn::store::{QueueKind, Store, UnitState};

use common::{FakeClock, FakeDiscovery, FakeHn, ScriptModel, fresh_store, titled_thread};

/// 2026-11-15 UTC.
const NOW: i64 = 1_794_700_800;
const RATE_LIMIT: &str = "ProviderResponseError: status 429 Too Many Requests";

fn title(month: &str) -> String {
    let names = [
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
    let (year, number) = month.split_once('-').unwrap();
    format!(
        "What are you working on ({} {year})",
        names[number.parse::<usize>().unwrap() - 1]
    )
}

struct Env {
    _dir: TempDir,
    db: PathBuf,
    export: PathBuf,
    store: Store,
    hn: FakeHn,
    disc: FakeDiscovery,
    model: ScriptModel,
    clock: FakeClock,
}

impl Env {
    fn new() -> Self {
        let (dir, db, store) = fresh_store();
        let export = dir.path().join("export");
        Self {
            _dir: dir,
            db,
            export,
            store,
            hn: FakeHn::default(),
            disc: FakeDiscovery::default(),
            model: ScriptModel::default(),
            clock: FakeClock::new(),
        }
    }

    /// A month with these thread roots, each with one comment (root + 1).
    fn month(&mut self, month: &str, roots: &[i64]) {
        self.month_with_kids(month, &roots.iter().map(|r| (*r, 1)).collect::<Vec<_>>());
    }

    fn month_with_kids(&mut self, month: &str, roots: &[(i64, i64)]) {
        for (root, kids) in roots {
            let kids: Vec<i64> = (1..=*kids).map(|k| root + k).collect();
            self.hn
                .items
                .extend(titled_thread(*root, &title(month), &kids));
        }
        self.disc.months.insert(
            month.to_owned(),
            Ok(roots.iter().map(|(root, _)| *root).collect()),
        );
    }

    fn args(&self, extra: &str) -> Args {
        let mut raw: Vec<String> = extra.split_whitespace().map(str::to_owned).collect();
        raw.extend([
            "--export".into(),
            self.export.display().to_string(),
            "--concurrency".into(),
            "1".into(),
        ]);
        let mut args = Args::parse(&raw).unwrap();
        args.options.max_attempts = 1;
        args.options.initial_backoff = Duration::ZERO;
        args.options.max_backoff = Duration::ZERO;
        args
    }

    fn run(&mut self, extra: &str) -> (DriverResult, String) {
        self.run_with(extra, |_| {})
    }

    fn run_with(
        &mut self,
        extra: &str,
        before: impl FnOnce(&RunSession),
    ) -> (DriverResult, String) {
        let mut err = Vec::new();
        let result = self.run_to(extra, before, &mut err);
        (result, String::from_utf8(err).unwrap())
    }

    fn run_to(
        &mut self,
        extra: &str,
        before: impl FnOnce(&RunSession),
        err: &mut impl Write,
    ) -> DriverResult {
        let args = self.args(extra);
        let session = run::begin(&mut self.store, &self.db, "monthly", NOW).unwrap();
        before(&session);
        let collector = Collector::new(
            &self.hn,
            CollectionConfig {
                max_concurrency: 1,
                max_attempts: 1,
                retry_base_delay: Duration::ZERO,
            },
        )
        .with_control(session.control.clone());
        let mut status = StatusLine::new(Vec::new(), false, &self.clock);
        monthly::execute_driver(
            &mut self.store,
            &session,
            &collector,
            &self.model,
            &self.disc,
            &args,
            &|| NOW,
            &mut status,
            err,
        )
    }

    fn searches(&self) -> usize {
        self.disc.search_calls.load(Ordering::SeqCst)
    }

    fn states(&self) -> HashMap<String, (UnitState, u32)> {
        unit_states(&self.store, "monthly")
            .unwrap()
            .into_iter()
            .collect()
    }

    fn state(&self, month: &str) -> Option<(UnitState, u32)> {
        self.states().get(month).copied()
    }

    fn fetch_log(&mut self) -> Arc<Mutex<Vec<i64>>> {
        let log = Arc::new(Mutex::new(Vec::new()));
        let writer = log.clone();
        self.hn.on_fetch = Some(Box::new(move |id| writer.lock().unwrap().push(id)));
        log
    }
}

fn done_months(env: &Env) -> Vec<String> {
    let mut months: Vec<String> = env
        .states()
        .into_iter()
        .filter(|(_, (state, _))| *state == UnitState::Done)
        .map(|(month, _)| month)
        .collect();
    months.sort();
    months
}

#[test]
fn empty_month_is_done_and_not_searched_again() {
    let mut env = Env::new();
    // Defaults: head 2026-11 and 2026-10 (empty), backfill 2026-09 (empty).
    let (result, report) = env.run("");
    assert_eq!(env.searches(), 3);
    assert_eq!(done_months(&env), ["2026-09", "2026-10", "2026-11"]);
    assert_eq!(result.exit_code, 0, "{report}");
    assert!(
        report.contains("backfill up to 1 more month(s) starting at 2026-08"),
        "{report}"
    );
    assert_eq!(env.state("2026-09").unwrap().1, 0);

    // The next run searches the head again and one new backfill month, but not
    // the empty month it already covered.
    let (_, _) = env.run("");
    assert_eq!(env.searches(), 3 + 3);
    assert_eq!(done_months(&env).len(), 4);
    assert!(env.state("2026-08").is_some());
}

#[test]
fn head_runs_before_backfill_and_resume_runs_before_head() {
    let mut env = Env::new();
    env.month("2026-11", &[1100]);
    env.month("2026-10", &[1000]);
    env.month("2026-09", &[900]);
    env.month("2026-08", &[800]);
    env.month("2026-07", &[700]);
    // 2026-07 was left pending by an earlier run; its thread is queued.
    env.store
        .enqueue_items("monthly", QueueKind::Thread, "2026-07", &[700], NOW)
        .unwrap();
    let log = env.fetch_log();
    let (result, report) = env.run("");
    assert_eq!(result.exit_code, 0, "{report}");
    let order: Vec<i64> = log
        .lock()
        .unwrap()
        .iter()
        .copied()
        .filter(|id| id % 100 == 0)
        .collect();
    // Resume (2026-07), head newest first, then backfill (2026-09).
    assert_eq!(order, [700, 1100, 1000, 900]);
    // The queued month was not rediscovered.
    assert_eq!(env.searches(), 3);
    assert_eq!(
        done_months(&env),
        ["2026-07", "2026-09", "2026-10", "2026-11"]
    );
}

#[test]
fn head_threads_do_not_consume_the_cap_and_backfill_still_advances() {
    let mut env = Env::new();
    env.month("2026-11", &[1100, 1110]);
    env.month("2026-10", &[1000, 1010]);
    env.month("2026-09", &[900, 910, 920]);
    // Cap 1 is smaller than the four head threads.
    let log = env.fetch_log();
    let (result, report) = env.run("--max-items 1");
    assert_eq!(result.summary.items_processed, 5, "{report}");
    let fetched: Vec<i64> = log
        .lock()
        .unwrap()
        .iter()
        .copied()
        .filter(|id| id % 10 == 0)
        .collect();
    assert_eq!(fetched, [1100, 1110, 1000, 1010, 900]);
    assert_eq!(result.summary.outcome, run::Outcome::LimitReached);
    assert_eq!(result.exit_code, 0);
    // The month is cut short, not failed, and its attempt is given back.
    assert_eq!(env.state("2026-09"), Some((UnitState::InProgress, 0)));
}

#[test]
fn head_window_larger_than_the_cap_lets_backfill_advance_each_run() {
    let mut env = Env::new();
    env.month("2026-11", &[1100, 1110, 1120]);
    env.month("2026-10", &[1000, 1010, 1020]);
    env.month("2026-09", &[900, 910, 920]);
    // Six head threads, cap two.
    let (first, _) = env.run("--max-items 2");
    assert_eq!(first.summary.items_processed, 8);
    assert_eq!(env.queue_done("2026-09"), 2);
    let (second, report) = env.run("--max-items 2");
    // Resume finishes the last 2026-09 thread, then the cap allows one more.
    assert_eq!(env.state("2026-09").unwrap().0, UnitState::Done, "{report}");
    assert_eq!(second.summary.items_processed, 6 + 1);
}

impl Env {
    fn queue_done(&self, month: &str) -> usize {
        self.store.queue_counts("monthly", month).unwrap().done
    }
}

#[test]
fn backfill_takes_newest_uncovered_first_and_fills_gaps() {
    let mut env = Env::new();
    for month in ["2026-09", "2026-07"] {
        env.store
            .upsert_unit("monthly", month, UnitState::Done, NOW)
            .unwrap();
    }
    env.month("2026-10", &[1000]);
    env.month("2026-08", &[800]);
    env.month("2026-06", &[600]);
    env.month("2026-11", &[1100]);
    let (result, report) = env.run("--months 1 --backfill-months 2");
    assert_eq!(result.exit_code, 0, "{report}");
    // Head 2026-11, then the gaps 2026-10 and 2026-08; 2026-06 waits.
    assert_eq!(
        done_months(&env),
        ["2026-07", "2026-08", "2026-09", "2026-10", "2026-11"]
    );
    assert!(env.state("2026-06").is_none());
    assert_eq!(result.summary.outcome, run::Outcome::LimitReached);
    assert_eq!(result.summary.limit.cap, None);
    assert_eq!(
        result.summary.limit.backfill_next.as_deref(),
        Some("2026-06")
    );
    assert!(
        report.contains("(more backfill remains (next: 2026-06))"),
        "{report}"
    );
    let (_, _) = env.run("--months 1 --backfill-months 2");
    assert!(env.state("2026-06").is_some());
}

#[test]
fn cap_stops_at_an_item_boundary_and_the_next_run_resumes_without_rediscovery() {
    let mut env = Env::new();
    env.month("2026-10", &[1000, 1010, 1020]);
    env.month("2026-09", &[900]);
    let log = env.fetch_log();
    let (first, report) = env.run("--months 1 --max-items 2");
    assert_eq!(
        first.summary.outcome,
        run::Outcome::LimitReached,
        "{report}"
    );
    assert_eq!(first.summary.items_processed, 2);
    assert_eq!(first.summary.limit.cap, Some((2, 2, "thread".to_owned())));
    assert!(report.contains("thread cap reached (2/2)"), "{report}");
    // The head (2026-11, empty) and the backfill month 2026-10 were searched.
    assert_eq!(env.searches(), 2);
    assert_eq!(env.state("2026-10"), Some((UnitState::InProgress, 0)));
    assert!(report.contains("2026-10 (1 thread(s) left)"), "{report}");
    assert!(
        report.contains("finish 1 pending month(s) first"),
        "{report}"
    );
    assert!(!log.lock().unwrap().contains(&1020));

    let (second, report) = env.run("--months 1 --max-items 2");
    assert_eq!(second.exit_code, 0, "{report}");
    // The head and the next backfill month 2026-09 were searched; 2026-10 was
    // resumed from its queue.
    assert_eq!(env.searches(), 2 + 2);
    assert_eq!(env.state("2026-10"), Some((UnitState::Done, 0)));
    assert_eq!(env.state("2026-09"), Some((UnitState::Done, 0)));
    assert!(log.lock().unwrap().contains(&1020));
    assert_eq!(second.summary.items_processed, 2);
}

#[test]
fn failed_month_is_retried_up_to_the_cap_then_left_for_manual_action() {
    let mut env = Env::new();
    env.month("2026-09", &[900]);
    // Every extraction fails (not transient, so the breaker stays quiet).
    env.model = ScriptModel::failing("model returned an unusable answer");
    for attempt in 1..=3u32 {
        let (result, report) = env.run("");
        assert_eq!(result.exit_code, 1, "{report}");
        assert_eq!(
            env.state("2026-09"),
            Some((UnitState::Failed, attempt)),
            "{report}"
        );
        assert_eq!(env.hn.calls(900), attempt as usize);
        assert!(report.contains("extract"), "{report}");
    }
    let (_, report) = env.run("");
    assert!(
        report.contains("Needs manual action (attempts exhausted):\n  - 2026-09: thread 900: "),
        "{report}"
    );
    // The fourth run did not touch the exhausted month.
    assert_eq!(env.hn.calls(900), 3);
}

#[test]
fn discovery_failure_skips_only_that_month() {
    let mut env = Env::new();
    env.month("2026-10", &[1000]);
    env.disc
        .months
        .insert("2026-11".into(), Err("search unavailable".into()));
    let (result, report) = env.run("--backfill-months 1");
    assert_eq!(result.exit_code, 1, "{report}");
    assert_eq!(env.state("2026-11"), Some((UnitState::Failed, 1)));
    assert_eq!(env.state("2026-10"), Some((UnitState::Done, 0)));
    assert!(env.state("2026-09").is_some());
    assert!(report.contains("search unavailable"), "{report}");
    assert!(report.contains("discovery"), "{report}");
    assert!(
        report.contains("Failed units (retry next run): 2026-11 (1/3 attempts)"),
        "{report}"
    );
}

#[test]
fn override_marks_coverage_bypasses_head_and_rediscovers_done_months() {
    let mut env = Env::new();
    env.month("2026-05", &[500]);
    env.month("2026-04", &[400]);
    env.month("2026-11", &[1100]);
    let (result, report) = env.run("--from 2026-05 --through 2026-04");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(env.searches(), 2);
    assert_eq!(done_months(&env), ["2026-04", "2026-05"]);
    assert_eq!(env.hn.calls(1100), 0);

    // Done months are rediscovered and walked again on request.
    let (_, _) = env.run("--from 2026-05 --through 2026-05");
    assert_eq!(env.searches(), 3);
    assert!(env.hn.calls(500) >= 2);
    assert_eq!(env.state("2026-05"), Some((UnitState::Done, 0)));
    // A default run afterwards treats them as covered.
    let (_, _) = env.run("--months 1 --backfill-months 1");
    assert!(env.state("2026-11").is_some());
    assert_eq!(env.hn.calls(400), 1);
}

#[test]
fn existing_threads_are_seeded_without_search_or_calls() {
    let mut env = Env::new();
    env.month("2026-03", &[300]);
    env.month("2026-02", &[200]);
    // Legacy runs through the old per-thread API: 300 completes, 200 does not.
    let hn_options = wrkgn::processing::Options {
        concurrency: 1,
        ..Default::default()
    };
    let collector = Collector::new(&env.hn, CollectionConfig::default());
    monthly::sync_thread(
        &mut env.store,
        &collector,
        &env.model,
        300,
        &hn_options,
        &env.export,
        &mut Vec::new(),
    )
    .unwrap();
    let failing = ScriptModel::failing("legacy extraction failure");
    monthly::sync_thread(
        &mut env.store,
        &collector,
        &failing,
        200,
        &hn_options,
        &env.export,
        &mut Vec::new(),
    )
    .unwrap_err();
    drop(collector);
    let before = env.hn.total_calls();

    // Head only 2026-11; backfill budget goes to 2026-10 (empty).
    let (result, report) = env.run("--months 1 --backfill-months 1");
    assert_eq!(result.exit_code, 0, "{report}");
    assert!(
        report.contains("seeded coverage for 2 month(s)"),
        "{report}"
    );
    // 2026-03 is covered by the seed and never searched; 2026-02 resumed from
    // its queue row (no search) and processed once more.
    assert_eq!(env.state("2026-03"), Some((UnitState::Done, 0)));
    assert_eq!(env.state("2026-02"), Some((UnitState::Done, 0)));
    // Thread 300 was fetched once by the legacy run and never again.
    assert_eq!(env.hn.calls(300), 1);
    assert!(env.hn.total_calls() > before);
    // Searches: head 2026-11 and backfill 2026-10 only.
    assert_eq!(env.searches(), 2);
    assert!(env.hn.calls(200) >= 2);
}

#[test]
fn seeding_marks_months_and_notes_unparsable_titles() {
    let mut env = Env::new();
    env.month("2026-03", &[300]);
    let collector = Collector::new(&env.hn, CollectionConfig::default());
    let opts = wrkgn::processing::Options {
        concurrency: 1,
        ..Default::default()
    };
    monthly::sync_thread(
        &mut env.store,
        &collector,
        &env.model,
        300,
        &opts,
        &env.export,
        &mut Vec::new(),
    )
    .unwrap();
    // A thread whose title names no month.
    env.hn
        .items
        .extend(titled_thread(777, "Ask HN: Anything?", &[778]));
    let collector = Collector::new(&env.hn, CollectionConfig::default());
    monthly::sync_thread(
        &mut env.store,
        &collector,
        &env.model,
        777,
        &opts,
        &env.export,
        &mut Vec::new(),
    )
    .unwrap();
    drop(collector);
    let (_, report) = env.run("--months 1");
    assert_eq!(env.state("2026-03"), Some((UnitState::Done, 0)));
    assert_eq!(
        env.store.queue_counts("monthly", "2026-03").unwrap().done,
        1
    );
    // A second run seeds nothing new.
    let (_, _) = env.run("--months 1");
    assert_eq!(
        env.store
            .queue_counts("monthly", "2026-03")
            .unwrap()
            .total(),
        1
    );
    let _ = report;
}

#[test]
fn simulated_kill_mid_unit_resumes_without_rediscovery() {
    let mut env = Env::new();
    env.month("2026-10", &[1000, 1010]);
    // Head is 2026-11 only, so 2026-10 is the backfill unit. Build exactly the
    // state a killed process leaves behind (the test toolchain cannot unwind
    // panics, so the kill is not simulated by a panic): the run is still
    // `running`, the unit is in progress on attempt 1, the first thread is done
    // and the second is queued.
    let killed = run::begin(&mut env.store, &env.db, "monthly", NOW).unwrap();
    let (killed_id, searches) = (killed.run_id, env.searches());
    env.store
        .begin_unit_attempt("monthly", "2026-10", killed_id, NOW)
        .unwrap();
    env.store
        .enqueue_items("monthly", QueueKind::Thread, "2026-10", &[1000, 1010], NOW)
        .unwrap();
    env.store
        .finish_item(&wrkgn::store::ItemUpdate {
            command: "monthly",
            kind: QueueKind::Thread,
            item_id: 1000,
            state: wrkgn::store::ItemState::Done,
            run_id: Some(killed_id),
            error: None,
            now: NOW,
        })
        .unwrap();
    // The process died: the lock file stays behind with a dead PID.
    let lock_path = run::lock::lock_path(&env.db, "monthly");
    std::mem::forget(killed);
    let dead = std::process::Command::new("true")
        .spawn()
        .and_then(|mut c| {
            let pid = c.id();
            c.wait().map(|_| pid)
        });
    std::fs::write(&lock_path, dead.unwrap().to_string()).unwrap();
    let log = env.fetch_log();

    let (result, report) = env.run("--months 1");
    assert_eq!(result.exit_code, 0, "{report}");
    assert!(report.contains("was found killed"), "{report}");
    assert_eq!(result.summary.killed_runs, [killed_id]);
    assert_eq!(env.state("2026-10"), Some((UnitState::Done, 0)));
    // Only the head (2026-11) and the next backfill month (2026-09) were
    // searched; 2026-10 was not rediscovered and 1000 was not fetched again.
    assert_eq!(env.searches(), searches + 2);
    let fetched = log.lock().unwrap().clone();
    assert_eq!(
        fetched.iter().filter(|id| **id == 1000).count(),
        0,
        "{fetched:?}"
    );
    assert_eq!(fetched.iter().filter(|id| **id == 1010).count(), 1);
    let run = env.store.driver_run(killed_id).unwrap().unwrap();
    assert_eq!(run.status, "killed_detected");
}

#[test]
fn breaker_stops_the_run_cleanly_and_exports() {
    let mut env = Env::new();
    env.month_with_kids("2026-11", &[(1100, 6)]);
    env.month("2026-10", &[1000]);
    env.hn.transient.extend(1101..=1106);
    let (result, report) = env.run("");
    assert_eq!(result.exit_code, 2, "{report}");
    assert_eq!(result.summary.outcome, run::Outcome::StoppedOnFailure);
    assert!(report.contains("HN Firebase API"), "{report}");
    assert!(report.contains("hn_fetch"), "{report}");
    assert!(env.export.join("current.json").exists());
    assert_eq!(env.hn.calls(1000), 0, "later work is not started");
    // The thread whose fetches tripped the breaker failed; nothing later ran.
    assert_eq!(env.state("2026-11"), Some((UnitState::Failed, 1)));
    let run = env
        .store
        .driver_run(result.summary.run_id)
        .unwrap()
        .unwrap();
    assert_eq!(run.status, "stopped_on_failure");
    // The stop is a flag; no placeholder failure row is stored.
    let collection = env.store.latest_run_for_thread(1100).unwrap().unwrap();
    assert!(
        env.store
            .failures_for_run(collection.id)
            .unwrap()
            .iter()
            .all(|f| !f.detail.contains("stopped"))
    );
}

#[test]
fn llm_breaker_stops_the_run_with_exit_two() {
    let mut env = Env::new();
    env.month_with_kids("2026-11", &[(1100, 4)]);
    env.month("2026-10", &[1000]);
    env.model = ScriptModel::failing(RATE_LIMIT);
    let (result, report) = env.run("");
    assert_eq!(result.exit_code, 2, "{report}");
    assert!(report.contains("LLM provider"), "{report}");
    assert_eq!(env.hn.calls(1000), 0);
    assert!(env.export.join("current.json").exists());
}

#[test]
fn interrupt_stops_at_an_item_boundary_exports_and_resumes() {
    let mut env = Env::new();
    env.month_with_kids("2026-10", &[(1000, 2), (1010, 1)]);
    let control: Arc<OnceLock<Arc<RunControl>>> = Arc::new(OnceLock::new());
    let hook = control.clone();
    env.hn.on_fetch = Some(Box::new(move |id| {
        if id == 1000
            && let Some(control) = hook.get()
        {
            control.interrupt();
        }
    }));
    let (result, report) = env.run_with("--months 1", |session| {
        control.set(session.control.clone()).unwrap();
    });
    assert_eq!(result.exit_code, 130, "{report}");
    assert_eq!(result.summary.outcome, run::Outcome::Interrupted);
    assert!(env.export.join("current.json").exists());
    assert_eq!(env.hn.calls(1010), 0);
    let run = env
        .store
        .driver_run(result.summary.run_id)
        .unwrap()
        .unwrap();
    assert_eq!(run.status, "interrupted");
    assert_eq!(env.state("2026-10"), Some((UnitState::InProgress, 0)));
    assert!(report.contains("continue the interrupted unit"), "{report}");
    assert!(!report.contains("Errors (1"), "{report}");

    env.hn.on_fetch = None;
    let (result, report) = env.run("--months 1");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(env.state("2026-10"), Some((UnitState::Done, 0)));
}

#[test]
fn report_lists_work_and_what_remains() {
    let mut env = Env::new();
    env.month("2026-10", &[1000]);
    let (result, report) = env.run("");
    assert_eq!(result.exit_code, 0, "{report}");
    for needle in [
        "# monthly run",
        "## Errors (0 groups)",
        "## Work done",
        "Units covered: 3 (",
        "Items collected: 1, processed: 1, approved: 0",
        "## What remains",
        "Next run will: refresh the head (2026-10 to 2026-11) with full walks",
        "backfill up to 1 more month(s) starting at 2026-08",
    ] {
        assert!(report.contains(needle), "{needle}\n{report}");
    }
    assert!(!report.contains("api_key"));
}

#[test]
fn override_processes_failing_threads_and_continues_past_failed_discovery() {
    let mut env = Env::new();
    // Thread 200 is missing from HN (null); discovery for 2025-12 fails.
    env.month("2026-01", &[200, 100]);
    env.month("2025-11", &[300]);
    env.disc.months.insert(
        "2025-12".into(),
        Err("fixture discovery unavailable".into()),
    );
    env.hn.null.insert(200);
    let log = env.fetch_log();
    let (result, report) = env.run("--from 2026-01 --through 2025-11");
    assert_eq!(result.exit_code, 1, "{report}");
    let roots: Vec<i64> = log
        .lock()
        .unwrap()
        .iter()
        .copied()
        .filter(|id| id % 100 == 0)
        .collect();
    assert_eq!(roots, [200, 100, 300]);
    assert_eq!(env.searches(), 3);
    assert_eq!(env.state("2026-01").map(|s| s.0), Some(UnitState::Failed));
    assert_eq!(env.state("2025-12").map(|s| s.0), Some(UnitState::Failed));
    assert_eq!(env.state("2025-11"), Some((UnitState::Done, 0)));
    assert!(report.contains("fixture discovery unavailable"), "{report}");
    assert!(report.contains("collect"), "{report}");
    // Every thread that completed was exported; the failed one withheld nothing new.
    assert!(env.export.join("current.json").exists());
}

struct Projects {
    fail: AtomicBool,
    calls: Mutex<Vec<i64>>,
}

impl Model for Projects {
    fn name(&self) -> &str {
        "continue-fixture"
    }

    fn extract(&self, prompt: &str) -> Result<Extraction, String> {
        let records: serde_json::Value =
            serde_json::from_str(prompt.strip_prefix("Source records (JSON data): ").unwrap())
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

#[test]
fn processing_failure_retains_pending_work_while_later_threads_publish_and_resume() {
    let (dir, db, mut store) = fresh_store();
    let export = dir.path().join("export");
    let mut hn = FakeHn::default();
    for (root, kids) in [(100, vec![101]), (200, vec![201, 202]), (300, vec![301])] {
        let mut items = titled_thread(root, "What are you working on (January 2026)", &kids);
        for kid in kids {
            items.insert(
                kid,
                json!({"id":kid,"parent":root,"by":"alice","time":kid,
                       "text":format!("Project {kid} https://example.test/p{kid}")}),
            );
        }
        hn.items.extend(items);
    }
    let disc = FakeDiscovery {
        months: HashMap::from([
            ("2026-01".to_owned(), Ok(vec![200, 100])),
            ("2025-12".to_owned(), Ok(vec![300])),
        ]),
        ..FakeDiscovery::default()
    };
    let model = Projects {
        fail: AtomicBool::new(true),
        calls: Mutex::new(vec![]),
    };
    let clock = FakeClock::new();
    let mut args = Args::parse(
        &[
            "--from",
            "2026-01",
            "--through",
            "2025-12",
            "--concurrency",
            "1",
            "--export",
            export.to_str().unwrap(),
        ]
        .map(String::from),
    )
    .unwrap();
    args.options.max_attempts = 1;
    let run_once = |store: &mut Store| {
        let session = run::begin(store, &db, "monthly", NOW).unwrap();
        let collector =
            Collector::new(&hn, CollectionConfig::default()).with_control(session.control.clone());
        let mut status = StatusLine::new(Vec::new(), false, &clock);
        let mut err = Vec::new();
        let result = monthly::execute_driver(
            store,
            &session,
            &collector,
            &model,
            &disc,
            &args,
            &|| NOW,
            &mut status,
            &mut err,
        );
        (result, String::from_utf8(err).unwrap())
    };
    let (result, report) = run_once(&mut store);
    assert_eq!(result.exit_code, 1, "{report}");
    assert!(report.contains("fixture extraction failure"), "{report}");
    assert!(report.contains("group 201"), "{report}");
    let pending = wrkgn::publication::pending_mentions(&mut store).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].source_item_id, 202);
    let approved: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM mentions WHERE review_state='approved'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(approved, 2);
    assert_eq!(*model.calls.lock().unwrap(), [201, 202, 101, 301]);
    assert!(export.join("current.json").exists());

    model.fail.store(false, Ordering::SeqCst);
    let (result, report) = run_once(&mut store);
    assert_eq!(result.exit_code, 0, "{report}");
    assert!(
        wrkgn::publication::pending_mentions(&mut store)
            .unwrap()
            .is_empty()
    );
    // Only the failed group is extracted again.
    assert_eq!(*model.calls.lock().unwrap(), [201, 202, 101, 301, 201]);
}

#[test]
fn report_write_failure_does_not_change_the_exit_code() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("closed stderr"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("closed stderr"))
        }
    }
    for failing in [false, true] {
        let mut env = Env::new();
        env.month("2026-11", &[1100]);
        if failing {
            env.model = ScriptModel::failing("model returned an unusable answer");
        }
        let result = env.run_to("", |_| {}, &mut Broken);
        assert_eq!(result.exit_code, u8::from(failing));
        assert!(
            env.store
                .driver_run(result.summary.run_id)
                .unwrap()
                .is_some()
        );
    }
}

/// Fails 2026-09 three times so it is exhausted, with the model then failing.
fn exhausted_env() -> Env {
    let mut env = Env::new();
    env.month("2026-09", &[900]);
    env.model = ScriptModel::failing("model returned an unusable answer");
    for _ in 0..3 {
        let (result, report) = env.run("");
        assert_eq!(result.exit_code, 1, "{report}");
    }
    assert_eq!(env.state("2026-09"), Some((UnitState::Failed, 3)));
    env
}

fn manage_now() -> i64 {
    NOW + 10
}

fn manual_collect(env: &mut Env, thread: i64) {
    let cfg = CollectionConfig {
        max_concurrency: 1,
        max_attempts: 1,
        retry_base_delay: Duration::ZERO,
    };
    let collector = Collector::new(&env.hn, cfg);
    assert!(collector.collect(&mut env.store, thread).unwrap().complete);
}

#[test]
fn acknowledged_unit_is_skipped_reported_and_exits_zero() {
    let mut env = exhausted_env();
    let message = manage::acknowledge(
        &mut env.store,
        &env.db,
        "monthly",
        "2026-09",
        Some("permanent validation failure"),
        manage_now(),
    )
    .unwrap();
    assert!(message.contains("Acknowledged 2026-09"), "{message}");
    let (result, report) = env.run("");
    assert_eq!(result.exit_code, 0, "{report}");
    assert!(!result.summary.has_failures());
    assert!(
        report.contains("- Acknowledged (skipped): 1 unit: 2026-09"),
        "{report}"
    );
    assert!(
        report.contains("- Needs manual action (attempts exhausted): None"),
        "{report}"
    );
    // Not retried, not covered: still failed with all its attempts.
    assert_eq!(env.hn.calls(900), 3);
    assert_eq!(env.state("2026-09"), Some((UnitState::Failed, 3)));
    assert!(!result.summary.units_done.contains(&"2026-09".to_owned()));
    let listing = manage::list(&env.store, "monthly").unwrap();
    assert!(listing.contains("2026-09  acknowledged"), "{listing}");
    assert!(listing.contains("note: permanent validation failure"));
}

#[test]
fn unacknowledge_restores_the_failure_without_retrying() {
    let mut env = exhausted_env();
    manage::acknowledge(
        &mut env.store,
        &env.db,
        "monthly",
        "2026-09",
        None,
        manage_now(),
    )
    .unwrap();
    manage::unacknowledge(&mut env.store, &env.db, "monthly", "2026-09").unwrap();
    assert!(
        manage::unacknowledge(&mut env.store, &env.db, "monthly", "2026-09")
            .unwrap_err()
            .contains("not acknowledged")
    );
    let (result, report) = env.run("");
    assert_eq!(result.exit_code, 1, "{report}");
    assert!(
        report.contains("Needs manual action (attempts exhausted):\n  - 2026-09: thread 900: "),
        "{report}"
    );
    assert!(!report.contains("Acknowledged (skipped)"), "{report}");
    assert_eq!(env.hn.calls(900), 3);
}

#[test]
fn retry_resets_attempts_and_the_next_run_processes_the_unit() {
    let mut env = exhausted_env();
    let message =
        manage::retry(&mut env.store, &env.db, "monthly", "2026-09", manage_now()).unwrap();
    assert!(message.contains("1 failed queue item(s)"), "{message}");
    assert_eq!(env.state("2026-09"), Some((UnitState::Pending, 0)));
    // The cause is fixed: the model now answers.
    env.model = ScriptModel::default();
    let (result, report) = env.run("");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(env.state("2026-09"), Some((UnitState::Done, 0)));
    assert_eq!(env.hn.calls(900), 4);
    assert!(result.summary.units_done.contains(&"2026-09".to_owned()));
}

#[test]
fn manual_collect_and_process_reopen_the_exhausted_unit_without_model_calls() {
    let mut env = exhausted_env();
    // The maintainer fixes thread 900 by hand with `collect` then `process`.
    manual_collect(&mut env, 900);
    let good = ScriptModel::default();
    let options = wrkgn::processing::Options {
        concurrency: 1,
        ..Default::default()
    };
    let processed =
        wrkgn::processing::process_thread_with(&mut env.store, 900, &good, false, &options, |_| {})
            .unwrap();
    assert!(processed.failed.is_empty());
    // Nothing in the driver's records changed yet.
    assert_eq!(env.state("2026-09"), Some((UnitState::Failed, 3)));
    // The next default run notices, re-collects, skips the extracted groups
    // (the model here would fail if called), approves and covers the month.
    env.model = ScriptModel::failing("must not be called");
    let (result, report) = env.run("");
    assert_eq!(result.exit_code, 0, "{report}");
    assert!(report.contains("reopened"), "{report}");
    assert_eq!(env.state("2026-09"), Some((UnitState::Done, 0)));
    assert_eq!(env.model.calls(), 0);
}

#[test]
fn a_complete_manual_process_after_the_failure_reopens_without_a_new_collect() {
    let mut env = exhausted_env();
    let good = ScriptModel::default();
    let options = wrkgn::processing::Options {
        concurrency: 1,
        ..Default::default()
    };
    wrkgn::processing::process_thread_with(&mut env.store, 900, &good, false, &options, |_| {})
        .unwrap();
    env.model = ScriptModel::failing("must not be called");
    let (result, report) = env.run("");
    assert_eq!(result.exit_code, 0, "{report}");
    assert!(report.contains("reopened"), "{report}");
    assert_eq!(env.state("2026-09"), Some((UnitState::Done, 0)));
}

#[test]
fn collect_alone_does_not_reopen_an_exhausted_unit() {
    let mut env = exhausted_env();
    manual_collect(&mut env, 900);
    let (result, report) = env.run("");
    assert_eq!(result.exit_code, 1, "{report}");
    assert_eq!(env.state("2026-09"), Some((UnitState::Failed, 3)));
    assert_eq!(env.hn.calls(900), 4, "only the manual collect fetched");
}

#[test]
fn manage_commands_validate_units_state_and_the_run_lock() {
    let mut env = exhausted_env();
    let now = manage_now();
    let db = env.db.clone();
    let store = &mut env.store;
    assert!(
        manage::acknowledge(store, &db, "monthly", "2026-9x", None, now)
            .unwrap_err()
            .contains("invalid unit")
    );
    assert!(
        manage::retry(store, &db, "monthly", "2025-01", now)
            .unwrap_err()
            .contains("unknown unit")
    );
    assert!(manage::command_name("weekly").is_err());
    assert_eq!(manage::command_name("show-hn").unwrap(), "show_hn");
    assert_eq!(
        manage::normalize_unit("show_hn", "2026-09-01").unwrap(),
        "2026-09-01"
    );
    assert!(manage::normalize_unit("show_hn", "2026-09").is_err());
    store
        .upsert_unit("monthly", "2026-07", UnitState::Done, NOW)
        .unwrap();
    assert!(
        manage::acknowledge(store, &db, "monthly", "2026-07", None, now)
            .unwrap_err()
            .contains("only failed units")
    );
    // A live run holds the lock, so changes are refused.
    let held = wrkgn::run::lock::acquire(&db, "monthly").unwrap();
    assert!(
        manage::retry(store, &db, "monthly", "2026-09", now)
            .unwrap_err()
            .contains("another run")
    );
    drop(held);
    assert!(manage::retry(store, &db, "monthly", "2026-09", now).is_ok());
}
