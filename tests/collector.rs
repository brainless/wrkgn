use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::tempdir;
use wrkgn::collector::{CollectionConfig, Collector, FetchError, FetchErrorKind, HnApi};
use wrkgn::store::Store;

type FixtureResponse = Result<Option<Value>, FetchError>;

struct FixtureApi {
    responses: Mutex<HashMap<i64, VecDeque<FixtureResponse>>>,
    calls: Mutex<HashMap<i64, usize>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl FixtureApi {
    fn new(responses: impl IntoIterator<Item = (i64, Vec<FixtureResponse>)>) -> Self {
        Self {
            responses: Mutex::new(
                responses
                    .into_iter()
                    .map(|(id, values)| (id, values.into()))
                    .collect(),
            ),
            calls: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        }
    }

    fn calls(&self, id: i64) -> usize {
        *self.calls.lock().unwrap().get(&id).unwrap_or(&0)
    }
}

impl HnApi for &FixtureApi {
    fn fetch_item(&self, item_id: i64) -> Result<Option<Value>, FetchError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        // A fixed delay lets the fixture exercise overlapping child requests.
        thread::sleep(Duration::from_millis(if item_id == 101 { 12 } else { 3 }));
        *self.calls.lock().unwrap().entry(item_id).or_default() += 1;
        let response = self
            .responses
            .lock()
            .unwrap()
            .get_mut(&item_id)
            .and_then(VecDeque::pop_front)
            .expect("fixture response missing");
        self.active.fetch_sub(1, Ordering::SeqCst);
        response
    }
}

fn config() -> CollectionConfig {
    CollectionConfig {
        max_concurrency: 2,
        max_attempts: 3,
        retry_base_delay: Duration::ZERO,
    }
}

#[test]
fn collects_complete_nested_tree_with_source_order_and_explicit_states() {
    let api = FixtureApi::new([
        (
            100,
            vec![Ok(Some(
                json!({"id":100,"type":"story","by":"root","time":1700000000,"title":"Ask HN","kids":[101,102]}),
            ))],
        ),
        (
            101,
            vec![Ok(Some(
                json!({"id":101,"parent":100,"by":"one","time":1700000001,"text":"first","kids":[103]}),
            ))],
        ),
        (
            102,
            vec![Ok(Some(
                json!({"id":102,"parent":100,"deleted":true,"kids":[]}),
            ))],
        ),
        (
            103,
            vec![Ok(Some(
                json!({"id":103,"parent":101,"by":"three","time":1700000003,"dead":true,"text":"nested"}),
            ))],
        ),
    ]);
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("source.sqlite")).unwrap();
    let report = Collector::new(&api, config())
        .collect(&mut store, 100)
        .unwrap();

    assert!(report.complete);
    assert!(report.failures.is_empty());
    assert_eq!(report.items_fetched, 4);
    assert_eq!(report.changed_item_ids, vec![100, 101, 102, 103]);
    assert_eq!(
        store
            .children_of(100)
            .unwrap()
            .iter()
            .map(|item| item.source.item_id)
            .collect::<Vec<_>>(),
        vec![101, 102]
    );
    assert_eq!(
        store.children_of(101).unwrap()[0].source.parent_id,
        Some(101)
    );
    assert_eq!(
        store.get_item(101).unwrap().unwrap().source.created_at,
        Some(1_700_000_001)
    );
    assert!(store.get_item(102).unwrap().unwrap().source.deleted);
    assert!(store.get_item(103).unwrap().unwrap().source.dead);
    assert_eq!(
        store.get_run(report.run_id).unwrap().unwrap().status,
        "complete"
    );
    assert_eq!(api.max_active.load(Ordering::SeqCst), 2);
}

#[test]
fn transient_retry_and_null_or_failed_children_report_incomplete_tree() {
    let api = FixtureApi::new([
        (100, vec![Ok(Some(json!({"id":100,"kids":[101,102,103]})))]),
        (
            101,
            vec![
                Err(FetchError::transient(FetchErrorKind::Transport, "timeout")),
                Ok(Some(json!({"id":101,"parent":100,"text":"retried"}))),
            ],
        ),
        (102, vec![Ok(None)]),
        (
            103,
            vec![
                Err(FetchError::transient(FetchErrorKind::Http, "503")),
                Err(FetchError::transient(FetchErrorKind::Http, "503")),
                Err(FetchError::transient(FetchErrorKind::Http, "503")),
            ],
        ),
    ]);
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("source.sqlite")).unwrap();
    let report = Collector::new(&api, config())
        .collect(&mut store, 100)
        .unwrap();

    assert!(!report.complete);
    assert_eq!(report.items_fetched, 2);
    assert_eq!(api.calls(101), 2);
    assert_eq!(api.calls(102), 1);
    assert_eq!(api.calls(103), 3);
    assert_eq!(
        report
            .failures
            .iter()
            .map(|failure| (failure.item_id, failure.kind.as_str()))
            .collect::<Vec<_>>(),
        vec![(102, "unavailable"), (103, "http")]
    );
    assert!(store.get_item(102).unwrap().is_none());
    assert!(store.get_item(103).unwrap().is_none());
    assert_eq!(
        store.get_run(report.run_id).unwrap().unwrap().status,
        "incomplete"
    );
    assert_eq!(store.failures_for_run(report.run_id).unwrap().len(), 2);
}

