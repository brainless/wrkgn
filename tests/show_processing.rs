use serde_json::{Value, json};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use wrkgn::{
    collector::{CollectionConfig, Collector, FetchError, HnApi},
    extraction::{Extraction, Mention},
    processing::{Model, Options, process_thread},
    publication,
    show_extraction::{ShowEvidence, ShowExtraction, ShowProject},
    store::{ExtractionMode, Store},
};
struct Api {
    body: String,
    comment: String,
}
impl HnApi for Api {
    fn fetch_item(&self, id: i64) -> Result<Option<Value>, FetchError> {
        Ok(Some(match id {
            100 => {
                json!({"id":100,"type":"story","title":"Show HN: Alpha","url":"https://alpha.test","text":self.body,"by":"alice","time":1000,"kids":[101]})
            }
            101 => {
                json!({"id":101,"type":"comment","parent":100,"by":"bob","time":1001,"text":self.comment})
            }
            _ => panic!(),
        }))
    }
}
struct Mock {
    calls: AtomicUsize,
    outputs: Mutex<Vec<Result<ShowExtraction, String>>>,
}
impl Model for Mock {
    fn name(&self) -> &str {
        "show-fixture"
    }
    fn extract(&self, _: &str) -> Result<Extraction, String> {
        panic!("monthly called")
    }
    fn extract_show(&self, _: &str) -> Result<ShowExtraction, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.outputs.lock().unwrap().remove(0)
    }
}
fn model(outputs: Vec<Result<ShowExtraction, String>>) -> Mock {
    Mock {
        calls: AtomicUsize::new(0),
        outputs: Mutex::new(outputs),
    }
}
fn output() -> ShowExtraction {
    ShowExtraction {
        project: Some(ShowProject {
            mention: Mention {
                source_item_id: 100,
                name: Some("Alpha".into()),
                urls: vec!["https://alpha.test".into()],
                description: "Alpha".into(),
                problem: None,
                audience: None,
                source_excerpt: "Show HN: Alpha".into(),
                builders: vec![],
                categories: vec![],
                audience_labels: vec![],
            },
            evidence: vec![
                ShowEvidence {
                    source_item_id: 100,
                    source_field: "title".into(),
                    value: "Show HN: Alpha".into(),
                    supports: vec!["name".into(), "description".into()],
                    authorization_source_item_id: None,
                },
                ShowEvidence {
                    source_item_id: 100,
                    source_field: "url".into(),
                    value: "https://alpha.test".into(),
                    supports: vec!["urls".into()],
                    authorization_source_item_id: None,
                },
            ],
        }),
        no_project_reason: None,
    }
}
fn setup() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("db")).unwrap();
    collect(&mut store, "", "Other project speculation");
    store
        .assign_extraction_mode(100, ExtractionMode::ShowHn)
        .unwrap();
    (dir, store)
}
fn collect(store: &mut Store, body: &str, comment: &str) {
    Collector::new(
        Api {
            body: body.into(),
            comment: comment.into(),
        },
        CollectionConfig::default(),
    )
    .collect(store, 100)
    .unwrap();
}
#[test]
fn title_link_evidence_persists_and_unchanged_skips() {
    let (dir, mut store) = setup();
    let m = model(vec![Ok(output())]);
    assert_eq!(
        process_thread(&mut store, 100, &m, false)
            .unwrap()
            .processed,
        1
    );
    assert_eq!(
        process_thread(&mut store, 100, &m, false).unwrap().skipped,
        1
    );
    assert_eq!(m.calls.load(Ordering::SeqCst), 1);
    publication::approve_thread(&mut store, 100).unwrap();
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert_eq!(
        db(&dir)
            .query_row("SELECT count(*) FROM mention_evidence", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}
#[test]
fn unauthorized_comment_and_invalid_fields_never_replace() {
    let (dir, mut store) = setup();
    process_thread(&mut store, 100, &model(vec![Ok(output())]), false).unwrap();
    let mut bad = output();
    bad.project.as_mut().unwrap().evidence.push(ShowEvidence {
        source_item_id: 101,
        source_field: "body".into(),
        value: "Other project".into(),
        supports: vec!["description".into()],
        authorization_source_item_id: None,
    });
    let m = model(vec![Ok(bad.clone()), Ok(bad.clone()), Ok(bad)]);
    let report = process_thread(&mut store, 100, &m, true).unwrap();
    assert_eq!(report.failed.len(), 1);
    assert_eq!(
        db(&dir)
            .query_row("SELECT count(*) FROM mentions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}
#[test]
fn zero_result_retires_previous_and_preserves_project() {
    let (dir, mut store) = setup();
    process_thread(&mut store, 100, &model(vec![Ok(output())]), false).unwrap();
    let zero = ShowExtraction {
        project: None,
        no_project_reason: Some("Insufficient project information".into()),
    };
    assert_eq!(
        process_thread(&mut store, 100, &model(vec![Ok(zero)]), true)
            .unwrap()
            .processed,
        1
    );
    assert_eq!(
        db(&dir)
            .query_row("SELECT count(*) FROM mentions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db(&dir)
            .query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}
#[test]
fn named_builder_requires_submitter_authorization() {
    let (_dir, mut store) = setup();
    collect(&mut store, "I built Alpha with bob", "Alpha works offline");
    let mut good = output();
    let p = good.project.as_mut().unwrap();
    p.mention.builders.push("bob".into());
    p.evidence.push(ShowEvidence {
        source_item_id: 100,
        source_field: "body".into(),
        value: "I built Alpha with bob".into(),
        supports: vec!["builders".into()],
        authorization_source_item_id: None,
    });
    p.evidence.push(ShowEvidence {
        source_item_id: 101,
        source_field: "body".into(),
        value: "Alpha works offline".into(),
        supports: vec!["description".into()],
        authorization_source_item_id: Some(100),
    });
    assert_eq!(
        process_thread(&mut store, 100, &model(vec![Ok(good)]), false)
            .unwrap()
            .processed,
        1
    );
}
#[test]
fn suppressed_support_invalidates_cache_and_cannot_return() {
    let (dir, mut store) = setup();
    process_thread(&mut store, 100, &model(vec![Ok(output())]), false).unwrap();
    db(&dir)
        .execute("INSERT INTO source_suppression VALUES(101,'remove')", [])
        .unwrap();
    let m = model(vec![Ok(output())]);
    assert_eq!(
        process_thread(&mut store, 100, &m, false)
            .unwrap()
            .processed,
        1
    );
    assert_eq!(m.calls.load(Ordering::SeqCst), 1);
}
#[test]
fn oversized_item_fails_explicitly_without_model_calls() {
    let (_dir, mut store) = setup();
    collect(&mut store, "", &"x".repeat(35000));
    let m = model(vec![]);
    let report = process_thread(&mut store, 100, &m, false).unwrap();
    assert_eq!(report.failed.len(), 1);
    assert_eq!(m.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn invalid_multi_project_response_rejected_by_schema() {
    let value = json!({"projects":[{},{}],"no_project_reason":null});
    assert!(serde_json::from_value::<ShowExtraction>(value).is_err());
}
#[test]
fn transport_failure_retains_previous_result() {
    let (dir, mut store) = setup();
    process_thread(&mut store, 100, &model(vec![Ok(output())]), false).unwrap();
    let m = model(vec![Err("permanent failure".into())]);
    let report = wrkgn::processing::process_thread_with(
        &mut store,
        100,
        &m,
        true,
        &Options::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.failed.len(), 1);
    assert_eq!(
        db(&dir)
            .query_row("SELECT count(*) FROM mentions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

fn db(dir: &tempfile::TempDir) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.path().join("db")).unwrap()
}
struct BranchApi;
impl HnApi for BranchApi {
    fn fetch_item(&self, id: i64) -> Result<Option<Value>, FetchError> {
        Ok(Some(if id == 100 {
            json!({"id":100,"type":"story","title":"Show HN: Alpha","url":"https://alpha.test","by":"alice","time":1000,"kids":[101,102,103]})
        } else {
            json!({"id":id,"type":"comment","parent":100,"by":"bob","time":1001,"text":"x".repeat(16000)})
        }))
    }
}
#[test]
fn oversized_tree_chunks_resume_and_consolidate() {
    let (dir, mut store) = setup();
    Collector::new(BranchApi, CollectionConfig::default())
        .collect(&mut store, 100)
        .unwrap();
    let m = model(vec![
        Ok(output()),
        Err("temporary fixture interruption".into()),
    ]);
    assert_eq!(
        process_thread(&mut store, 100, &m, false)
            .unwrap()
            .failed
            .len(),
        1
    );
    assert_eq!(
        db(&dir)
            .query_row("SELECT count(*) FROM mentions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let m = model(vec![Ok(output()), Ok(output()), Ok(output()), Ok(output())]);
    assert_eq!(
        process_thread(&mut store, 100, &m, false)
            .unwrap()
            .processed,
        1
    );
    assert_eq!(m.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        db(&dir)
            .query_row("SELECT count(*) FROM mentions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}
struct MonthlyApi;
impl HnApi for MonthlyApi {
    fn fetch_item(&self, id: i64) -> Result<Option<Value>, FetchError> {
        Ok(Some(match id {
            200 => json!({"id":200,"kids":[201]}),
            201 => {
                json!({"id":201,"type":"comment","parent":200,"by":"alice","time":900,"text":"I built Alpha https://alpha.test","kids":[202]})
            }
            202 => {
                json!({"id":202,"type":"comment","parent":201,"by":"other","time":901,"text":"Hi"})
            }
            _ => panic!(),
        }))
    }
}
struct MonthlyModel;
impl Model for MonthlyModel {
    fn name(&self) -> &str {
        "monthly-fixture"
    }
    fn extract(&self, _: &str) -> Result<Extraction, String> {
        let mut m = output().project.unwrap().mention;
        m.source_item_id = 201;
        m.source_excerpt = "Alpha".into();
        Ok(Extraction { mentions: vec![m] })
    }
}
#[test]
fn cross_mode_matching_dates_comments_and_stable_slug() {
    let (dir, mut store) = setup();
    Collector::new(MonthlyApi, CollectionConfig::default())
        .collect(&mut store, 200)
        .unwrap();
    process_thread(&mut store, 200, &MonthlyModel, false).unwrap();
    let slug: String = db(&dir)
        .query_row("SELECT slug FROM projects", [], |r| r.get(0))
        .unwrap();
    process_thread(&mut store, 100, &model(vec![Ok(output())]), false).unwrap();
    assert_eq!(
        db(&dir)
            .query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    publication::approve_thread(&mut store, 100).unwrap();
    publication::approve_thread(&mut store, 200).unwrap();
    let path = dir.path().join("public");
    let version = publication::export(&mut store, &path).unwrap();
    let discovery: Value = serde_json::from_slice(
        &std::fs::read(path.join("snapshots").join(version).join("discovery.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(discovery[0]["comment_count"], 2);
    assert_eq!(discovery[0]["latest_shared_at"], 1000);
    assert_eq!(discovery[0]["slug"], slug);
}
#[test]
fn failed_support_edit_hides_stale_export() {
    let (dir, mut store) = setup();
    process_thread(&mut store, 100, &model(vec![Ok(output())]), false).unwrap();
    publication::approve_thread(&mut store, 100).unwrap();
    collect(&mut store, "", "edited discussion");
    process_thread(
        &mut store,
        100,
        &model(vec![Err("failed fixture".into())]),
        false,
    )
    .unwrap();
    let path = dir.path().join("public");
    let version = publication::export(&mut store, &path).unwrap();
    let discovery: Value = serde_json::from_slice(
        &std::fs::read(path.join("snapshots").join(version).join("discovery.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(discovery.as_array().unwrap().len(), 0);
}
#[test]
fn bounded_consolidation_fails_without_partial_publication() {
    let (dir, mut store) = setup();
    Collector::new(BranchApi, CollectionConfig::default())
        .collect(&mut store, 100)
        .unwrap();
    let mut huge = output();
    let record = huge.project.as_ref().unwrap().evidence[0].clone();
    huge.project
        .as_mut()
        .unwrap()
        .evidence
        .extend(std::iter::repeat_n(record, 400));
    let m = model(vec![Ok(huge.clone()), Ok(huge.clone()), Ok(huge)]);
    let report = process_thread(&mut store, 100, &m, false).unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].1.contains("consolidation input exceeds"));
    assert_eq!(
        db(&dir)
            .query_row("SELECT count(*) FROM mentions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn validator_rejects_invented_sources_fields_urls_and_quotations() {
    let (_dir, store) = setup();
    let tree = store.conversation_groups(100).unwrap();
    let mut all = vec![&tree.thread];
    for group in &tree.groups {
        wrkgn::extraction::flatten(group, &mut all)
    }
    let sources = all.iter().map(|i| (i.source.item_id, *i)).collect();
    let ids = all.iter().map(|i| i.source.item_id).collect();
    let mut cases = vec![];
    let mut bad = output();
    bad.project.as_mut().unwrap().evidence[0].source_item_id = 999;
    cases.push(bad);
    let mut bad = output();
    bad.project.as_mut().unwrap().evidence[0].source_field = "missing".into();
    cases.push(bad);
    let mut bad = output();
    bad.project.as_mut().unwrap().evidence[0].value = "invented".into();
    cases.push(bad);
    let mut bad = output();
    bad.project.as_mut().unwrap().mention.urls = vec!["javascript:alert(1)".into()];
    cases.push(bad);
    for case in cases {
        assert!(wrkgn::show_extraction::validate(&case, 100, &sources, &ids).is_err())
    }
}
