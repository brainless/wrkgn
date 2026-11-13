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
    assert_eq!(m.calls.load(Ordering::SeqCst), 4);
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

#[test]
fn urls_require_complete_tokens_in_evidence_and_source_fields() {
    let (_dir, mut store) = setup();
    for (body, evidence, url, valid) in [
        ("", "https://alpha.test", "https://alpha.test", true),
        ("", "https://alpha.test", "https://alpha.tes", false),
        (
            "Project: https://github.com/alice/alpha.",
            "https://github.com/alice/alpha",
            "https://github.com/alice/alpha",
            true,
        ),
        (
            "Project: https://github.com/alice/alpha.",
            "https://github.com/alice/alpha",
            "https://github.com/alice",
            false,
        ),
        (
            "Project: https://github.com/alice/alpha.",
            "https://github.com/alice",
            "https://github.com/alice",
            false,
        ),
        (
            "Project: https://alpha.test",
            "https://alpha.tes",
            "https://alpha.tes",
            false,
        ),
        (
            "Try https://alpha.test/demo and https://alpha.test/examples/one",
            "Try https://alpha.test/demo and https://alpha.test/examples/one",
            "https://alpha.test",
            true,
        ),
        (
            "Try https://github.com/alice/alpha and https://github.com/alice/beta",
            "Try https://github.com/alice/alpha and https://github.com/alice/beta",
            "https://github.com",
            false,
        ),
    ] {
        collect(&mut store, body, "Other project speculation");
        let tree = store.conversation_groups(100).unwrap();
        let mut all = vec![&tree.thread];
        for group in &tree.groups {
            wrkgn::extraction::flatten(group, &mut all);
        }
        let sources = all.iter().map(|i| (i.source.item_id, *i)).collect();
        let ids = all.iter().map(|i| i.source.item_id).collect();
        let mut result = output();
        let project = result.project.as_mut().unwrap();
        project.mention.urls = vec![url.into()];
        project.evidence[1].source_field = if body.is_empty() { "url" } else { "body" }.into();
        project.evidence[1].value = evidence.into();
        assert_eq!(
            wrkgn::show_extraction::validate(&result, 100, &sources, &ids).is_ok(),
            valid,
            "URL {url:?}, evidence {evidence:?}, source {body:?}"
        );
    }
}

