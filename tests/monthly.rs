use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tempfile::tempdir;
use wrkgn::collector::{CollectionConfig, Collector, FetchError, HnApi};
use wrkgn::extraction::{Extraction, Mention};
use wrkgn::monthly::sync_thread;
use wrkgn::processing::{Model, Options};
use wrkgn::{publication, store::Store};

struct Api {
    reply: AtomicBool,
    incomplete: AtomicBool,
}
impl HnApi for Api {
    fn fetch_item(&self, id: i64) -> Result<Option<Value>, FetchError> {
        if id == 102 && self.incomplete.load(Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(Some(match id {
            100 => json!({"id":100,"type":"story","kids":[101]}),
            101 => json!({"id":101,"parent":100,"by":"alice","time":1000,
                "text":"Building Alpha https://alpha.test", "kids":if self.reply.load(Ordering::SeqCst) {vec![102]} else {vec![]}}),
            102 => json!({"id":102,"parent":101,"by":"bob","time":1001,"text":"Nice work"}),
            200 => json!({"id":200,"type":"story","kids":[201]}),
            201 => json!({"id":201,"parent":200,"by":"alice","time":2000,
                "text":"Building Alpha https://alpha.test"}),
            _ => panic!("unexpected item {id}"),
        }))
    }
}
struct Mock {
    source: i64,
    calls: AtomicUsize,
    fail: AtomicBool,
}
impl Model for Mock {
    fn name(&self) -> &str {
        "monthly-fixture"
    }
    fn extract(&self, _: &str) -> Result<Extraction, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err("fixture extraction failure".into());
        }
        Ok(Extraction {
            mentions: vec![Mention {
                source_item_id: self.source,
                name: Some("Alpha".into()),
                urls: vec!["https://alpha.test".into()],
                description: "Alpha helps organize work".into(),
                problem: None,
                audience: None,
                source_excerpt: "Building Alpha".into(),
                builders: vec![],
                categories: vec![],
                audience_labels: vec![],
            }],
        })
    }
}
fn model(source: i64) -> Mock {
    Mock {
        source,
        calls: AtomicUsize::new(0),
        fail: AtomicBool::new(false),
    }
}
fn collector() -> Collector<Api> {
    Collector::new(
        Api {
            reply: AtomicBool::new(false),
            incomplete: AtomicBool::new(false),
        },
        CollectionConfig::default(),
    )
}
fn discovery(export: &std::path::Path) -> Value {
    let pointer: Value =
        serde_json::from_slice(&std::fs::read(export.join("current.json")).unwrap()).unwrap();
    let version = pointer["version"].as_str().unwrap();
    serde_json::from_slice(
        &std::fs::read(export.join(format!("snapshots/{version}/discovery.json"))).unwrap(),
    )
    .unwrap()
}

#[test]
fn automatic_publication_is_thread_scoped_incremental_and_keeps_review_controls() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(dir.path().join("db.sqlite")).unwrap();
    let export = dir.path().join("export");
    let collector = collector();
    let first = model(101);
    let second = model(201);
    let options = Options::default();
    // A pending mention in another thread must stay pending.
    collector.collect(&mut store, 200).unwrap();
    wrkgn::processing::process_thread(&mut store, 200, &second, false).unwrap();
    let key = publication::pending_mentions(&mut store).unwrap()[0]
        .key
        .clone();
    publication::correct(
        &mut store,
        &key,
        "description",
        Some("Corrected Alpha description"),
    )
    .unwrap();
    sync_thread(
        &mut store,
        &collector,
        &first,
        100,
        &options,
        &export,
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(publication::pending_mentions(&mut store).unwrap().len(), 1);
    assert_eq!(discovery(&export).as_array().unwrap().len(), 1);
    // Same source and model reuse existing processing; auto-approval is idempotent.
    sync_thread(
        &mut store,
        &collector,
        &first,
        100,
        &options,
        &export,
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(first.calls.load(Ordering::SeqCst), 1);
    // Reuse recent author/URL matching to combine mentions across threads.
    sync_thread(
        &mut store,
        &collector,
        &second,
        200,
        &options,
        &export,
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(second.calls.load(Ordering::SeqCst), 1);
    assert!(
        publication::pending_mentions(&mut store)
            .unwrap()
            .is_empty()
    );
    let projects = discovery(&export);
    assert_eq!(projects.as_array().unwrap().len(), 1);
    assert_eq!(projects[0]["description"], "Corrected Alpha description");
    publication::suppress_source(&mut store, 101, "removal").unwrap();
    publication::suppress_source(&mut store, 201, "removal").unwrap();
    sync_thread(
        &mut store,
        &collector,
        &first,
        100,
        &options,
        &export,
        &mut Vec::new(),
    )
    .unwrap();
    assert!(discovery(&export).as_array().unwrap().is_empty());
}

#[test]
fn new_replies_reprocess_their_group_and_failures_resume_without_approval() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(dir.path().join("db.sqlite")).unwrap();
    let export = dir.path().join("export");
    let api = Api {
        reply: AtomicBool::new(false),
        incomplete: AtomicBool::new(false),
    };
    // Use shared atomic state in the fixture transport.
    struct Shared<'a>(&'a Api);
    impl HnApi for Shared<'_> {
        fn fetch_item(&self, id: i64) -> Result<Option<Value>, FetchError> {
            self.0.fetch_item(id)
        }
    }
    let collector = Collector::new(Shared(&api), CollectionConfig::default());
    let model = model(101);
    let options = Options::default();
    sync_thread(
        &mut store,
        &collector,
        &model,
        100,
        &options,
        &export,
        &mut Vec::new(),
    )
    .unwrap();
    api.reply.store(true, Ordering::SeqCst);
    api.incomplete.store(true, Ordering::SeqCst);
    assert!(
        sync_thread(
            &mut store,
            &collector,
            &model,
            100,
            &options,
            &export,
            &mut Vec::new()
        )
        .unwrap_err()
        .contains("collection incomplete")
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    api.incomplete.store(false, Ordering::SeqCst);
    model.fail.store(true, Ordering::SeqCst);
    assert!(
        sync_thread(
            &mut store,
            &collector,
            &model,
            100,
            &options,
            &export,
            &mut Vec::new()
        )
        .unwrap_err()
        .contains("processing incomplete")
    );
    assert!(discovery(&export).as_array().unwrap().is_empty());
    model.fail.store(false, Ordering::SeqCst);
    sync_thread(
        &mut store,
        &collector,
        &model,
        100,
        &options,
        &export,
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(model.calls.load(Ordering::SeqCst), 3);
    assert_eq!(discovery(&export)[0]["comment_count"], 1);
}