#[test]
fn invalid_parent_and_cycle_are_reported_without_claiming_completeness() {
    let api = FixtureApi::new([
        (100, vec![Ok(Some(json!({"id":100,"kids":[101,100]})))]),
        (
            101,
            vec![Ok(Some(
                json!({"id":101,"parent":999,"text":"wrong parent"}),
            ))],
        ),
    ]);
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("source.sqlite")).unwrap();
    let report = Collector::new(&api, config())
        .collect(&mut store, 100)
        .unwrap();

    assert!(!report.complete);
    assert_eq!(report.items_fetched, 1);
    assert_eq!(report.failures.len(), 2);
    assert!(
        report
            .failures
            .iter()
            .all(|failure| failure.kind == "invalid_payload")
    );
    assert!(store.get_item(101).unwrap().is_none());
}

#[test]
fn retry_after_partial_run_refetches_tree_and_preserves_successful_items() {
    let root = json!({"id":100,"kids":[101,102]});
    let first = json!({"id":101,"parent":100,"text":"unchanged"});
    let second = json!({"id":102,"parent":100,"text":"recovered"});
    let api = FixtureApi::new([
        (100, vec![Ok(Some(root.clone())), Ok(Some(root))]),
        (101, vec![Ok(Some(first.clone())), Ok(Some(first))]),
        (102, vec![Ok(None), Ok(Some(second))]),
    ]);
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("source.sqlite")).unwrap();
    let collector = Collector::new(&api, config());
    let partial = collector.collect(&mut store, 100).unwrap();
    assert!(!partial.complete);
    assert!(store.get_item(102).unwrap().is_none());

    let recovered = collector.collect(&mut store, 100).unwrap();
    assert!(recovered.complete);
    assert_eq!(recovered.resumed_from_run_id, Some(partial.run_id));
    assert_eq!(recovered.changed_item_ids, vec![102]);
    assert_eq!(store.get_item(100).unwrap().unwrap().revision, 1);
    assert_eq!(store.get_item(101).unwrap().unwrap().revision, 1);
    assert_eq!(store.get_item(102).unwrap().unwrap().revision, 1);
    assert_eq!(
        store.get_run(partial.run_id).unwrap().unwrap().status,
        "incomplete"
    );
    assert_eq!(
        store.get_run(recovered.run_id).unwrap().unwrap().status,
        "complete"
    );
}

#[test]
fn interrupted_run_is_marked_and_replayed_without_duplicate_revisions() {
    let root = json!({"id":100,"kids":[101]});
    let api = FixtureApi::new([
        (100, vec![Ok(Some(root.clone()))]),
        (
            101,
            vec![Ok(Some(json!({"id":101,"parent":100,"text":"reply"})))],
        ),
    ]);
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("source.sqlite")).unwrap();
    let interrupted_run = store.begin_run(100, 10).unwrap();
    store
        .upsert_item(
            interrupted_run,
            11,
            &wrkgn::store::SourceItem {
                item_id: 100,
                thread_id: 100,
                parent_id: None,
                sibling_position: 0,
                author: None,
                created_at: None,
                body: None,
                raw_payload: root,
                deleted: false,
                dead: false,
            },
        )
        .unwrap();

    let recovered = Collector::new(&api, config())
        .collect(&mut store, 100)
        .unwrap();
    assert!(recovered.complete);
    assert_eq!(recovered.resumed_from_run_id, Some(interrupted_run));
    assert_eq!(recovered.changed_item_ids, vec![101]);
    assert_eq!(store.get_item(100).unwrap().unwrap().revision, 1);
    let old_run = store.get_run(interrupted_run).unwrap().unwrap();
    assert_eq!(old_run.status, "incomplete");
    assert!(old_run.interrupted);
    assert!(old_run.finished_at.is_some());
}

#[test]
fn complete_refresh_tracks_absent_then_reappearing_child_without_deletion() {
    let api = FixtureApi::new([
        (
            100,
            vec![
                Ok(Some(json!({"id":100,"kids":[101,102]}))),
                Ok(Some(json!({"id":100,"kids":[101]}))),
                Ok(Some(json!({"id":100,"kids":[101,102]}))),
            ],
        ),
        (
            101,
            vec![
                Ok(Some(json!({"id":101,"parent":100}))),
                Ok(Some(json!({"id":101,"parent":100}))),
                Ok(Some(json!({"id":101,"parent":100}))),
            ],
        ),
        (
            102,
            vec![
                Ok(Some(json!({"id":102,"parent":100,"text":"retained"}))),
                Ok(Some(json!({"id":102,"parent":100,"text":"retained"}))),
            ],
        ),
    ]);
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("source.sqlite")).unwrap();
    let collector = Collector::new(&api, config());
    assert!(collector.collect(&mut store, 100).unwrap().complete);

    let absent = collector.collect(&mut store, 100).unwrap();
    assert!(absent.complete);
    assert!(absent.changed_item_ids.contains(&102));
    let stored = store.get_item(102).unwrap().unwrap();
    assert!(!stored.present_in_tree);
    assert!(!stored.source.deleted);
    assert_eq!(stored.revision, 2);
    assert_eq!(store.conversation_groups(100).unwrap().groups.len(), 1);

    let restored = collector.collect(&mut store, 100).unwrap();
    assert!(restored.complete);
    assert!(restored.changed_item_ids.contains(&102));
    let stored = store.get_item(102).unwrap().unwrap();
    assert!(stored.present_in_tree);
    assert_eq!(stored.revision, 3);
    assert_eq!(store.conversation_groups(100).unwrap().groups.len(), 2);
    assert_eq!(
        store
            .revisions_for_item(102)
            .unwrap()
            .iter()
            .map(|version| version.present_in_tree)
            .collect::<Vec<_>>(),
        vec![true, false, true]
    );
}