#[derive(Clone)]
struct CacheApi {
    items: std::collections::BTreeMap<i64, Value>,
}
impl CacheApi {
    fn branches() -> Self {
        let mut items = std::collections::BTreeMap::new();
        for id in [100, 101, 102, 103] {
            items.insert(id, BranchApi.fetch_item(id).unwrap().unwrap());
        }
        Self { items }
    }
    fn collect(&self, store: &mut Store) {
        let report = Collector::new(self.clone(), CollectionConfig::default())
            .collect(store, 100)
            .unwrap();
        assert!(report.complete);
    }
    fn add(&mut self, id: i64, parent: i64, author: &str, body: &str) {
        self.items.insert(
            id,
            json!({"id":id,"type":"comment","parent":parent,"by":author,"time":1001,"text":body}),
        );
        self.items
            .get_mut(&parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .entry("kids")
            .or_insert(json!([]))
            .as_array_mut()
            .unwrap()
            .push(json!(id));
    }
}
impl HnApi for CacheApi {
    fn fetch_item(&self, id: i64) -> Result<Option<Value>, FetchError> {
        Ok(self.items.get(&id).cloned())
    }
}
#[derive(Default)]
struct CacheModel {
    prompts: Mutex<Vec<Value>>,
}
impl CacheModel {
    fn extraction_ids(&self) -> Vec<Vec<i64>> {
        self.prompts
            .lock()
            .unwrap()
            .iter()
            .filter_map(|p| p.get("sources"))
            .map(|sources| {
                sources
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s["id"].as_i64().unwrap())
                    .collect()
            })
            .collect()
    }
    fn consolidation_count(&self) -> usize {
        self.prompts
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.get("consolidate").is_some())
            .count()
    }
}
impl Model for CacheModel {
    fn name(&self) -> &str {
        "cache-fixture"
    }
    fn extract(&self, _: &str) -> Result<Extraction, String> {
        panic!("monthly called")
    }
    fn extract_show(&self, prompt: &str) -> Result<ShowExtraction, String> {
        self.prompts
            .lock()
            .unwrap()
            .push(serde_json::from_str(prompt).unwrap());
        Ok(output())
    }
}
fn cache_process(store: &mut Store, reprocess: bool) -> CacheModel {
    let model = CacheModel::default();
    let report = process_thread(store, 100, &model, reprocess).unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    model
}
#[test]
fn independent_reply_edits_and_additions_reuse_unaffected_chunks() {
    let (_dir, mut store) = setup();
    let mut api = CacheApi::branches();
    api.collect(&mut store);
    let first = cache_process(&mut store, false);
    assert_eq!(
        first.extraction_ids(),
        [vec![100, 101], vec![100, 102], vec![100, 103]]
    );
    assert_eq!(first.consolidation_count(), 2);

    api.items.get_mut(&103).unwrap()["text"] = json!("y".repeat(16000));
    api.collect(&mut store);
    let edited = cache_process(&mut store, false);
    assert_eq!(edited.extraction_ids(), [vec![100, 103]]);
    assert_eq!(edited.consolidation_count(), 1);

    api.add(104, 100, "bob", &"z".repeat(16000));
    api.collect(&mut store);
    let added = cache_process(&mut store, false);
    assert_eq!(added.extraction_ids(), [vec![100, 104]]);
    assert_eq!(added.consolidation_count(), 2);
    assert!(
        cache_process(&mut store, false)
            .prompts
            .lock()
            .unwrap()
            .is_empty()
    );

    let forced = cache_process(&mut store, true);
    assert_eq!(forced.extraction_ids().len(), 4);
    assert_eq!(forced.consolidation_count(), 3);
}
#[test]
fn repeated_submitter_context_edits_and_suppression_invalidate_dependent_chunks() {
    let (_dir, mut store) = setup();
    let mut api = CacheApi::branches();
    api.add(105, 100, "alice", "I built Alpha with bob");
    api.collect(&mut store);
    assert_eq!(cache_process(&mut store, false).extraction_ids().len(), 3);

    api.items.get_mut(&105).unwrap()["text"] = json!("I built Alpha without bob");
    api.collect(&mut store);
    let edited = cache_process(&mut store, false);
    assert_eq!(edited.extraction_ids().len(), 3);
    assert!(edited.extraction_ids().iter().all(|ids| ids.contains(&105)));

    publication::suppress_source(&mut store, 105, "remove authorization").unwrap();
    let suppressed = cache_process(&mut store, false);
    assert_eq!(suppressed.extraction_ids().len(), 3);
    assert!(
        suppressed
            .extraction_ids()
            .iter()
            .all(|ids| !ids.contains(&105))
    );
}
#[test]
fn ancestor_edits_and_suppression_only_invalidate_its_descendant_chunks() {
    let (_dir, mut store) = setup();
    let mut api = CacheApi::branches();
    api.items.get_mut(&100).unwrap()["kids"] = json!([101, 103]);
    api.items.get_mut(&101).unwrap()["text"] = json!("Question context");
    api.items.get_mut(&101).unwrap()["kids"] = json!([102]);
    api.items.get_mut(&102).unwrap()["parent"] = json!(101);
    api.add(104, 101, "bob", &"x".repeat(16000));
    api.collect(&mut store);
    assert_eq!(cache_process(&mut store, false).extraction_ids().len(), 3);

    api.items.get_mut(&101).unwrap()["text"] = json!("Edited question context");
    api.collect(&mut store);
    let edited = cache_process(&mut store, false);
    assert_eq!(
        edited.extraction_ids(),
        [vec![100, 101, 102], vec![100, 101, 104]]
    );

    publication::suppress_source(&mut store, 101, "remove context").unwrap();
    let suppressed = cache_process(&mut store, false);
    assert_eq!(
        suppressed.extraction_ids(),
        [vec![100, 102], vec![100, 104]]
    );
}

