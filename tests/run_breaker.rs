mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wrkgn::collector::{CollectionConfig, Collector};
use wrkgn::processing::{Options, process_thread_with};
use wrkgn::run::{Api, Breaker, RunControl};

use common::{FakeHn, ScriptModel, fresh_store, thread};

fn fast_collection() -> CollectionConfig {
    CollectionConfig {
        max_concurrency: 1,
        max_attempts: 1,
        retry_base_delay: Duration::ZERO,
    }
}

fn processing_options(control: Arc<RunControl>) -> Options {
    Options {
        concurrency: 1,
        max_attempts: 1,
        initial_backoff: Duration::ZERO,
        max_backoff: Duration::ZERO,
        control: Some(control),
        ..Options::default()
    }
}

const RATE_LIMIT: &str = "ProviderResponseError: status 429 Too Many Requests";

#[test]
fn counters_are_separate_per_api_and_reset_on_success() {
    let breaker = Breaker::new(3);
    assert!(!breaker.record_failure(Api::Llm));
    assert!(!breaker.record_failure(Api::Llm));
    assert!(!breaker.record_failure(Api::Algolia));
    assert!(!breaker.record_failure(Api::HnFirebase));
    breaker.record_success(Api::Llm);
    assert_eq!(breaker.consecutive_failures(Api::Llm), 0);
    assert_eq!(breaker.consecutive_failures(Api::Algolia), 1);
    assert!(!breaker.record_failure(Api::Llm));
    assert!(!breaker.record_failure(Api::Llm));
    assert!(!breaker.is_tripped());
    assert!(breaker.record_failure(Api::Llm));
    assert!(breaker.is_tripped());
    assert_eq!(breaker.tripped_api(), Some(Api::Llm));
}

#[test]
fn counter_is_shared_across_worker_threads() {
    let breaker = Breaker::new(8);
    std::thread::scope(|scope| {
        for _ in 0..7 {
            scope.spawn(|| breaker.record_failure(Api::HnFirebase));
        }
    });
    assert!(!breaker.is_tripped());
    std::thread::scope(|scope| {
        scope.spawn(|| breaker.record_failure(Api::HnFirebase));
    });
    assert!(breaker.is_tripped());
    assert_eq!(breaker.tripped_api(), Some(Api::HnFirebase));
}

#[test]
fn disabled_control_never_trips_but_still_interrupts() {
    let control = RunControl::disabled();
    for _ in 0..1000 {
        control.breaker.record_failure(Api::Llm);
    }
    assert!(!control.should_stop());
    assert!(!control.interrupt());
    assert!(control.interrupt());
    assert!(control.should_stop());
}

#[test]
fn collector_trips_after_three_exhausted_fetches_and_ends_incomplete() {
    let (_dir, _db, mut store) = fresh_store();
    let kids: Vec<i64> = (101..=106).collect();
    let mut hn = FakeHn::new(thread(100, &kids));
    hn.transient.extend(101..=106);
    let control = Arc::new(RunControl::default());
    let report = Collector::new(&hn, fast_collection())
        .with_control(control.clone())
        .collect(&mut store, 100)
        .unwrap();
    assert!(control.breaker.is_tripped());
    assert_eq!(control.breaker.tripped_api(), Some(Api::HnFirebase));
    assert!(report.stopped);
    assert!(!report.complete);
    // Exactly three kids were attempted; the rest were never fetched.
    assert_eq!((101..=106).map(|id| hn.calls(id)).sum::<usize>(), 3);
    assert_eq!(hn.calls(106), 0);
    // Only real fetch failures are recorded; the stop itself is a flag.
    assert!(!report.failures.is_empty());
    assert!(
        report
            .failures
            .iter()
            .all(|f| !f.detail.contains("stopped"))
    );
    assert_eq!(
        store.get_run(report.run_id).unwrap().unwrap().status,
        "incomplete"
    );
}

