use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tempfile::tempdir;
use wrkgn::category::Category;
use wrkgn::collector::{CollectionConfig, Collector, FetchError, HnApi};
use wrkgn::extraction::{Extraction, Mention, validate};
use wrkgn::processing::{Model, Options, Progress, process_thread, process_thread_with};
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
                json!({"id":103,"parent":101,"by":"cara","time":1002,"text":"I built Gamma with co-builder zara https://gamma.test"})
            }
            104 => json!({"id":104,"parent":101,"by":"dan","time":1003,"text":"off topic"}),
            105 => json!({"id":105,"parent":100,"by":"eve","time":1004,"text":"nothing here"}),
            106 => {
                json!({"id":106,"parent":100,"by":"frank","time":1005,"text":"Unnamed tool for writers"})
            }
            200 => json!({"id":200,"kids":[201]}),
            201 => {
                json!({"id":201,"parent":200,"by":"alice","time":2000,"text":"Alpha again https://alpha.test/ and https://alpha.test; Beta https://beta.test"})
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
        source_excerpt: match id {
            101 => name.unwrap_or("Alpha"),
            103 => "Gamma",
            106 => "Unnamed tool",
            201 => "Alpha",
            301 => "A free app",
            _ => "unknown",
        }
        .into(),
        builders: vec![],
        categories: vec![Category::Productivity],
        audience_labels: vec![wrkgn::audience::Audience::CreativeProfessionals],
    }
}
fn fixtures() -> Vec<Result<Extraction, String>> {
    let a = mention(
        101,
        Some("Alpha"),
        Some("https://alpha.test"),
        "Alpha organizes work",
    );
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
/// Answers by prompt content so results do not depend on call order, and
/// records the peak number of overlapping calls.
struct Parallel {
    in_flight: AtomicUsize,
    peak: AtomicUsize,
}
impl Model for Parallel {
    fn name(&self) -> &str {
        "fixture-v1"
    }
    fn extract(&self, prompt: &str) -> Result<Extraction, String> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(50));
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        if prompt.contains("nothing here") {
            Err("model failed".into())
        } else {
            answer(prompt)
        }
    }
}
/// The fixture answer for whichever group a prompt describes.
fn answer(prompt: &str) -> Result<Extraction, String> {
    let mut answers = fixtures().into_iter();
    if prompt.contains("Building Alpha") {
        answers.next().unwrap()
    } else if prompt.contains("nothing here") {
        answers.nth(1).unwrap()
    } else {
        answers.nth(2).unwrap()
    }
}
fn options(concurrency: usize) -> Options {
    Options {
        concurrency,
        initial_backoff: Duration::ZERO,
        max_backoff: Duration::ZERO,
        ..Options::default()
    }
}
const RATE_LIMITED: &str =
    "ProviderResponseError: status 429 Too Many Requests: Concurrent limit reached";