struct MetadataModel {
    expected: std::collections::BTreeMap<i64, Value>,
    consolidation_calls: AtomicUsize,
}
impl Model for MetadataModel {
    fn name(&self) -> &str {
        "consolidation-metadata-fixture"
    }
    fn extract(&self, _: &str) -> Result<Extraction, String> {
        panic!("monthly called")
    }
    fn extract_show(&self, prompt: &str) -> Result<ShowExtraction, String> {
        assert!(prompt.len() <= wrkgn::extraction::MAX_INPUT_BYTES);
        let prompt: Value = serde_json::from_str(prompt).unwrap();
        if let Some(inputs) = prompt.get("consolidate") {
            self.consolidation_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(prompt["submitter_author"], "alice");
            let inputs: Vec<ShowExtraction> = serde_json::from_value(inputs.clone()).unwrap();
            let mut combined = inputs[0].clone();
            let project = combined.project.as_mut().unwrap();
            for input in &inputs[1..] {
                for evidence in &input.project.as_ref().unwrap().evidence {
                    if !project.evidence.iter().any(|existing| {
                        serde_json::to_value(existing).unwrap()
                            == serde_json::to_value(evidence).unwrap()
                    }) {
                        project.evidence.push(evidence.clone());
                    }
                }
            }
            let mut needed = std::collections::BTreeSet::from([100]);
            for evidence in &project.evidence {
                needed.insert(evidence.source_item_id);
                needed.extend(evidence.authorization_source_item_id);
            }
            let metadata = prompt["source_metadata"].as_array().unwrap();
            assert_eq!(metadata.len(), needed.len());
            let mut previous = None;
            for source in metadata {
                let id = source["id"].as_i64().unwrap();
                assert!(needed.remove(&id));
                let expected = &self.expected[&id];
                assert_eq!(source["author"], expected["by"]);
                assert_eq!(source["created_at"], expected["time"]);
                assert_eq!(source["parent_id"], expected["parent"]);
                assert!(source.get("body").is_none());
                assert!(source.get("text").is_none());
                let key = (source["created_at"].as_i64().unwrap(), id);
                assert!(previous.is_none_or(|previous| previous <= key));
                previous = Some(key);
            }
            assert!(needed.is_empty());
            // The lower ID is the later submitter correction. Builder evidence
            // remains separately attributed and carries submitter authorization.
            let correction = metadata.iter().find(|s| s["id"] == 104).unwrap();
            let original = metadata.iter().find(|s| s["id"] == 105).unwrap();
            assert_eq!(correction["author"], "alice");
            for evidence in &project.evidence {
                if let Some(authorization) = evidence.authorization_source_item_id {
                    let source = metadata
                        .iter()
                        .find(|s| s["id"] == evidence.source_item_id)
                        .unwrap();
                    let auth = metadata.iter().find(|s| s["id"] == authorization).unwrap();
                    assert_eq!(source["author"], "bob");
                    assert_eq!(auth["author"], prompt["submitter_author"]);
                }
            }
            project.mention.description = if correction["created_at"].as_i64().unwrap()
                > original["created_at"].as_i64().unwrap()
            {
                "Alpha works offline".into()
            } else {
                // Equal dates do not establish which statement supersedes the
                // other. Keep the undisputed project identity only.
                "Alpha".into()
            };
            return Ok(combined);
        }
        let mut result = output();
        let project = result.project.as_mut().unwrap();
        project.mention.builders = vec!["bob".into()];
        for (id, value, supports, authorization) in [
            (100, "I built Alpha with bob", "builders", None),
            (104, "Correction: Alpha works offline.", "description", None),
            (105, "Alpha requires a server.", "description", None),
        ] {
            project.evidence.push(ShowEvidence {
                source_item_id: id,
                source_field: "body".into(),
                value: value.into(),
                supports: vec![supports.into()],
                authorization_source_item_id: authorization,
            });
        }
        for source in prompt["sources"].as_array().unwrap() {
            if source["author"] == "bob" {
                project.evidence.push(ShowEvidence {
                    source_item_id: source["id"].as_i64().unwrap(),
                    source_field: "body".into(),
                    value: "Alpha supports sync.".into(),
                    supports: vec!["description".into()],
                    authorization_source_item_id: Some(100),
                });
            }
        }
        Ok(result)
    }
}
#[test]
fn every_consolidation_level_receives_source_chronology_and_authorization_metadata() {
    for (correction_time, expected_description) in [(2200, "Alpha works offline"), (1200, "Alpha")]
    {
        let (dir, mut store) = setup();
        let mut api = CacheApi::branches();
        api.items.get_mut(&100).unwrap()["text"] = json!("I built Alpha with bob");
        api.add(104, 100, "alice", "Correction: Alpha works offline.");
        api.add(105, 100, "alice", "Alpha requires a server.");
        api.items.get_mut(&104).unwrap()["time"] = json!(correction_time);
        api.items.get_mut(&105).unwrap()["time"] = json!(1200);
        for id in [101, 102, 103] {
            api.items.get_mut(&id).unwrap()["text"] =
                json!(format!("Alpha supports sync. {}", "x".repeat(16000)));
            api.items.get_mut(&id).unwrap()["time"] = json!(1300 + id);
        }
        api.collect(&mut store);
        let model = MetadataModel {
            expected: api.items,
            consolidation_calls: AtomicUsize::new(0),
        };
        let report = process_thread(&mut store, 100, &model, false).unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(model.consolidation_calls.load(Ordering::SeqCst), 2);
        let description: String = db(&dir)
            .query_row("SELECT description FROM mentions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(description, expected_description);
    }
}

#[test]
fn consolidation_bound_includes_metadata_and_prompt_wrapper() {
    let (dir, mut store) = setup();
    Collector::new(BranchApi, CollectionConfig::default())
        .collect(&mut store, 100)
        .unwrap();
    let mut bounded = output();
    let record = bounded.project.as_ref().unwrap().evidence[0].clone();
    loop {
        let mut candidate = bounded.clone();
        candidate
            .project
            .as_mut()
            .unwrap()
            .evidence
            .push(record.clone());
        let inputs = serde_json::to_string(&[&candidate, &candidate]).unwrap();
        if inputs.len() > wrkgn::extraction::MAX_INPUT_BYTES - 128 {
            break;
        }
        bounded = candidate;
    }
    // The intermediate results alone fit. Source metadata and the prompt
    // wrapper must be included before deciding whether the model may be called.
    assert!(
        serde_json::to_string(&[&bounded, &bounded]).unwrap().len()
            < wrkgn::extraction::MAX_INPUT_BYTES
    );
    let m = model(vec![Ok(bounded.clone()), Ok(bounded.clone()), Ok(bounded)]);
    let report = process_thread(&mut store, 100, &m, false).unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].1.contains("consolidation input exceeds"));
    assert_eq!(m.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        db(&dir)
            .query_row("SELECT count(*) FROM mentions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

fn pending_cli(dir: &tempfile::TempDir, json: bool) -> String {
    struct NoNetwork;
    impl HnApi for NoNetwork {
        fn fetch_item(&self, _: i64) -> Result<Option<Value>, FetchError> {
            panic!("review must not fetch HN")
        }
    }
    let mut args = vec!["review".into(), "pending".into()];
    if json {
        args.push("--json".into());
    }
    args.extend([
        "--db".into(),
        dir.path().join("db").to_str().unwrap().into(),
    ]);
    let mut out = vec![];
    let mut err = vec![];
    let status = wrkgn::cli::run_with_api(&args, NoNetwork, &mut out, &mut err).unwrap();
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&err));
    assert!(err.is_empty());
    String::from_utf8(out).unwrap()
}