#[test]
fn stopped_collection_leaves_presence_unreconciled() {
    let (_dir, _db, mut store) = fresh_store();
    let first = Collector::new(
        &FakeHn::new(thread(100, &[101, 102, 103])),
        fast_collection(),
    )
    .collect(&mut store, 100)
    .unwrap();
    assert!(first.complete);

    // The tree shrinks, but the run is interrupted right after the root.
    let control = Arc::new(RunControl::default());
    let mut hn = FakeHn::new(thread(100, &[101, 102]));
    let interrupt = control.clone();
    hn.on_fetch = Some(Box::new(move |id| {
        if id == 100 {
            interrupt.interrupt();
        }
    }));
    let report = Collector::new(&hn, fast_collection())
        .with_control(control)
        .collect(&mut store, 100)
        .unwrap();
    assert!(report.stopped && !report.complete);
    assert!(report.failures.is_empty(), "a stop records no fake failure");
    assert_eq!(hn.calls(101), 0);
    assert_eq!(
        store.get_run(report.run_id).unwrap().unwrap().status,
        "incomplete"
    );
    // Item 103 left the tree, but an incomplete run must not mark it absent.
    assert!(store.get_item(103).unwrap().unwrap().present_in_tree);

    // A later complete run reconciles it.
    let done = Collector::new(&FakeHn::new(thread(100, &[101, 102])), fast_collection())
        .collect(&mut store, 100)
        .unwrap();
    assert!(done.complete);
    assert!(!store.get_item(103).unwrap().unwrap().present_in_tree);
}

#[test]
fn null_items_are_not_failures_for_the_breaker() {
    let (_dir, _db, mut store) = fresh_store();
    let mut hn = FakeHn::new(thread(100, &[101, 102, 103, 104]));
    hn.null.extend(101..=104);
    let control = Arc::new(RunControl::with_threshold(1));
    let report = Collector::new(&hn, fast_collection())
        .with_control(control.clone())
        .collect(&mut store, 100)
        .unwrap();
    assert!(!control.breaker.is_tripped());
    assert!(!report.stopped);
    // Null items are still recorded as collection failures.
    assert_eq!(report.failures.len(), 4);
    assert_eq!(hn.total_calls(), 5);
}

#[test]
fn permanent_errors_are_not_counted_and_success_resets_the_counter() {
    let (_dir, _db, mut store) = fresh_store();
    let mut hn = FakeHn::new(thread(100, &[101, 102, 103, 104, 105]));
    hn.permanent.extend([101, 102, 103]);
    hn.transient.extend([104]);
    let control = Arc::new(RunControl::with_threshold(2));
    // Items 101-103 fail permanently, 104 exhausts once, 105 succeeds.
    let report = Collector::new(&hn, fast_collection())
        .with_control(control.clone())
        .collect(&mut store, 100)
        .unwrap();
    assert!(!control.breaker.is_tripped());
    assert!(!report.stopped);
    assert_eq!(control.breaker.consecutive_failures(Api::HnFirebase), 0);
}

#[test]
fn validation_failures_do_not_trip_the_breaker() {
    let (_dir, _db, mut store) = fresh_store();
    let hn = FakeHn::new(thread(100, &[101, 102, 103, 104]));
    assert!(
        Collector::new(&hn, fast_collection())
            .collect(&mut store, 100)
            .unwrap()
            .complete
    );
    let model = ScriptModel::failing("empty description: source 101");
    let control = Arc::new(RunControl::with_threshold(2));
    let report = process_thread_with(
        &mut store,
        100,
        &model,
        false,
        &processing_options(control.clone()),
        |_| {},
    )
    .unwrap();
    assert!(!control.breaker.is_tripped());
    assert_eq!((report.failed.len(), report.not_attempted), (4, 0));
    assert!(!report.stopped_early);
}

