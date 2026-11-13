use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::tempdir;
use wrkgn::publication;
use wrkgn::store::{ExtractionMode, SourceItem, Store};

fn source(id: i64, parent: Option<i64>) -> SourceItem {
    SourceItem {
        item_id: id,
        thread_id: 100,
        parent_id: parent,
        sibling_position: 0,
        author: Some("alice".into()),
        created_at: Some(id),
        body: Some("Example".into()),
        raw_payload: json!({"id":id,"title":"Show HN: Example","url":"https://example.test"}),
        deleted: false,
        dead: false,
    }
}
#[test]
fn modes_are_unassigned_until_processing_and_conflicts_are_rejected() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(dir.path().join("db")).unwrap();
    let run = store.begin_run(100, 1).unwrap();
    store.upsert_item(run, 2, &source(100, None)).unwrap();
    store.finish_run(run, 3).unwrap();
    assert_eq!(store.extraction_mode(100).unwrap(), None);
    store
        .assign_extraction_mode(100, ExtractionMode::ShowHn)
        .unwrap();
    assert!(
        store
            .assign_extraction_mode(100, ExtractionMode::Monthly)
            .is_err()
    );
    assert_eq!(
        store.extraction_mode(100).unwrap(),
        Some(ExtractionMode::ShowHn)
    );
}
#[test]
fn version_six_backfills_processed_threads_without_changing_cache() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("db");
    let mut store = Store::open(&path).unwrap();
    let run = store.begin_run(100, 1).unwrap();
    store.upsert_item(run, 2, &source(100, None)).unwrap();
    store.finish_run(run, 3).unwrap();
    drop(store);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE mention_evidence; DROP TABLE show_chunk_cache; DROP TABLE show_processing_results; DROP TABLE thread_extraction_modes; PRAGMA user_version=6; INSERT INTO processing_runs(id,thread_id,started_at,status,prompt_version,schema_version,model) VALUES(1,100,1,'complete','original','4','fixture');").unwrap();
    drop(conn);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.extraction_mode(100).unwrap(),
        Some(ExtractionMode::Monthly)
    );
    drop(store);
    let conn = Connection::open(path).unwrap();
    assert_eq!(
        conn.query_row("SELECT prompt_version FROM processing_runs", [], |r| r
            .get::<_, String>(
            0
        ))
        .unwrap(),
        "original"
    );
}
#[test]
fn evidence_authorization_and_suppression_withhold_public_mentions() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("db");
    let mut store = Store::open(&path).unwrap();
    let run = store.begin_run(100, 1).unwrap();
    store.upsert_item(run, 2, &source(100, None)).unwrap();
    store.upsert_item(run, 2, &source(101, Some(100))).unwrap();
    store.finish_run(run, 3).unwrap();
    let tree = store.conversation_groups(100).unwrap();
    let root = wrkgn::store::ConversationNode {
        item: tree.thread,
        replies: tree.groups,
    };
    let fp = wrkgn::extraction::group_fingerprint(&root);
    store
        .assign_extraction_mode(100, ExtractionMode::ShowHn)
        .unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("INSERT INTO processing_runs(id,thread_id,started_at,status,prompt_version,schema_version,model) VALUES(1,100,1,'complete','show','1','fixture'); INSERT INTO projects VALUES(1,'example','example',1,0);").unwrap();
    conn.execute("INSERT INTO processing_groups VALUES(100,100,?1,'show','1','fixture','complete',1,2,2,NULL)",[fp]).unwrap();
    conn.execute_batch("INSERT INTO mentions(id,project_id,root_item_id,source_item_id,mention_key,name,description,builder_username,review_state,source_fingerprint,source_excerpt) SELECT 1,1,100,100,'100:sole','Example','Example','alice','approved',content_fingerprint,'Example' FROM source_items WHERE item_id=100;
    INSERT INTO mention_evidence SELECT 1,100,'title','Show HN: Example','[\"name\"]',content_fingerprint,NULL,NULL FROM source_items WHERE item_id=100;
    INSERT INTO mention_evidence SELECT 1,101,'body','Example','[\"description\"]',content_fingerprint,100,(SELECT content_fingerprint FROM source_items WHERE item_id=100) FROM source_items WHERE item_id=101;").unwrap();
    drop(conn);
    let export = dir.path().join("export");
    publication::export(&mut store, &export).unwrap();
    let pointer: Value =
        serde_json::from_slice(&std::fs::read(export.join("current.json")).unwrap()).unwrap();
    assert_eq!(pointer["schema_version"], 5);
    let details: Value = serde_json::from_slice(
        &std::fs::read(export.join(format!(
            "snapshots/{}/projects/example.json",
            pointer["version"].as_str().unwrap()
        )))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(details["mentions"][0]["evidence"][1]["author"], "alice");
    // An authorization edit invalidates evidence even without a new model result.
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE mention_evidence SET authorization_fingerprint='edited' WHERE source_item_id=101",
        [],
    )
    .unwrap();
    publication::export(&mut store, &export).unwrap();
    let stale: Value =
        serde_json::from_slice(&std::fs::read(export.join("current.json")).unwrap()).unwrap();
    let stale_discovery: Value = serde_json::from_slice(
        &std::fs::read(export.join(format!(
            "snapshots/{}/discovery.json",
            stale["version"].as_str().unwrap()
        )))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(stale_discovery, json!([]));
    conn.execute("UPDATE mention_evidence SET authorization_fingerprint=(SELECT content_fingerprint FROM source_items WHERE item_id=100) WHERE source_item_id=101", []).unwrap();
    drop(conn);
    publication::suppress_source(&mut store, 101, "removal").unwrap();
    publication::export(&mut store, &export).unwrap();
    let pointer: Value =
        serde_json::from_slice(&std::fs::read(export.join("current.json")).unwrap()).unwrap();
    let discovery: Value = serde_json::from_slice(
        &std::fs::read(export.join(format!(
            "snapshots/{}/discovery.json",
            pointer["version"].as_str().unwrap()
        )))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(discovery, json!([]));
}