#[test]
fn pending_review_exposes_supporting_evidence_and_submitter_authorization() {
    let (dir, mut store) = setup();
    let mut api = CacheApi::branches();
    for (id, author, body, date) in [
        (101, "alice", "Alpha works offline", 1_704_153_600),
        (102, "alice", "I built Alpha with bob", 1_704_240_000),
        (103, "bob", "Alpha syncs folders", 1_704_326_400),
    ] {
        api.items.insert(
            id,
            json!({"id":id,"type":"comment","parent":100,"by":author,"time":date,"text":body}),
        );
    }
    api.collect(&mut store);
    let mut result = output();
    let project = result.project.as_mut().unwrap();
    project.mention.description = "Alpha works offline and syncs folders".into();
    project.mention.builders = vec!["bob".into()];
    project.evidence[0].supports = vec!["name".into()];
    for (id, value, supports, authorization) in [
        (101, "Alpha works offline", "description", None),
        (102, "I built Alpha with bob", "builders", None),
        (103, "Alpha syncs folders", "description", Some(102)),
    ] {
        project.evidence.push(ShowEvidence {
            source_item_id: id,
            source_field: "body".into(),
            value: value.into(),
            supports: vec![supports.into()],
            authorization_source_item_id: authorization,
        });
    }
    let report = process_thread(&mut store, 100, &model(vec![Ok(result)]), false).unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    publication::correct(
        &mut store,
        "100:sole",
        "description",
        Some("Reviewed factual description"),
    )
    .unwrap();
    let cards = pending_cli(&dir, false);
    let mentions = publication::pending_mentions(&mut store).unwrap();
    let narrow = wrkgn::review_grid::render(&mentions, 60);
    for line in narrow.lines().filter(|line| line.starts_with('│')) {
        assert!(line.chars().count() <= 60, "overwide review row: {line}");
    }
    for expected in [
        "Reviewed factual description",
        "Alpha works offline",
        "I built Alpha with bob",
        "Alpha syncs folders",
        "@alice",
        "@bob",
        "2024-01-02",
        "2024-01-03",
        "2024-01-04",
        "item?id=101",
        "item?id=102",
        "item?id=103",
        "Supports",
        "builders",
        "Auth source",
    ] {
        assert!(cards.contains(expected), "missing {expected:?} in {cards}");
    }
    let json: Value = serde_json::from_str(pending_cli(&dir, true).trim()).unwrap();
    assert_eq!(json["key"], "100:sole");
    assert_eq!(json["description"], "Reviewed factual description");
    assert_eq!(json["source_item_id"], 100);
    assert_eq!(json["shared_at"], 1000);
    let evidence = json["evidence"].as_array().unwrap();
    assert_eq!(evidence.len(), 5);
    let builder = evidence
        .iter()
        .find(|e| e["source_item_id"] == 103)
        .unwrap();
    assert_eq!(builder["author"], "bob");
    assert_eq!(builder["shared_at"], 1_704_326_400_i64);
    assert_eq!(builder["authorization_source_item_id"], 102);
    assert_eq!(builder["supports"], json!(["description"]));
    assert_eq!(
        builder["source_url"],
        "https://news.ycombinator.com/item?id=103"
    );
    assert!(
        evidence
            .iter()
            .any(|e| e["source_item_id"] == 102 && e["value"] == "I built Alpha with bob")
    );
    for private in [
        "fingerprint",
        "raw_payload",
        "source_text",
        "processing_failures",
    ] {
        assert!(!json.to_string().contains(private));
    }
    assert_eq!(publication::pending_mentions(&mut store).unwrap().len(), 1);
}