/// Rate-limits the first `failures` calls, then answers normally.
struct Flaky {
    failures: AtomicUsize,
    calls: AtomicUsize,
}
impl Model for Flaky {
    fn name(&self) -> &str {
        "fixture-v1"
    }
    fn extract(&self, prompt: &str) -> Result<Extraction, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let left = self.failures.load(Ordering::SeqCst);
        if left > 0 {
            self.failures.store(left - 1, Ordering::SeqCst);
            return Err(RATE_LIMITED.into());
        }
        answer(prompt)
    }
}
fn flaky(failures: usize) -> Flaky {
    Flaky {
        failures: AtomicUsize::new(failures),
        calls: AtomicUsize::new(0),
    }
}
#[test]
fn rate_limited_calls_are_retried() {
    let (_dir, mut store) = setup();
    let model = flaky(2);
    let report = process_thread_with(&mut store, 100, &model, false, &options(1), |_| {}).unwrap();
    assert_eq!((report.processed, report.failed.len()), (3, 0));
    assert_eq!(model.calls.load(Ordering::SeqCst), 5);
}
#[test]
fn repeated_rate_limits_stop_the_run_and_leave_the_rest_unattempted() {
    let (_dir, mut store) = setup();
    let model = flaky(usize::MAX);
    let mut seen: Vec<Progress> = Vec::new();
    let report = process_thread_with(
        &mut store,
        100,
        &model,
        false,
        &Options {
            max_attempts: 2,
            stop_after_transient_failures: 2,
            ..options(1)
        },
        |p| seen.push(p),
    )
    .unwrap();
    assert!(report.stopped_early);
    assert_eq!((report.failed.len(), report.not_attempted), (2, 1));
    assert_eq!(model.calls.load(Ordering::SeqCst), 4);
    let last = seen.last().unwrap();
    assert_eq!((last.done, last.total, last.failed), (2, 3, 2));
    let resumed =
        process_thread_with(&mut store, 100, &flaky(0), false, &options(1), |_| {}).unwrap();
    assert_eq!((resumed.processed, resumed.skipped), (3, 0));
}
#[test]
fn parallel_extraction_saves_each_group_with_its_own_result() {
    let (_dir, mut store) = setup();
    let model = Parallel {
        in_flight: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
    };
    let report = process_thread_with(&mut store, 100, &model, false, &options(10), |_| {}).unwrap();
    assert_eq!(model.peak.load(Ordering::SeqCst), 3);
    assert_eq!(report.processed, 2);
    assert_eq!(report.failed, vec![(105, "model failed".to_string())]);
    let pending = publication::pending(&mut store).unwrap();
    let mut by_source: Vec<(i64, String)> = pending
        .iter()
        .map(|m| {
            (
                m["source_item_id"].as_i64().unwrap(),
                m["description"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    by_source.sort();
    assert_eq!(
        by_source,
        vec![
            (101, "Alpha organizes work".to_string()),
            (101, "Beta helps writers".to_string()),
            (103, "Gamma is a tool".to_string()),
            (106, "Tool for writers".to_string()),
        ]
    );
    let retry = process_thread_with(&mut store, 100, &model, false, &options(10), |_| {}).unwrap();
    assert_eq!((retry.skipped, retry.failed.len()), (2, 1));
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
    let report = process_thread_with(
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
        &Options {
            max_validation_corrections: 0,
            ..options(1)
        },
        |_| {},
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
    m.source_excerpt = "Unsupported text".into();
    let report = process_thread_with(
        &mut store,
        100,
        &model(vec![
            Ok(Extraction { mentions: vec![m] }),
            Ok(Extraction { mentions: vec![] }),
            Ok(Extraction { mentions: vec![] }),
        ]),
        false,
        &Options {
            max_validation_corrections: 0,
            ..options(1)
        },
        |_| {},
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
fn categories_reject_unknown_duplicate_and_more_than_five() {
    let (_, store) = setup();
    let item = store.get_item(101).unwrap().unwrap();
    let source = BTreeMap::from([(101, &item)]);
    let ids = BTreeSet::from([101]);
    let mut m = mention(101, Some("Alpha"), None, "Alpha organizes work");
    m.categories = vec![Category::Productivity, Category::Productivity];
    assert!(
        wrkgn::extraction::validate(
            &Extraction {
                mentions: vec![m.clone()]
            },
            &ids,
            &source
        )
        .unwrap_err()
        .contains("duplicate")
    );
    m.categories = wrkgn::category::Category::ALL[..6].to_vec();
    assert!(
        wrkgn::extraction::validate(
            &Extraction {
                mentions: vec![m.clone()]
            },
            &ids,
            &source
        )
        .unwrap_err()
        .contains("five")
    );
    let mut value = serde_json::to_value(Extraction { mentions: vec![m] }).unwrap();
    value["mentions"][0]["categories"] = json!(["invented-category"]);
    assert!(serde_json::from_value::<Extraction>(value).is_err());
    let mut valid = mention(101, Some("Alpha"), None, "Alpha organizes work");
    valid.categories.clear();
    assert!(
        wrkgn::extraction::validate(
            &Extraction {
                mentions: vec![valid.clone()]
            },
            &ids,
            &source
        )
        .is_ok()
    );
    valid.categories = wrkgn::category::Category::ALL[..5].to_vec();
    assert!(
        wrkgn::extraction::validate(
            &Extraction {
                mentions: vec![valid]
            },
            &ids,
            &source
        )
        .is_ok()
    );
}
#[test]
fn category_migration_is_atomic_and_preserves_review() {
    let (dir, mut store) = setup();
    let conn = rusqlite::Connection::open(dir.path().join("db.sqlite")).unwrap();
    process_thread(&mut store, 100, &model(fixtures()), false).unwrap();
    let pending = publication::pending(&mut store).unwrap();
    let id = pending[0]["id"].as_i64().unwrap();
    let key = pending[0]["key"].as_str().unwrap();
    publication::approve(&mut store, id).unwrap();
    publication::correct(&mut store, key, "name", Some("Corrected Alpha")).unwrap();
    conn.execute(
        "DELETE FROM mention_labels WHERE mention_id=?1 AND kind='category'",
        [id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO mention_labels VALUES(?1,'category','AI agents')",
        [id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO mention_labels VALUES(?1,'category','mystery')",
        [id],
    )
    .unwrap();
    assert!(
        wrkgn::category_migration::normalize(&mut store)
            .unwrap_err()
            .contains("unknown categories")
    );
    let values = || -> Vec<String> {
        let mut st=conn.prepare("SELECT value FROM mention_labels WHERE mention_id=?1 AND kind='category' ORDER BY value").unwrap();
        st.query_map([id], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    assert_eq!(values(), vec!["AI agents", "mystery"]);
    conn.execute(
        "DELETE FROM mention_labels WHERE mention_id=?1 AND value='mystery'",
        [id],
    )
    .unwrap();
    wrkgn::category_migration::normalize(&mut store).unwrap();
    assert_eq!(values(), vec!["ai-automation"]);
    wrkgn::category_migration::normalize(&mut store).unwrap();
    assert_eq!(values(), vec!["ai-automation"]);
    let state: String = conn
        .query_row("SELECT review_state FROM mentions WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(state, "approved");
    let corrected: String = conn
        .query_row(
            "SELECT value FROM review_corrections WHERE mention_key=?1 AND field='name'",
            [key],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(corrected, "Corrected Alpha");
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
    let conn = rusqlite::Connection::open(dir.path().join("db.sqlite")).unwrap();
    let alias_target: i64 = conn.query_row("SELECT project_id FROM project_url_aliases WHERE author='alice' AND canonical_url='https://beta.test/'", [], |r| r.get(0)).unwrap();
    assert_eq!(alias_target, alpha);
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert_eq!(discovery(&dir).len(), 3);
    process_thread(&mut store, 100, &model(fixtures()), true).unwrap();
    for row in publication::pending(&mut store).unwrap() {
        publication::approve(&mut store, row["id"].as_i64().unwrap()).unwrap();
    }
    publication::export(&mut store, &dir.path().join("public")).unwrap();
    assert_eq!(discovery(&dir).len(), 3);
}
#[test]
fn url_order_and_trailing_slash_match_only_the_same_source_author() {
    let (dir, mut store) = setup();
    let mut alpha = mention(
        101,
        Some("Alpha"),
        Some("https://alpha.test"),
        "Alpha organizes work",
    );
    alpha.urls.push("https://beta.test".into());
    let mut bob = mention(
        102,
        Some("Alpha"),
        Some("https://alpha.test"),
        "Bob's own project",
    );
    bob.source_excerpt = "Demo of Alpha".into();
    process_thread(
        &mut store,
        100,
        &model(vec![
            Ok(Extraction {
                mentions: vec![alpha, bob],
            }),
            Ok(Extraction { mentions: vec![] }),
            Ok(Extraction { mentions: vec![] }),
        ]),
        false,
    )
    .unwrap();
    let mut repeated = mention(
        201,
        Some("Alpha"),
        Some("https://beta.test"),
        "Alpha organizes work",
    );
    repeated.urls.push("https://alpha.test/".into());
    process_thread(
        &mut store,
        200,
        &model(vec![Ok(Extraction {
            mentions: vec![repeated],
        })]),
        false,
    )
    .unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("db.sqlite")).unwrap();
    let project = |source| {
        conn.query_row(
            "SELECT project_id FROM mentions WHERE source_item_id=?1",
            [source],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(project(101), project(201));
    assert_ne!(project(102), project(201));
}

#[test]
fn suppressed_project_stays_suppressed_when_shared_again() {
    let (dir, mut store) = setup();
    let alpha = mention(
        101,
        Some("Alpha"),
        Some("https://alpha.test"),
        "Alpha organizes work",
    );
    process_thread(
        &mut store,
        100,
        &model(vec![
            Ok(Extraction {
                mentions: vec![alpha],
            }),
            Ok(Extraction { mentions: vec![] }),
            Ok(Extraction { mentions: vec![] }),
        ]),
        false,
    )
    .unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("db.sqlite")).unwrap();
    let pid: i64 = conn
        .query_row(
            "SELECT project_id FROM mentions WHERE source_item_id=101",
            [],
            |r| r.get(0),
        )
        .unwrap();
    publication::suppress_project(&mut store, pid).unwrap();
    process_thread(
        &mut store,
        100,
        &model(vec![
            Ok(Extraction { mentions: vec![] }),
            Ok(Extraction { mentions: vec![] }),
            Ok(Extraction { mentions: vec![] }),
        ]),
        true,
    )
    .unwrap();
    let repeated = mention(
        201,
        Some("Alpha"),
        Some("https://alpha.test/"),
        "Alpha organizes work",
    );
    process_thread(
        &mut store,
        200,
        &model(vec![Ok(Extraction {
            mentions: vec![repeated],
        })]),
        false,
    )
    .unwrap();
    let repeated_pid: i64 = conn
        .query_row(
            "SELECT project_id FROM mentions WHERE source_item_id=201",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pid, repeated_pid);
}

#[test]
fn ambiguous_url_with_suppressed_identity_fails_closed() {
    let (dir, mut store) = setup();
    let alpha = mention(
        101,
        Some("Alpha"),
        Some("https://alpha.test"),
        "Alpha organizes work",
    );
    process_thread(
        &mut store,
        100,
        &model(vec![
            Ok(Extraction {
                mentions: vec![alpha],
            }),
            Ok(Extraction { mentions: vec![] }),
            Ok(Extraction { mentions: vec![] }),
        ]),
        false,
    )
    .unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("db.sqlite")).unwrap();
    conn.execute(
        "INSERT INTO projects(identity_key,slug,created_at,suppressed) VALUES('other','other',0,1)",
        [],
    )
    .unwrap();
    let other = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO project_url_aliases VALUES('alice','https://alpha.test/',?1)",
        [other],
    )
    .unwrap();
    let repeated = mention(
        201,
        Some("Alpha"),
        Some("https://alpha.test/"),
        "Alpha organizes work",
    );
    let report = process_thread(
        &mut store,
        200,
        &model(vec![Ok(Extraction {
            mentions: vec![repeated],
        })]),
        false,
    )
    .unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].1.contains("suppressed project"));
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM mentions WHERE source_item_id=201",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn splitting_a_sole_mention_does_not_assign_both_projects_to_the_old_identity() {
    let (dir, mut store) = setup();
    let alpha = mention(
        101,
        Some("Alpha"),
        Some("https://alpha.test"),
        "Alpha organizes work",
    );
    process_thread(
        &mut store,
        100,
        &model(vec![
            Ok(Extraction {
                mentions: vec![alpha.clone()],
            }),
            Ok(Extraction { mentions: vec![] }),
            Ok(Extraction { mentions: vec![] }),
        ]),
        false,
    )
    .unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("db.sqlite")).unwrap();
    let old: i64 = conn
        .query_row(
            "SELECT project_id FROM mentions WHERE source_item_id=101",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let beta = mention(
        101,
        Some("Beta"),
        Some("https://beta.test"),
        "Beta helps writers",
    );
    process_thread(
        &mut store,
        100,
        &model(vec![
            Ok(Extraction {
                mentions: vec![alpha, beta],
            }),
            Ok(Extraction { mentions: vec![] }),
            Ok(Extraction { mentions: vec![] }),
        ]),
        true,
    )
    .unwrap();
    let mut st = conn
        .prepare("SELECT project_id FROM mentions WHERE source_item_id=101 ORDER BY name")
        .unwrap();
    let ids: Vec<i64> = st
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    assert_eq!(ids.iter().filter(|id| **id == old).count(), 1);
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
    assert!(!out.contains("Looking for"));
    assert!(out.contains("Evidence"));
    assert!(!out.contains("@bob: Demo link"));
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

/// Bodies as the HN API returns them: HTML-escaped, with links as anchors.
struct EscapedApi;
impl HnApi for EscapedApi {
    fn fetch_item(&self, id: i64) -> Result<Option<Value>, FetchError> {
        Ok(Some(match id {
            300 => json!({"id":300,"kids":[301]}),
            301 => json!({"id":301,"parent":300,"by":"hana","time":3000,
                "text":"<a href=\"https:&#x2F;&#x2F;pahunchi.netlify.app&#x2F;\" rel=\"nofollow\">https:&#x2F;&#x2F;pahunchi.netlify.app&#x2F;</a><p>A free app I&#x27;ve built, also hydrafetch.com. Feedback welcome.",
                "kids":[302]}),
            302 => json!({"id":302,"parent":301,"by":"ian","time":3001,
                "text":"It&#x27;s  great\nstuff, see <a href=\"https:&#x2F;&#x2F;example.org&#x2F;long&#x2F;path&#x2F;page\">https:&#x2F;&#x2F;example.org&#x2F;lo...</a>"}),
            _ => panic!("unexpected {id}"),
        }))
    }
}
fn escaped_store() -> (tempfile::TempDir, Store) {
    let dir = tempdir().unwrap();
    let mut store = Store::open(dir.path().join("db.sqlite")).unwrap();
    let report = Collector::new(EscapedApi, CollectionConfig::default())
        .collect(&mut store, 300)
        .unwrap();
    assert!(report.complete);
    (dir, store)
}
fn escaped_mention() -> Mention {
    let mut m = mention(
        301,
        Some("Pahunchi"),
        Some("https://pahunchi.netlify.app/"),
        "A free app",
    );
    m.source_excerpt = "A free app".into();
    m
}
fn validated(m: Mention) -> Result<(), String> {
    let (_dir, store) = escaped_store();
    let items = [
        store.get_item(301).unwrap().unwrap(),
        store.get_item(302).unwrap().unwrap(),
    ];
    let source = BTreeMap::from([(301, &items[0]), (302, &items[1])]);
    validate(
        &Extraction { mentions: vec![m] },
        &BTreeSet::from([301, 302]),
        &source,
    )
}
#[test]
fn validation_compares_against_decoded_text() {
    let mut m = escaped_mention();
    assert_eq!(validated(m.clone()), Ok(()));
    m.urls = vec![
        "https://pahunchi.netlify.app".into(),
        "https://Pahunchi.Netlify.App/".into(),
        "https://hydrafetch.com".into(),
    ];
    m.source_excerpt = "I\u{2019}ve built,   also hydrafetch.com. Feedback welcome".into();
    assert_eq!(validated(m), Ok(()));
}
#[test]
fn validation_still_rejects_unsupported_urls_and_evidence() {
    for url in [
        "https://other.test/",
        "https://pahunchi.netlify.app/x",
        "http://pahunchi.netlify.app",
    ] {
        let mut m = escaped_mention();
        m.urls = vec![url.into()];
        assert_eq!(
            validated(m),
            Err(format!("URL absent from source: source 301, url {url:?}")),
            "{url}"
        );
    }
    let mut m = escaped_mention();
    m.source_excerpt = "Unsupported text".into();
    assert_eq!(
        validated(m),
        Err("unsupported project source excerpt: source 301, excerpt \"Unsupported text\"".into())
    );
    let mut m = escaped_mention();
    m.source_excerpt = "Unsupported text".into();
    assert_eq!(
        validated(m),
        Err("unsupported project source excerpt: source 301, excerpt \"Unsupported text\"".into())
    );
}
#[test]
fn validation_diagnostics_identify_source_and_bound_model_values() {
    let mut m = escaped_mention();
    m.source_item_id = 999;
    assert_eq!(
        validated(m),
        Err("mention source outside input: source 999".into())
    );

    let mut m = escaped_mention();
    m.urls = vec![format!("https://other.test/{}", "é".repeat(300))];
    let error = validated(m).unwrap_err();
    assert!(error.starts_with("URL absent from source: source 301, url \"https://other.test/"));
    assert!(error.contains('…'));
    assert!(!error.contains(&"é".repeat(150)));

    let mut m = escaped_mention();
    m.builders = vec!["unmentioned_builder".into()];
    assert_eq!(
        validated(m),
        Err(
            "unsupported additional builder username: source 301, builder \"unmentioned_builder\""
                .into()
        )
    );
}
#[test]
fn omitted_arrays_and_flags_default() {
    let value = json!({"mentions":[{"source_item_id":1,"name":null,"description":"d",
        "problem":null,"audience":null,"source_excerpt":"d"}]});
    let m = &serde_json::from_value::<Extraction>(value)
        .unwrap()
        .mentions[0];
    assert!(m.urls.is_empty() && m.builders.is_empty());
    assert!(m.categories.is_empty() && m.audience_labels.is_empty());
    assert!(
        serde_json::from_value::<Extraction>(json!({}))
            .unwrap()
            .mentions
            .is_empty()
    );
    assert!(
        serde_json::from_value::<Mention>(
            json!({"source_item_id":1,"description":"d","source_excerpt":"d","requests":[]})
        )
        .is_err()
    );
}
#[test]
fn escaped_comment_is_saved_and_builder_comes_from_the_author() {
    {
        let expected = Some("hana".to_string());
        let (dir, mut store) = escaped_store();
        let m = escaped_mention();
        let report = process_thread(
            &mut store,
            300,
            &model(vec![Ok(Extraction { mentions: vec![m] })]),
            false,
        )
        .unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let stored: Option<String> = rusqlite::Connection::open(dir.path().join("db.sqlite"))
            .unwrap()
            .query_row("SELECT builder_username FROM mentions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, expected);
    }
}
#[test]
fn model_prompt_carries_plain_text() {
    struct Capture(Mutex<String>);
    impl Model for Capture {
        fn name(&self) -> &str {
            "fixture-v1"
        }
        fn extract(&self, prompt: &str) -> Result<Extraction, String> {
            *self.0.lock().unwrap() = prompt.to_string();
            Ok(Extraction { mentions: vec![] })
        }
    }
    let (_dir, mut store) = escaped_store();
    let capture = Capture(Mutex::new(String::new()));
    process_thread(&mut store, 300, &capture, false).unwrap();
    let prompt = capture.0.lock().unwrap().clone();
    assert!(prompt.contains("https://pahunchi.netlify.app/"));
    assert!(prompt.contains("I've built") && !prompt.contains("&#x"));
}

#[test]
fn export_requires_reprocessing_legacy_mentions_and_preserves_snapshot() {
    let (dir, mut store) = setup();
    process_thread(&mut store, 100, &model(fixtures()), false).unwrap();
    for m in publication::pending(&mut store).unwrap() {
        publication::approve(&mut store, m["id"].as_i64().unwrap()).unwrap();
    }
    let destination = dir.path().join("public");
    publication::export(&mut store, &destination).unwrap();
    let pointer = std::fs::read(destination.join("current.json")).unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("db.sqlite")).unwrap();
    conn.execute("UPDATE mentions SET source_excerpt=NULL", [])
        .unwrap();
    assert!(
        publication::export(&mut store, &destination)
            .unwrap_err()
            .contains("reprocess")
    );
    assert_eq!(
        pointer,
        std::fs::read(destination.join("current.json")).unwrap()
    );
}

#[test]
fn reply_project_preserves_its_author_and_additional_builders_in_export() {
    let (dir, mut store) = setup();
    let mut answers = fixtures();
    answers[0]
        .as_mut()
        .unwrap()
        .mentions
        .iter_mut()
        .find(|m| m.source_item_id == 103)
        .unwrap()
        .builders = vec!["zara".into()];
    let report = process_thread(&mut store, 100, &model(answers), false).unwrap();
    assert!(report.failed.is_empty());
    for row in publication::pending(&mut store).unwrap() {
        publication::approve(&mut store, row["id"].as_i64().unwrap()).unwrap();
    }
    let destination = dir.path().join("public");
    let version = publication::export(&mut store, &destination).unwrap();
    let gamma = discovery(&dir)
        .into_iter()
        .find(|p| p["name"] == "Gamma")
        .unwrap();
    assert_eq!(gamma["builder_usernames"], json!(["cara", "zara"]));
    assert!(gamma.get("looking_for").is_none());
    let detail: Value = serde_json::from_slice(
        &std::fs::read(
            destination
                .join("snapshots")
                .join(version)
                .join("projects")
                .join(format!("{}.json", gamma["slug"].as_str().unwrap())),
        )
        .unwrap(),
    )
    .unwrap();
    let mention = &detail["mentions"][0];
    assert_eq!(mention["source_item_id"], 103);
    assert_eq!(mention["author"], "cara");
    assert_eq!(mention["builder_usernames"], json!(["cara", "zara"]));
    assert_eq!(mention["source_excerpt"], "Gamma");
    assert!(mention.get("requests").is_none() && mention.get("supporting").is_none());
}

#[test]
fn public_reply_counts_deduplicate_overlapping_mentions_and_respect_suppression() {
    let (dir, mut store) = setup();
    // Add a nested reply so two published source subtrees overlap after matching.
    let mut nested = store.get_item(104).unwrap().unwrap().source;
    nested.item_id = 107;
    nested.parent_id = Some(103);
    nested.raw_payload = json!({"id":107,"parent":103,"text":"nested reply"});
    let run = store.begin_run(100, 3000).unwrap();
    for id in [100, 101, 102, 103, 104, 105, 106] {
        let source = store.get_item(id).unwrap().unwrap().source;
        store.upsert_item(run, 3000, &source).unwrap();
    }
    store.upsert_item(run, 3000, &nested).unwrap();
    store.finish_run(run, 3000).unwrap();
    process_thread(&mut store, 100, &model(fixtures()), false).unwrap();
    for row in publication::pending(&mut store).unwrap() {
        publication::approve(&mut store, row["id"].as_i64().unwrap()).unwrap();
    }
    let destination = dir.path().join("public");
    publication::export(&mut store, &destination).unwrap();
    let cards = discovery(&dir);
    let alpha = cards.iter().find(|v| v["name"] == "Alpha").unwrap();
    let gamma = cards.iter().find(|v| v["name"] == "Gamma").unwrap();
    assert_eq!(alpha["comment_count"], 4);
    assert_eq!(gamma["comment_count"], 1);
    assert_eq!(
        cards.iter().find(|v| v["name"].is_null()).unwrap()["comment_count"],
        0
    );
    assert_eq!(
        publication::match_projects(
            &mut store,
            gamma["id"].as_i64().unwrap(),
            alpha["id"].as_i64().unwrap(),
        )
        .unwrap_err(),
        "projects must have the same known source author"
    );
    publication::export(&mut store, &destination).unwrap();
    assert_eq!(
        discovery(&dir)
            .iter()
            .find(|v| v["id"] == alpha["id"])
            .unwrap()["comment_count"],
        4
    );
    publication::suppress_source(&mut store, 103, "removal").unwrap();
    publication::export(&mut store, &destination).unwrap();
    assert_eq!(
        discovery(&dir)
            .iter()
            .find(|v| v["id"] == alpha["id"])
            .unwrap()["comment_count"],
        3
    );
}
