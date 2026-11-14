use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use wrkgn::collector::{CollectionConfig, Collector, FetchError, HnApi};
use wrkgn::extraction::{Extraction, Mention};
use wrkgn::processing::{Model, process_thread};
use wrkgn::{publication, store::Store};

struct Api(u8);
impl HnApi for Api {
    fn fetch_item(&self, id: i64) -> Result<Option<Value>, FetchError> {
        Ok(Some(match id {
            1 => json!({"id":1,"kids":if self.0 == 0 {vec![2,3]} else {vec![3,2]}}),
            2 => json!({"id":2,"parent":1,"by":"alice","time":1000,
                "text":if self.0 == 4 {"Edited Alpha https://alpha.test"} else {"Alpha https://alpha.test"},
                "kids":if self.0 >= 3 {vec![4,5]} else {vec![4]}}),
            3 => json!({"id":3,"parent":1,"by":"bob","time":1001,"text":"No project"}),
            4 => json!({"id":4,"parent":2,"by":"bob","time":1002,
                "text":if self.0 >= 2 {"Updated reply"} else {"First reply"}}),
            5 => json!({"id":5,"parent":2,"by":"bob","time":1003,"text":"New reply"}),
            _ => panic!("unexpected item {id}"),
        }))
    }
}
struct Fixture(AtomicUsize);
impl Model for Fixture {
    fn name(&self) -> &str {
        "ordering-fixture"
    }
    fn extract(&self, prompt: &str) -> Result<Extraction, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Extraction {
            mentions: if prompt.contains("Alpha") {
                vec![Mention {
                    source_item_id: 2,
                    name: Some("Alpha".into()),
                    urls: vec!["https://alpha.test".into()],
                    description: "Alpha".into(),
                    problem: None,
                    audience: None,
                    source_excerpt: "Alpha".into(),
                    builders: vec![],
                    categories: vec![],
                    audience_labels: vec![],
                }]
            } else {
                vec![]
            },
        })
    }
}
fn collect(store: &mut Store, stage: u8) {
    assert!(
        Collector::new(Api(stage), CollectionConfig::default())
            .collect(store, 1)
            .unwrap()
            .complete
    );
}

#[test]
fn top_level_reordering_reuses_legacy_cache_without_losing_approval_or_freshness() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("db.sqlite")).unwrap();
    let model = Fixture(AtomicUsize::new(0));
    collect(&mut store, 0);
    let first = process_thread(&mut store, 1, &model, false).unwrap();
    assert_eq!((first.processed, first.skipped), (2, 0));
    publication::approve_thread(&mut store, 1).unwrap();
    let destination = dir.path().join("public");
    let before = publication::export(&mut store, &destination).unwrap();

    collect(&mut store, 1);
    let second = process_thread(&mut store, 1, &model, false).unwrap();
    assert_eq!((second.processed, second.skipped), (0, 2));
    assert_eq!(model.0.load(Ordering::SeqCst), 2);
    assert!(publication::pending(&mut store).unwrap().is_empty());
    let after = publication::export(&mut store, &destination).unwrap();
    assert_eq!(before, after);

    // Cache fingerprints are advanced to the current rank without a model
    // invocation; a subsequent unchanged pass also reuses both groups.
    let third = process_thread(&mut store, 1, &model, false).unwrap();
    assert_eq!((third.processed, third.skipped), (0, 2));
    assert_eq!(model.0.load(Ordering::SeqCst), 2);

    let forced = process_thread(&mut store, 1, &model, true).unwrap();
    assert_eq!((forced.processed, forced.skipped), (2, 0));
    assert_eq!(model.0.load(Ordering::SeqCst), 4);
}

#[test]
fn reply_edits_new_replies_and_root_edits_still_invalidate_their_group() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("db.sqlite")).unwrap();
    let model = Fixture(AtomicUsize::new(0));
    collect(&mut store, 0);
    process_thread(&mut store, 1, &model, false).unwrap();
    for stage in [2, 3, 4] {
        collect(&mut store, stage);
        let report = process_thread(&mut store, 1, &model, false).unwrap();
        assert_eq!((report.processed, report.skipped), (1, 1), "stage {stage}");
        assert!(report.failed.is_empty());
    }
    assert_eq!(model.0.load(Ordering::SeqCst), 5);
}
