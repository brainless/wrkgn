use serde_json::{Value, json};
use std::sync::Mutex;
use tempfile::tempdir;
use wrkgn::collector::{CollectionConfig, Collector, FetchError, HnApi};
use wrkgn::extraction::{Extraction, Mention, Request, Supporting};
use wrkgn::processing::{Model, process_thread};
use wrkgn::publication;
use wrkgn::store::Store;
struct Api;
impl HnApi for Api {
    fn fetch_item(&self, id: i64) -> Result<Option<Value>, FetchError> {
        Ok(Some(match id {
            100 => json!({"id":100,"kids":[101,105,106]}),
            101 => {
                json!({"id":101,"parent":100,"by":"alice","time":1000,"text":"Building Alpha https://alpha.test and Beta https://beta.test. Feedback welcome.","kids":[102,103,104]})
            }
            102 => {
                json!({"id":102,"parent":101,"by":"bob","time":1001,"text":"Demo of Alpha https://alpha.test"})
            }
            103 => {
                json!({"id":103,"parent":101,"by":"cara","time":1002,"text":"I built Gamma https://gamma.test"})
            }
            104 => json!({"id":104,"parent":101,"by":"dan","time":1003,"text":"off topic"}),
            105 => json!({"id":105,"parent":100,"by":"eve","time":1004,"text":"nothing here"}),
            106 => {
                json!({"id":106,"parent":100,"by":"frank","time":1005,"text":"Unnamed tool for writers"})
            }
            200 => json!({"id":200,"kids":[201]}),
            201 => {
                json!({"id":201,"parent":200,"by":"alice","time":2000,"text":"Alpha again https://alpha.test"})
            }
            _ => panic!("unexpected {id}"),
        }))
    }
}
struct Mock {
    answers: Mutex<Vec<Result<Extraction, String>>>,
}
impl Model for Mock {
    fn name(&self) -> &str {
        "fixture-v1"
    }
    fn extract(&self, _: &str) -> Result<Extraction, String> {
        self.answers.lock().unwrap().remove(0)
    }
}
fn model(answers: Vec<Result<Extraction, String>>) -> Mock {
    Mock {
        answers: Mutex::new(answers),
    }
}
fn mention(id: i64, name: Option<&str>, url: Option<&str>, description: &str) -> Mention {
    Mention {
        source_item_id: id,
        name: name.map(str::to_string),
        urls: url.into_iter().map(str::to_string).collect(),
        description: description.into(),
        problem: Some("organize work".into()),
        audience: Some("writers".into()),
        builder_username: Some(
            match id {
                101 | 201 => "alice",
                103 => "cara",
                106 => "frank",
                _ => "",
            }
            .into(),
        ),
        categories: vec!["productivity tools".into()],
        audience_labels: vec!["writers".into()],
        requests: vec![],
        supporting: vec![],
    }
}
fn fixtures() -> Vec<Result<Extraction, String>> {
    let mut a = mention(
        101,
        Some("Alpha"),
        Some("https://alpha.test"),
        "Alpha organizes work",
    );
    a.requests.push(Request {
        kind: "feedback".into(),
        source_item_id: 101,
        evidence: "Feedback welcome".into(),
    });
    a.supporting.push(Supporting {
        source_item_id: 102,
        summary: "Demo link".into(),
        excerpt: Some("Demo of Alpha".into()),
    });
    let b = mention(
        101,
        Some("Beta"),
        Some("https://beta.test"),
        "Beta helps writers",
    );
    let c = mention(
        103,
        Some("Gamma"),
        Some("https://gamma.test"),
        "Gamma is a tool",
    );
    let u = mention(106, None, None, "Tool for writers");
    vec![
        Ok(Extraction {
            mentions: vec![a, b, c],
        }),
        Ok(Extraction { mentions: vec![] }),
        Ok(Extraction { mentions: vec![u] }),
    ]
}
fn setup() -> (tempfile::TempDir, Store) {
    let dir = tempdir().unwrap();
    let mut store = Store::open(dir.path().join("db.sqlite")).unwrap();
    for id in [100, 200] {
        let report = Collector::new(Api, CollectionConfig::default())
            .collect(&mut store, id)
            .unwrap();
        assert!(report.complete)
    }
    (dir, store)
}
fn discovery(dir: &tempfile::TempDir) -> Vec<Value> {
    let pointer: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("public/current.json")).unwrap())
            .unwrap();
    let version = pointer["version"].as_str().unwrap();
    serde_json::from_slice(
        &std::fs::read(
            dir.path()
                .join("public/snapshots")
                .join(version)
                .join("discovery.json"),
        )
        .unwrap(),
    )
    .unwrap()
}
#[test]
fn extraction_review_rerun_and_export() {
    let (dir, mut store) = setup();
    let report = process_thread(&mut store, 100, &model(fixtures()), false).unwrap();
    assert_eq!(report.processed, 3);
    assert_eq!(publication::pending(&mut store).unwrap().len(), 4);
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert!(discovery(&dir).is_empty());
    let pending = publication::pending(&mut store).unwrap();
    for row in &pending {
        publication::approve(&mut store, row["id"].as_i64().unwrap()).unwrap();
    }
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert_eq!(discovery(&dir).len(), 4);
    let skipped = process_thread(&mut store, 100, &model(vec![]), false).unwrap();
    assert_eq!(skipped.skipped, 3);
    assert_eq!(discovery(&dir).len(), 4);
    let m = mention(
        201,
        Some("Alpha"),
        Some("https://alpha.test"),
        "Alpha again",
    );
    process_thread(
        &mut store,
        200,
        &model(vec![Ok(Extraction { mentions: vec![m] })]),
        false,
    )
    .unwrap();
    let id = publication::pending(&mut store).unwrap()[0]["id"]
        .as_i64()
        .unwrap();
    publication::approve(&mut store, id).unwrap();
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert_eq!(discovery(&dir).len(), 4);
    let alpha = discovery(&dir)
        .into_iter()
        .find(|v| v["name"] == "Alpha")
        .unwrap();
    assert_eq!(alpha["latest_shared_at"], 2000);
    let key = pending.iter().find(|v| v["source_item_id"] == 106).unwrap()["key"]
        .as_str()
        .unwrap();
    publication::correct(&mut store, key, "description", Some("Reviewed wording")).unwrap();
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert!(
        discovery(&dir)
            .iter()
            .any(|v| v["description"] == "Reviewed wording")
    );
    publication::suppress_source(&mut store, 103, "removal request").unwrap();
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert_eq!(discovery(&dir).len(), 3);
}
#[test]
fn invalid_output_and_failed_rerun_keep_prior_records() {
    let (dir, mut store) = setup();
    process_thread(&mut store, 100, &model(fixtures()), false).unwrap();
    let pending = publication::pending(&mut store).unwrap();
    for row in pending {
        publication::approve(&mut store, row["id"].as_i64().unwrap()).unwrap();
    }
    let bad = mention(999, Some("Invented"), None, "bad");
    let report = process_thread(
        &mut store,
        100,
        &model(vec![
            Ok(Extraction {
                mentions: vec![bad],
            }),
            Ok(Extraction { mentions: vec![] }),
            Ok(Extraction { mentions: vec![] }),
        ]),
        true,
    )
    .unwrap();
    assert_eq!(report.failed.len(), 1);
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert_eq!(discovery(&dir).len(), 3);
    let report = process_thread(
        &mut store,
        100,
        &model(vec![
            Err("model unavailable".into()),
            Ok(Extraction { mentions: vec![] }),
            Ok(Extraction { mentions: vec![] }),
        ]),
        true,
    )
    .unwrap();
    assert_eq!(report.failed.len(), 1);
}
#[test]
fn evidence_and_oversize_are_rejected() {
    let (_dir, mut store) = setup();
    let mut m = mention(
        101,
        Some("Alpha"),
        Some("https://alpha.test"),
        "Alpha organizes work",
    );
    m.requests.push(Request {
        kind: "collaborators".into(),
        source_item_id: 101,
        evidence: "please collaborate".into(),
    });
    let report = process_thread(
        &mut store,
        100,
        &model(vec![
            Ok(Extraction { mentions: vec![m] }),
            Ok(Extraction { mentions: vec![] }),
            Ok(Extraction { mentions: vec![] }),
        ]),
        false,
    )
    .unwrap();
    assert_eq!(report.failed.len(), 1);
}
#[test]
fn oversized_input_is_recorded_and_does_not_publish() {
    let (dir, mut store) = setup();
    let run = store.begin_run(100, 3000).unwrap();
    let mut source = store.get_item(106).unwrap().unwrap().source;
    source.body = Some("x".repeat(31_000));
    source.raw_payload = json!({"id":106,"parent":100,"by":"frank","time":1005,"text":source.body});
    for id in [100, 101, 102, 103, 104, 105] {
        let original = store.get_item(id).unwrap().unwrap().source;
        store.upsert_item(run, 3000, &original).unwrap();
    }
    store.upsert_item(run, 3000, &source).unwrap();
    store.finish_run(run, 3000).unwrap();
    let report = process_thread(
        &mut store,
        100,
        &model(vec![
            Ok(Extraction { mentions: vec![] }),
            Ok(Extraction { mentions: vec![] }),
        ]),
        false,
    )
    .unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].1.contains("exceeds"));
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert!(discovery(&dir).is_empty());
}
#[test]
fn schema_rejects_unknown_fields() {
    let value = json!({"mentions":[],"secret":"do this"});
    assert!(serde_json::from_value::<Extraction>(value).is_err());
}
#[test]
fn deleted_source_disappears_before_reprocessing() {
    let (dir, mut store) = setup();
    process_thread(&mut store, 100, &model(fixtures()), false).unwrap();
    for row in publication::pending(&mut store).unwrap() {
        publication::approve(&mut store, row["id"].as_i64().unwrap()).unwrap();
    }
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert_eq!(discovery(&dir).len(), 4);
    let run = store.begin_run(100, 3000).unwrap();
    for id in [100, 101, 102, 103, 104, 105, 106] {
        let mut source = store.get_item(id).unwrap().unwrap().source;
        if id == 103 {
            source.deleted = true;
            source.body = None;
            source.raw_payload = json!({"id":103,"parent":101,"deleted":true});
        }
        store.upsert_item(run, 3000, &source).unwrap();
    }
    store.finish_run(run, 3000).unwrap();
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert!(!discovery(&dir).iter().any(|v| v["name"] == "Gamma"));
}
#[test]
fn manual_project_match_survives_reprocess() {
    let (dir, mut store) = setup();
    process_thread(&mut store, 100, &model(fixtures()), false).unwrap();
    for row in publication::pending(&mut store).unwrap() {
        publication::approve(&mut store, row["id"].as_i64().unwrap()).unwrap();
    }
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    let cards = discovery(&dir);
    let alpha = cards.iter().find(|v| v["name"] == "Alpha").unwrap()["id"]
        .as_i64()
        .unwrap();
    let beta = cards.iter().find(|v| v["name"] == "Beta").unwrap()["id"]
        .as_i64()
        .unwrap();
    publication::match_projects(&mut store, beta, alpha).unwrap();
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert_eq!(discovery(&dir).len(), 3);
    process_thread(&mut store, 100, &model(fixtures()), true).unwrap();
    for row in publication::pending(&mut store).unwrap() {
        publication::approve(&mut store, row["id"].as_i64().unwrap()).unwrap();
    }
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert_eq!(discovery(&dir).len(), 3);
}
fn review_cli(dir: &tempfile::TempDir, args: &[&str]) -> (u8, String, String) {
    let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    for (flag, path) in [("--db", "db.sqlite"), ("--export", "public")] {
        args.push(flag.into());
        args.push(dir.path().join(path).to_str().unwrap().into());
    }
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = wrkgn::cli::run_with_api(&args, Api, &mut out, &mut err).unwrap();
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}
#[test]
fn review_grid_lists_readable_cards_and_approves_ranges() {
    let (dir, mut store) = setup();
    process_thread(&mut store, 100, &model(fixtures()), false).unwrap();
    let ids: Vec<i64> = publication::pending(&mut store)
        .unwrap()
        .iter()
        .map(|v| v["id"].as_i64().unwrap())
        .collect();
    drop(store);
    let (code, out, _) = review_cli(&dir, &["review", "pending"]);
    assert_eq!(code, 0);
    assert!(out.contains("Alpha organizes work"));
    assert!(out.contains("(unnamed)"));
    assert!(out.contains("Looking for  feedback"));
    assert!(out.contains("@bob: Demo link"));
    assert!(out.contains("Building Alpha https://alpha.test"));
    assert!(!out.contains("\"description\""));
    let range = format!("{}-{}", ids[0], ids[3]);
    assert!(out.contains(&format!("Pending IDs: {range}")));
    let (code, out, _) = review_cli(&dir, &["review", "pending", "--json"]);
    assert_eq!((code, out.lines().count()), (0, 4));

    let (code, _, err) = review_cli(&dir, &["review", "approve", "999-1000"]);
    assert_eq!(code, 1);
    assert!(err.contains("no pending mentions in 999-1000"));
    let (code, out, _) = review_cli(&dir, &["review", "approve", &ids[0].to_string()]);
    assert_eq!(code, 0);
    assert!(out.contains("Approved 1 mention(s)."));
    let (code, out, _) = review_cli(&dir, &["review", "approve", &range]);
    assert_eq!(code, 0);
    assert!(out.contains("Approved 3 mention(s)."));
    assert_eq!(discovery(&dir).len(), 4);
    let (_, out, _) = review_cli(&dir, &["review", "pending"]);
    assert_eq!(out, "No pending mentions.\n");
}
