//! Show HN on the shared run driver, offline: fake HN, Algolia and model.
mod common;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;
use wrkgn::collector::{CollectionConfig, Collector};
use wrkgn::extraction::{Extraction, Mention};
use wrkgn::processing::Model;
use wrkgn::run::driver::{DriverResult, unit_states};
use wrkgn::run::status::StatusLine;
use wrkgn::run::{self, RunControl, RunSession};
use wrkgn::show_extraction::{ShowEvidence, ShowExtraction, ShowProject};
use wrkgn::show_hn::{self, Args};
use wrkgn::show_hn_discovery::UtcDay;
use wrkgn::store::{ExtractionMode, ItemState, ItemUpdate, QueueKind, Store, UnitState};

use common::{FakeClock, FakeDiscovery, FakeHn, fresh_store};

/// 2026-11-15 12:00 UTC.
const NOW: i64 = 1_794_700_800 + 43_200;

fn at(day: &str, secs: i64) -> i64 {
    UtcDay::parse(day).unwrap().range().start + secs
}

/// A model that extracts one project per story, recording the order.
#[derive(Default)]
struct ShowModel {
    fail: Mutex<HashMap<i64, String>>,
    order: Mutex<Vec<i64>>,
    calls: AtomicUsize,
}

impl Model for ShowModel {
    fn name(&self) -> &str {
        "show-fixture"
    }

    fn extract(&self, _: &str) -> Result<Extraction, String> {
        panic!("monthly extraction must not run")
    }

    fn extract_show(&self, prompt: &str) -> Result<ShowExtraction, String> {
        let story = serde_json::from_str::<serde_json::Value>(prompt).unwrap()["story_id"]
            .as_i64()
            .unwrap();
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.order.lock().unwrap().push(story);
        if let Some(message) = self.fail.lock().unwrap().get(&story) {
            return Err(message.clone());
        }
        let title = format!("Show HN: Story {story}");
        Ok(ShowExtraction {
            project: Some(ShowProject {
                mention: Mention {
                    source_item_id: story,
                    name: Some(format!("Project {story}")),
                    description: "Organizes work".into(),
                    urls: vec![],
                    problem: None,
                    audience: None,
                    source_excerpt: title.clone(),
                    builders: vec![],
                    categories: vec![],
                    audience_labels: vec![],
                },
                evidence: vec![ShowEvidence {
                    source_item_id: story,
                    source_field: "title".into(),
                    value: title,
                    supports: vec!["name".into(), "description".into()],
                    authorization_source_item_id: None,
                }],
            }),
            no_project_reason: None,
        })
    }
}