#[test]
fn pending_review_shows_title_and_link_evidence_without_manufacturing_comment_text() {
    let (dir, mut store) = setup();
    process_thread(&mut store, 100, &model(vec![Ok(output())]), false).unwrap();
    let cards = pending_cli(&dir, false);
    assert!(cards.contains("title · @alice"));
    assert!(cards.contains("url · @alice"));
    assert!(cards.contains("Show HN: Alpha"));
    assert!(cards.contains("https://alpha.test"));
    assert!(!cards.contains("Comment"));
    assert!(!cards.contains("Other project speculation"));
    let json: Value = serde_json::from_str(pending_cli(&dir, true).trim()).unwrap();
    assert_eq!(json["evidence"][0]["source_field"], "title");
    assert_eq!(json["evidence"][1]["source_field"], "url");
}

#[test]
fn monthly_pending_review_keeps_legacy_excerpt_without_invented_field_support() {
    let (dir, mut store) = setup();
    Collector::new(MonthlyApi, CollectionConfig::default())
        .collect(&mut store, 200)
        .unwrap();
    process_thread(&mut store, 200, &MonthlyModel, false).unwrap();
    let cards = pending_cli(&dir, false);
    assert!(cards.contains("Evidence"));
    assert!(cards.contains("Comment"));
    assert!(cards.contains("I built Alpha"));
    assert!(!cards.contains("Supports"));
    let json: Value = serde_json::from_str(pending_cli(&dir, true).trim()).unwrap();
    assert_eq!(json["source_item_id"], 201);
    assert_eq!(json["evidence"], json!([]));
}
