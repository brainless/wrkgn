use serde_json::json;
use tempfile::tempdir;
use wrkgn::store::{FetchFailure, SourceItem, Store};

fn item(id: i64, parent_id: Option<i64>, sibling_position: i64) -> SourceItem {
    SourceItem {
        item_id: id,
        thread_id: 100,
        parent_id,
        sibling_position,
        author: Some(format!("user-{id}")),
        created_at: Some(1_700_000_000 + id),
        body: Some(format!("body-{id}")),
        raw_payload: json!({"id": id, "text": format!("body-{id}")}),
        deleted: false,
        dead: false,
    }
}

#[test]
fn migration_persists_nested_source_and_order() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("nested/source.sqlite");
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 3);
    let run = store.begin_run(100, 10).unwrap();
    store.upsert_item(run, 11, &item(100, None, 0)).unwrap();
    store
        .upsert_item(run, 12, &item(102, Some(100), 1))
        .unwrap();
    store
        .upsert_item(run, 13, &item(101, Some(100), 0))
        .unwrap();
    store
        .upsert_item(run, 14, &item(103, Some(101), 0))
        .unwrap();
    store.finish_run(run, 15).unwrap();
    drop(store);

    let store = Store::open(&path).unwrap();
    let children = store.children_of(100).unwrap();
    assert_eq!(
        children
            .iter()
            .map(|child| child.source.item_id)
            .collect::<Vec<_>>(),
        vec![101, 102]
    );
    assert_eq!(
        store.children_of(101).unwrap()[0].source.parent_id,
        Some(101)
    );
    let child = store.get_item(101).unwrap().unwrap();
    assert_eq!(child.source.author.as_deref(), Some("user-101"));
    assert_eq!(child.source.created_at, Some(1_700_000_101));
    assert_eq!(child.source.raw_payload["id"], 101);
    assert_eq!(child.first_collected_at, 13);
    assert_eq!(store.get_run(run).unwrap().unwrap().status, "complete");
    let tree = store.conversation_groups(100).unwrap();
    assert_eq!(tree.thread.source.item_id, 100);
    assert_eq!(
        tree.groups
            .iter()
            .map(|group| group.item.source.item_id)
            .collect::<Vec<_>>(),
        vec![101, 102]
    );
    assert_eq!(tree.groups[0].replies[0].item.source.item_id, 103);
}

#[test]
fn version_one_database_upgrades_without_losing_source() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("v1.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!("../migrations/001_source_storage.sql"))
        .unwrap();
    connection.pragma_update(None, "user_version", 1).unwrap();
    connection.execute("INSERT INTO collection_runs (id, thread_id, started_at, status) VALUES (1, 100, 10, 'running')", []).unwrap();
    drop(connection);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 3);
    assert_eq!(store.get_run(1).unwrap().unwrap().status, "running");
    assert!(!store.get_run(1).unwrap().unwrap().interrupted);
}

#[test]
fn repeat_refresh_is_idempotent_and_edit_creates_revision() {
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("source.sqlite")).unwrap();
    let first = store.begin_run(100, 10).unwrap();
    let original = item(101, Some(100), 0);
    assert!(store.upsert_item(first, 11, &original).unwrap());
    store.finish_run(first, 12).unwrap();

    let repeat = store.begin_run(100, 20).unwrap();
    assert!(!store.upsert_item(repeat, 21, &original).unwrap());
    assert!(store.changed_item_ids_for_run(repeat).unwrap().is_empty());
    assert_eq!(store.revisions_for_item(101).unwrap().len(), 1);

    let mut edited = original.clone();
    edited.body = Some("edited body".into());
    edited.raw_payload["text"] = json!("edited body");
    assert!(store.upsert_item(repeat, 22, &edited).unwrap());
    store.finish_run(repeat, 23).unwrap();
    let current = store.get_item(101).unwrap().unwrap();
    let revisions = store.revisions_for_item(101).unwrap();
    assert_eq!(current.first_collected_at, 11);
    assert_eq!(current.last_fetched_at, 22);
    assert_eq!(current.revision, 2);
    assert_ne!(
        revisions[0].content_fingerprint,
        revisions[1].content_fingerprint
    );
    assert_eq!(revisions[0].source.body.as_deref(), Some("body-101"));
    assert_eq!(revisions[1].source.body.as_deref(), Some("edited body"));
    assert_eq!(store.changed_item_ids_for_run(repeat).unwrap(), vec![101]);
}

#[test]
fn failure_keeps_last_successful_item_and_explicit_deletion_is_separate() {
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("source.sqlite")).unwrap();
    let first = store.begin_run(100, 10).unwrap();
    let original = item(101, Some(100), 0);
    store.upsert_item(first, 11, &original).unwrap();
    store.finish_run(first, 12).unwrap();

    let failed = store.begin_run(100, 20).unwrap();
    store
        .record_fetch_failure(
            failed,
            &FetchFailure {
                item_id: 101,
                kind: "unavailable".into(),
                detail: "API returned null".into(),
                occurred_at: 21,
            },
        )
        .unwrap();
    store.finish_run(failed, 22).unwrap();
    assert_eq!(store.get_run(failed).unwrap().unwrap().status, "incomplete");
    assert_eq!(store.failures_for_run(failed).unwrap()[0].item_id, 101);
    assert_eq!(store.get_item(101).unwrap().unwrap().source, original);

    let deletion = store.begin_run(100, 30).unwrap();
    let mut deleted = original;
    deleted.deleted = true;
    deleted.body = None;
    deleted.raw_payload = json!({"id": 101, "deleted": true});
    store.upsert_item(deletion, 31, &deleted).unwrap();
    store.finish_run(deletion, 32).unwrap();
    assert!(store.get_item(101).unwrap().unwrap().source.deleted);
    assert_eq!(store.revisions_for_item(101).unwrap().len(), 2);
    assert_eq!(store.get_run(deletion).unwrap().unwrap().status, "complete");
}