struct Env {
    _dir: TempDir,
    db: PathBuf,
    export: PathBuf,
    store: Store,
    hn: FakeHn,
    disc: FakeDiscovery,
    model: ShowModel,
    clock: FakeClock,
    concurrency: &'static str,
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
            model: ShowModel::default(),
            clock: FakeClock::new(),
            concurrency: "1",
        }
    }

    /// A Show HN story posted `secs` into the UTC day.
    fn story(&mut self, day: &str, id: i64, secs: i64) {
        self.story_titled(day, id, secs, &format!("Show HN: Story {id}"));
    }

    fn story_titled(&mut self, day: &str, id: i64, secs: i64, title: &str) {
        let time = at(day, secs);
        self.hn.items.insert(
            id,
            json!({"id": id, "type": "story", "by": "alice", "time": time,
                   "title": title, "url": "https://alpha.test", "kids": []}),
        );
        self.disc.stories.push((id, title.to_owned(), time));
    }

    fn args(&self, extra: &str) -> Args {
        let mut raw: Vec<String> = extra.split_whitespace().map(str::to_owned).collect();
        raw.extend([
            "--export".into(),
            self.export.display().to_string(),
            "--concurrency".into(),
            self.concurrency.into(),
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
        let args = self.args(extra);
        let session = run::begin(&mut self.store, &self.db, "show_hn", NOW).unwrap();
        before(&session);
        *self.disc.breaker.lock().unwrap() = Some(session.control.clone());
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
        let mut err = Vec::new();
        let result = show_hn::execute_driver(
            &mut self.store,
            &session,
            &collector,
            &self.model,
            &self.disc,
            &args,
            &|| NOW,
            &mut status,
            &mut err,
        );
        (result, String::from_utf8(err).unwrap())
    }

    fn searches(&self) -> usize {
        self.disc.search_calls.load(Ordering::SeqCst)
    }

    fn states(&self) -> HashMap<String, (UnitState, u32)> {
        unit_states(&self.store, "show_hn")
            .unwrap()
            .into_iter()
            .collect()
    }

    fn state(&self, day: &str) -> Option<(UnitState, u32)> {
        self.states().get(day).copied()
    }

    fn done_days(&self) -> Vec<String> {
        let mut days: Vec<String> = self
            .states()
            .into_iter()
            .filter(|(_, (state, _))| *state == UnitState::Done)
            .map(|(day, _)| day)
            .collect();
        days.sort();
        days
    }

    fn pending(&self, day: &str) -> usize {
        self.store.queue_counts("show_hn", day).unwrap().pending
    }

    fn fetch_log(&mut self) -> Arc<Mutex<Vec<i64>>> {
        let log = Arc::new(Mutex::new(Vec::new()));
        let writer = log.clone();
        self.hn.on_fetch = Some(Box::new(move |id| writer.lock().unwrap().push(id)));
        log
    }

    fn pending_mentions(&mut self) -> usize {
        wrkgn::publication::pending_mentions(&mut self.store)
            .unwrap()
            .len()
    }
}

#[test]
fn resume_after_simulated_kill_mid_day_does_not_rediscover() {
    let mut env = Env::new();
    env.story("2026-11-13", 130, 100);
    env.story("2026-11-13", 131, 200);
    // The test toolchain cannot unwind panics, so build the post-kill state:
    // the run is still `running`, the day is in progress on attempt 1, story
    // 130 is done and 131 is queued.
    let killed = run::begin(&mut env.store, &env.db, "show_hn", NOW).unwrap();
    let (killed_id, searches) = (killed.run_id, env.searches());
    env.store
        .begin_unit_attempt("show_hn", "2026-11-13", killed_id, NOW)
        .unwrap();
    env.store
        .enqueue_items("show_hn", QueueKind::Story, "2026-11-13", &[131, 130], NOW)
        .unwrap();
    env.store
        .finish_item(&ItemUpdate {
            command: "show_hn",
            kind: QueueKind::Story,
            item_id: 130,
            state: ItemState::Done,
            run_id: Some(killed_id),
            error: None,
            now: NOW,
        })
        .unwrap();
    let lock_path = run::lock::lock_path(&env.db, "show_hn");
    std::mem::forget(killed);
    let dead = std::process::Command::new("true")
        .spawn()
        .and_then(|mut c| {
            let pid = c.id();
            c.wait().map(|_| pid)
        });
    std::fs::write(&lock_path, dead.unwrap().to_string()).unwrap();
    let log = env.fetch_log();

    let (result, report) = env.run("--days 1 --backfill-days 1");
    assert_eq!(result.exit_code, 0, "{report}");
    assert!(report.contains("was found killed"), "{report}");
    assert_eq!(result.summary.killed_runs, [killed_id]);
    assert_eq!(env.state("2026-11-13"), Some((UnitState::Done, 0)));
    // Only the head (11-15) and the backfill day (11-14) were searched.
    assert_eq!(env.searches(), searches + 2);
    let fetched = log.lock().unwrap().clone();
    assert_eq!(fetched.iter().filter(|id| **id == 130).count(), 0);
    assert_eq!(fetched.iter().filter(|id| **id == 131).count(), 1);
    assert_eq!(
        env.store.driver_run(killed_id).unwrap().unwrap().status,
        "killed_detected"
    );
}

#[test]
fn resume_runs_before_head_before_backfill_and_head_is_three_days() {
    let mut env = Env::new();
    for (day, id) in [
        ("2026-11-15", 1500),
        ("2026-11-14", 1400),
        ("2026-11-13", 1300),
        ("2026-11-12", 1200),
        ("2026-11-11", 1100),
        ("2026-11-10", 1000),
        ("2026-11-09", 900),
    ] {
        env.story(day, id, 100);
    }
    env.store
        .enqueue_items("show_hn", QueueKind::Story, "2026-11-10", &[1000], NOW)
        .unwrap();
    let log = env.fetch_log();
    let (result, report) = env.run("");
    assert_eq!(result.exit_code, 0, "{report}");
    let order: Vec<i64> = log.lock().unwrap().clone();
    // Resume (11-10), head newest first (15, 14, 13), backfill (12, 11, 09).
    assert_eq!(order, [1000, 1500, 1400, 1300, 1200, 1100, 900]);
    // The queued day was not rediscovered: 3 head + 3 backfill searches.
    assert_eq!(env.searches(), 6);
    assert_eq!(
        env.done_days(),
        [
            "2026-11-09",
            "2026-11-10",
            "2026-11-11",
            "2026-11-12",
            "2026-11-13",
            "2026-11-14",
            "2026-11-15"
        ]
    );
}

#[test]
fn head_stories_do_not_consume_the_cap() {
    let mut env = Env::new();
    env.story("2026-11-15", 1500, 100);
    env.story("2026-11-15", 1510, 200);
    env.story("2026-11-14", 1400, 100);
    env.story("2026-11-14", 1410, 200);
    for (n, id) in [1300, 1310, 1320].into_iter().enumerate() {
        env.story("2026-11-13", id, 100 * (n as i64 + 1));
    }
    let (result, report) = env.run("--days 2 --backfill-days 1 --max-items 1");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(result.summary.items_processed, 5, "{report}");
    assert_eq!(env.pending("2026-11-13"), 2);
    assert_eq!(env.state("2026-11-13").unwrap().0, UnitState::InProgress);
    assert_eq!(result.summary.outcome, run::Outcome::LimitReached);
}

#[test]
fn head_window_larger_than_the_cap_still_lets_backfill_advance() {
    let mut env = Env::new();
    for (n, id) in [1500, 1510, 1520].into_iter().enumerate() {
        env.story("2026-11-15", id, 100 * (n as i64 + 1));
    }
    env.story("2026-11-14", 1400, 100);
    env.story("2026-11-14", 1410, 200);
    let (result, report) = env.run("--days 1 --backfill-days 1 --max-items 1");
    assert_eq!(result.summary.items_processed, 4, "{report}");
    assert_eq!(env.pending("2026-11-14"), 1);
    let (result, report) = env.run("--days 1 --backfill-days 1 --max-items 1");
    // Head re-walks its three stories; resume finishes the last backfill one.
    assert_eq!(result.summary.items_processed, 4, "{report}");
    assert_eq!(env.state("2026-11-14"), Some((UnitState::Done, 0)));
    assert_eq!(env.hn.calls(1400), 1);
    assert_eq!(env.hn.calls(1410), 1);
}

#[test]
fn backfill_takes_newest_uncovered_days_first_and_fills_gaps() {
    let mut env = Env::new();
    env.store
        .seed_unit(
            "show_hn",
            QueueKind::Story,
            "2026-11-13",
            UnitState::Done,
            &[],
            NOW,
        )
        .unwrap();
    let (_, report) = env.run("--days 1 --backfill-days 2");
    assert_eq!(
        env.done_days(),
        ["2026-11-12", "2026-11-13", "2026-11-14", "2026-11-15"],
        "{report}"
    );
    assert!(report.contains("day(s)"), "{report}");
    env.run("--days 1 --backfill-days 2");
    assert_eq!(env.done_days().len(), 6);
    assert!(env.state("2026-11-10").is_some());
}

#[test]
fn empty_day_is_done_and_not_rescanned() {
    let mut env = Env::new();
    let (result, report) = env.run("--days 1 --backfill-days 1");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(env.searches(), 2);
    assert_eq!(env.done_days(), ["2026-11-14", "2026-11-15"]);
    assert_eq!(env.state("2026-11-14").unwrap().1, 0);
    assert!(report.contains("Errors (0 groups)"), "{report}");
    env.run("--days 1 --backfill-days 1");
    // Head again plus one new backfill day, not the covered empty day.
    assert_eq!(env.searches(), 4);
    assert_eq!(env.done_days().len(), 3);
}

#[test]
fn day_larger_than_the_batch_size_uses_cursor_batches() {
    let mut env = Env::new();
    for id in 1..=5 {
        env.story("2026-11-15", id, id * 100);
    }
    let (result, report) = env.run("--days 1 --backfill-days 1 --batch-size 2 --auto-approve");
    assert_eq!(result.exit_code, 0, "{report}");
    // The inclusive cursor repeats the boundary story, so five stories at
    // two per search take five searches, plus one for the empty day.
    assert_eq!(env.searches(), 6);
    let log = env.disc.search_log.lock().unwrap().clone();
    assert!(log.iter().all(|(_, _, limit)| *limit == 2));
    assert!(log[1].1 < log[0].1 && log[2].1 < log[1].1, "{log:?}");
    assert_eq!(*env.model.order.lock().unwrap(), [5, 4, 3, 2, 1]);
    assert_eq!(result.summary.items_approved, 5);
}

#[test]
fn tie_at_a_batch_boundary_loses_no_story() {
    let mut env = Env::new();
    // Stories 7 and 6 share a timestamp straddling the first batch edge.
    for (id, secs) in [(9, 900), (8, 800), (7, 700), (6, 700), (4, 400)] {
        env.story("2026-11-15", id, secs);
    }
    let (result, report) = env.run("--days 1 --backfill-days 1 --batch-size 3");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(*env.model.order.lock().unwrap(), [9, 8, 7, 6, 4]);
}

#[test]
fn story_cap_stops_at_a_story_boundary_and_the_next_run_resumes() {
    let mut env = Env::new();
    for (n, id) in [1400, 1410, 1420].into_iter().enumerate() {
        env.story("2026-11-14", id, 100 * (n as i64 + 1));
    }
    let (result, report) = env.run("--days 1 --backfill-days 1 --max-items 2");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(result.summary.outcome, run::Outcome::LimitReached);
    assert_eq!(result.summary.items_processed, 2);
    assert!(report.contains("story cap reached (2/2)"), "{report}");
    assert_eq!(env.pending("2026-11-14"), 1);
    assert!(report.contains("pending day(s)"), "{report}");
    assert_eq!(env.searches(), 2);

    let (result, report) = env.run("--days 1 --backfill-days 1 --max-items 2");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(env.state("2026-11-14"), Some((UnitState::Done, 0)));
    // Head (11-15) and the new backfill day (11-13) only; 11-14 not rediscovered.
    assert_eq!(env.searches(), 4);
    assert_eq!(env.model.calls.load(Ordering::SeqCst), 3);
}

#[test]
fn results_await_review_unless_auto_approve() {
    let mut env = Env::new();
    env.story("2026-11-15", 1, 100);
    env.story("2026-11-15", 2, 200);
    let (result, report) = env.run("--days 1 --backfill-days 1");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(result.summary.items_approved, 0);
    assert_eq!(env.pending_mentions(), 2);
    assert!(env.export.join("current.json").exists());
    let (result, report) = env.run("--days 1 --backfill-days 1 --auto-approve");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(result.summary.items_approved, 2);
    assert_eq!(env.pending_mentions(), 0);
    // Unchanged stories are not extracted again.
    assert_eq!(env.model.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn thread_processes_a_story_without_changing_day_coverage() {
    let mut env = Env::new();
    env.story("2026-11-14", 100, 100);
    env.story("2026-11-14", 200, 200);
    let (result, report) = env.run("--thread 100");
    assert_eq!(result.exit_code, 0, "{report}");
    assert!(env.states().is_empty(), "no coverage row is created");
    assert_eq!(env.searches(), 0);
    assert_eq!(env.pending_mentions(), 1);
    assert_eq!(
        env.store.extraction_mode(100).unwrap(),
        Some(ExtractionMode::ShowHn)
    );
    assert!(
        report.contains("an explicit story was processed"),
        "{report}"
    );

    // Explicit approval later reuses the extraction.
    let (result, report) = env.run("--thread 100 --auto-approve");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(env.pending_mentions(), 0);
    assert_eq!(env.model.calls.load(Ordering::SeqCst), 1);

    // A queued story's queue row is marked; the day stays as it was.
    env.store
        .enqueue_items("show_hn", QueueKind::Story, "2026-11-14", &[200], NOW)
        .unwrap();
    let (result, report) = env.run("--thread 200");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(env.pending("2026-11-14"), 0);
    assert_eq!(env.state("2026-11-14").unwrap().0, UnitState::Pending);
}

#[test]
fn thread_failure_exits_one_with_the_full_message() {
    let mut env = Env::new();
    env.story("2026-11-15", 100, 100);
    let long = format!("permanent failure {}", "x".repeat(700));
    env.model.fail.lock().unwrap().insert(100, long.clone());
    let (result, report) = env.run("--thread 100 --auto-approve");
    assert_eq!(result.exit_code, 1, "{report}");
    assert!(report.contains(&long), "{report}");
    assert!(env.states().is_empty());
}

#[test]
fn non_show_story_and_monthly_assignment_fail_before_model_calls() {
    let mut env = Env::new();
    env.story_titled("2026-11-15", 100, 100, "Ask HN: Alpha");
    let (result, report) = env.run("--thread 100 --auto-approve");
    assert_eq!(result.exit_code, 1, "{report}");
    assert!(report.contains("live Show HN:"), "{report}");
    assert_eq!(env.store.extraction_mode(100).unwrap(), None);
    env.store
        .assign_extraction_mode(100, ExtractionMode::Monthly)
        .unwrap();
    let (result, report) = env.run("--thread 100 --auto-approve");
    assert_eq!(result.exit_code, 1, "{report}");
    assert!(report.contains("assigned to monthly"), "{report}");
    assert_eq!(env.model.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn from_through_marks_coverage_and_rewalks_done_days() {
    let mut env = Env::new();
    env.story("2026-11-10", 100, 100);
    env.story("2026-11-11", 110, 100);
    let (result, report) = env.run("--from 2026-11-10 --through 2026-11-11");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(env.done_days(), ["2026-11-10", "2026-11-11"]);
    // No head or backfill: 11-15 is not covered.
    assert!(env.state("2026-11-15").is_none());
    assert_eq!(*env.model.order.lock().unwrap(), [110, 100]);
    assert_eq!(env.searches(), 2);
    let (result, report) = env.run("--from 2026-11-10 --through 2026-11-11");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(env.searches(), 4);
    assert_eq!(env.hn.calls(110), 2);
    // Unchanged extraction is skipped.
    assert_eq!(env.model.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn algolia_breaker_trip_exits_two_and_exports() {
    let mut env = Env::new();
    env.story("2026-11-15", 1500, 100);
    // Call 1 (11-15) succeeds; calls 2 to 4 (11-14, 11-13, 11-12) fail.
    env.disc
        .search_failures
        .lock()
        .unwrap()
        .extend((2..=4).map(|n| (n, "Show HN discovery failed: connection reset".to_owned())));
    let (result, report) = env.run("--days 3 --backfill-days 3 --auto-approve");
    assert_eq!(result.exit_code, 2, "{report}");
    assert_eq!(result.summary.outcome, run::Outcome::StoppedOnFailure);
    assert!(report.contains("Algolia"), "{report}");
    assert!(env.export.join("current.json").exists());
    // The backfill day after the tripping search was never searched.
    assert_eq!(env.searches(), 4);
}

#[test]
fn interrupt_stops_the_running_story_without_failing_it_and_resumes() {
    let mut env = Env::new();
    env.story("2026-11-15", 1500, 100);
    env.story("2026-11-15", 1510, 200);
    let control: Arc<OnceLock<Arc<RunControl>>> = Arc::new(OnceLock::new());
    let hook = control.clone();
    env.hn.on_fetch = Some(Box::new(move |id| {
        if id == 1510
            && let Some(control) = hook.get()
        {
            control.interrupt();
        }
    }));
    let (result, report) = env.run_with("--days 1 --backfill-days 1", |session| {
        control.set(session.control.clone()).unwrap();
    });
    assert_eq!(result.exit_code, 130, "{report}");
    assert_eq!(result.summary.outcome, run::Outcome::Interrupted);
    assert!(env.export.join("current.json").exists());
    assert_eq!(env.model.calls.load(Ordering::SeqCst), 0);
    assert_eq!(env.hn.calls(1500), 0);
    assert_eq!(env.state("2026-11-15"), Some((UnitState::InProgress, 0)));
    assert!(!report.contains("Errors (1"), "{report}");

    env.hn.on_fetch = None;
    let (result, report) = env.run("--days 1 --backfill-days 1");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(env.state("2026-11-15"), Some((UnitState::Done, 0)));
    assert_eq!(env.model.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn failed_story_is_reported_in_full_and_later_work_continues() {
    let mut env = Env::new();
    for id in 1..=5 {
        env.story("2026-11-15", id, id * 10);
    }
    let long = format!("permanent failure {}", "y".repeat(650));
    env.model.fail.lock().unwrap().insert(4, long.clone());
    let (result, report) = env.run("--days 1 --backfill-days 1 --auto-approve");
    assert_eq!(result.exit_code, 1, "{report}");
    assert_eq!(*env.model.order.lock().unwrap(), [5, 4, 3, 2, 1]);
    assert!(report.contains(&long), "{report}");
    assert_eq!(result.summary.items_approved, 4);
    let connection = rusqlite::Connection::open(&env.db).unwrap();
    let approved: Vec<i64> = connection
        .prepare("SELECT root_item_id FROM mentions WHERE review_state='approved' ORDER BY 1")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(approved, [1, 2, 3, 5]);
    assert_eq!(env.state("2026-11-15").unwrap().0, UnitState::Failed);
}

#[test]
fn monthly_assigned_story_is_a_per_story_failure() {
    let mut env = Env::new();
    for id in 1..=3 {
        env.story("2026-11-15", id, id * 10);
    }
    Collector::new(&env.hn, CollectionConfig::default())
        .collect(&mut env.store, 2)
        .unwrap();
    env.store
        .assign_extraction_mode(2, ExtractionMode::Monthly)
        .unwrap();
    let (result, report) = env.run("--days 1 --backfill-days 1");
    assert_eq!(result.exit_code, 1, "{report}");
    assert!(
        report.contains("thread 2 is assigned to monthly"),
        "{report}"
    );
    assert_eq!(*env.model.order.lock().unwrap(), [3, 1]);
}

#[test]
fn concurrent_extraction_in_a_chunk_processes_every_story() {
    let mut env = Env::new();
    env.concurrency = "4";
    for id in 1..=5 {
        env.story("2026-11-15", id, id * 10);
    }
    let (result, report) = env.run("--days 1 --backfill-days 1 --auto-approve");
    assert_eq!(result.exit_code, 0, "{report}");
    let mut seen = env.model.order.lock().unwrap().clone();
    seen.sort();
    assert_eq!(seen, [1, 2, 3, 4, 5]);
    assert_eq!(result.summary.items_approved, 5);
}

#[test]
fn recheck_of_collected_date_and_deleted_sources_skip_extraction() {
    let mut env = Env::new();
    // Algolia places story 100 on 11-15, but HN says it was posted 11-10.
    env.story("2026-11-15", 100, 100);
    env.hn.items.get_mut(&100).unwrap()["time"] = json!(at("2026-11-10", 100));
    env.story("2026-11-15", 200, 200);
    env.hn.items.get_mut(&200).unwrap()["deleted"] = json!(true);
    let (result, report) = env.run("--days 1 --backfill-days 1 --auto-approve");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(env.model.calls.load(Ordering::SeqCst), 0);
    assert!(env.export.join("current.json").exists());
    assert_eq!(env.state("2026-11-15"), Some((UnitState::Done, 0)));
}

#[test]
fn discovery_failure_fails_the_whole_day_and_a_later_run_retries_it() {
    let mut env = Env::new();
    for id in 1..=4 {
        env.story("2026-11-15", id, id * 10);
    }
    env.disc
        .search_failures
        .lock()
        .unwrap()
        .push((2, "search offline".to_owned()));
    let (result, report) = env.run("--days 1 --backfill-days 1 --batch-size 2");
    assert_eq!(result.exit_code, 1, "{report}");
    assert!(report.contains("search offline"), "{report}");
    assert_eq!(env.state("2026-11-15"), Some((UnitState::Failed, 1)));
    assert_eq!(env.model.calls.load(Ordering::SeqCst), 0);
    env.disc.search_failures.lock().unwrap().clear();
    let (result, report) = env.run("--days 1 --backfill-days 1 --batch-size 2");
    assert_eq!(result.exit_code, 0, "{report}");
    assert_eq!(env.state("2026-11-15"), Some((UnitState::Done, 0)));
    assert_eq!(*env.model.order.lock().unwrap(), [4, 3, 2, 1]);
}