#[test]
fn model_trips_after_consecutive_exhausted_calls_and_resets_on_success() {
    let (_dir, _db, mut store) = fresh_store();
    let hn = FakeHn::new(thread(100, &[101, 102, 103, 104, 105]));
    Collector::new(&hn, fast_collection())
        .collect(&mut store, 100)
        .unwrap();
    // Failure, success (an empty result), failure: never two in a row.
    let model = ScriptModel::new(vec![
        Err(RATE_LIMIT.into()),
        Ok(wrkgn::extraction::Extraction { mentions: vec![] }),
        Err(RATE_LIMIT.into()),
        Ok(wrkgn::extraction::Extraction { mentions: vec![] }),
        Err(RATE_LIMIT.into()),
    ]);
    let control = Arc::new(RunControl::with_threshold(2));
    let report = process_thread_with(
        &mut store,
        100,
        &model,
        false,
        &processing_options(control.clone()),
        |_| {},
    )
    .unwrap();
    assert!(!control.breaker.is_tripped());
    assert_eq!((report.processed, report.failed.len()), (2, 3));

    // Two failures in a row trip it and leave the rest unattempted.
    let model = ScriptModel::failing(RATE_LIMIT);
    let control = Arc::new(RunControl::with_threshold(2));
    let report = process_thread_with(
        &mut store,
        100,
        &model,
        true,
        &processing_options(control.clone()),
        |_| {},
    )
    .unwrap();
    assert!(control.breaker.is_tripped());
    assert_eq!(control.breaker.tripped_api(), Some(Api::Llm));
    assert!(report.stopped_early);
    assert_eq!((report.failed.len(), report.not_attempted), (2, 3));
}

#[test]
fn interrupt_stops_processing_at_an_item_boundary_and_keeps_saved_groups() {
    let (_dir, _db, mut store) = fresh_store();
    let hn = FakeHn::new(thread(100, &[101, 102, 103]));
    Collector::new(&hn, fast_collection())
        .collect(&mut store, 100)
        .unwrap();
    let control = Arc::new(RunControl::default());
    let hook = control.clone();
    let model = ScriptModel {
        on_call: Some(Box::new(move |n| {
            if n == 0 {
                hook.interrupt();
            }
        })),
        ..ScriptModel::default()
    };
    let report = process_thread_with(
        &mut store,
        100,
        &model,
        false,
        &processing_options(control.clone()),
        |_| {},
    )
    .unwrap();
    assert!(report.stopped_early);
    // The in-flight group finished and was saved; the others never started.
    assert_eq!((report.processed, report.not_attempted), (1, 2));
    assert_eq!(model.calls(), 1);
    assert!(!control.breaker.is_tripped());

    // A later run with no control resumes: the saved group is skipped.
    let resumed = process_thread_with(
        &mut store,
        100,
        &ScriptModel::default(),
        false,
        &Options {
            concurrency: 1,
            ..Options::default()
        },
        |_| {},
    )
    .unwrap();
    assert_eq!((resumed.skipped, resumed.processed), (1, 2));
}

#[test]
fn without_a_control_each_thread_keeps_its_own_breaker() {
    let (_dir, _db, mut store) = fresh_store();
    let hn = FakeHn::new(thread(100, &[101, 102, 103, 104, 105]));
    Collector::new(&hn, fast_collection())
        .collect(&mut store, 100)
        .unwrap();
    let model = ScriptModel::failing(RATE_LIMIT);
    let calls = AtomicUsize::new(0);
    let report = process_thread_with(
        &mut store,
        100,
        &model,
        false,
        &Options {
            concurrency: 1,
            max_attempts: 1,
            initial_backoff: Duration::ZERO,
            ..Options::default()
        },
        |_| {
            calls.fetch_add(1, Ordering::SeqCst);
        },
    )
    .unwrap();
    assert!(report.stopped_early);
    assert_eq!((report.failed.len(), report.not_attempted), (3, 2));
}
