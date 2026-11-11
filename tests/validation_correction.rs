use serde_json::{Value, json};
use std::sync::Mutex;
use tempfile::tempdir;
use wrkgn::collector::{CollectionConfig, Collector, FetchError, HnApi};
use wrkgn::extraction::{Extraction, Mention};
use wrkgn::processing::{Model, process_thread};
use wrkgn::store::Store;

struct Api;
impl HnApi for Api {
    fn fetch_item(&self, id: i64) -> Result<Option<Value>, FetchError> {
        Ok(Some(match id {
            1 => json!({"id": 1, "kids": [2]}),
            2 => json!({"id": 2, "parent": 1, "by": "alice", "time": 1000,
                "text": "Building Alpha https://alpha.test"}),
            _ => panic!("unexpected item"),
        }))
    }
}

struct Responses {
    answers: Mutex<Vec<Extraction>>,
    prompts: Mutex<Vec<String>>,
}
impl Model for Responses {
    fn name(&self) -> &str {
        "validation-correction-fixture"
    }
    fn extract(&self, prompt: &str) -> Result<Extraction, String> {
        self.prompts.lock().unwrap().push(prompt.to_owned());
        Ok(self.answers.lock().unwrap().remove(0))
    }
}
fn mention(excerpt: &str) -> Extraction {
    Extraction {
        mentions: vec![Mention {
            source_item_id: 2,
            name: Some("Alpha".into()),
            urls: vec!["https://alpha.test".into()],
            description: "Alpha is being built".into(),
            problem: None,
            audience: None,
            source_excerpt: excerpt.into(),
            builders: vec![],
            categories: vec![],
            audience_labels: vec![],
        }],
    }
}
fn setup() -> (tempfile::TempDir, Store) {
    let dir = tempdir().unwrap();
    let mut store = Store::open(dir.path().join("db.sqlite")).unwrap();
    assert!(
        Collector::new(Api, CollectionConfig::default())
            .collect(&mut store, 1)
            .unwrap()
            .complete
    );
    (dir, store)
}
#[test]
fn invalid_extraction_is_corrected_with_original_source_and_validation_error() {
    let (_dir, mut store) = setup();
    let model = Responses {
        answers: Mutex::new(vec![mention("invented excerpt"), mention("Building Alpha")]),
        prompts: Mutex::new(vec![]),
    };
    let report = process_thread(&mut store, 1, &model, false).unwrap();
    assert_eq!((report.processed, report.failed.len()), (1, 0));
    let prompts = model.prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    assert!(prompts[1].contains("unsupported project source excerpt"));
    assert!(prompts[1].contains("Building Alpha https://alpha.test"));
    assert!(prompts[1].contains("untrusted data"));
}
#[test]
fn exhausted_corrections_leave_previously_saved_group_intact() {
    let (dir, mut store) = setup();
    let good = Responses {
        answers: Mutex::new(vec![mention("Building Alpha")]),
        prompts: Mutex::new(vec![]),
    };
    assert_eq!(
        process_thread(&mut store, 1, &good, false)
            .unwrap()
            .processed,
        1
    );
    let db = rusqlite::Connection::open(dir.path().join("db.sqlite")).unwrap();
    let before: i64 = db
        .query_row("SELECT COUNT(*) FROM mentions", [], |r| r.get(0))
        .unwrap();
    let bad = Responses {
        answers: Mutex::new(vec![mention("invented"); 3]),
        prompts: Mutex::new(vec![]),
    };
    let report = process_thread(&mut store, 1, &bad, true).unwrap();
    assert_eq!(report.failed.len(), 1);
    assert_eq!(bad.prompts.lock().unwrap().len(), 3);
    assert!(
        report.failed[0]
            .1
            .contains("unsupported project source excerpt")
    );
    let after: i64 = db
        .query_row("SELECT COUNT(*) FROM mentions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, before);
}
