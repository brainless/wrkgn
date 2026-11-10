use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use tempfile::tempdir;
use wrkgn::cli::run_with_api;
use wrkgn::collector::{FetchError, HnApi};
use wrkgn::store::Store;

struct FixtureApi {
    calls: AtomicUsize,
    missing_child: bool,
}

impl HnApi for &FixtureApi {
    fn fetch_item(&self, item_id: i64) -> Result<Option<Value>, FetchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match item_id {
            100 if self.missing_child => Ok(Some(json!({"id":100,"kids":[101]}))),
            100 => Ok(Some(json!({"id":100,"kids":[]}))),
            101 => Ok(None),
            _ => panic!("unexpected fixture item {item_id}"),
        }
    }
}

fn invoke(api: &FixtureApi, command: &str, db_path: &str) -> (u8, String, String) {
    let args = vec![
        command.into(),
        "--thread".into(),
        "100".into(),
        "--db".into(),
        db_path.into(),
    ];
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_with_api(&args, api, &mut out, &mut err).unwrap();
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

#[test]
fn collect_reports_complete_source_collection() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("source.sqlite");
    let api = FixtureApi {
        calls: AtomicUsize::new(0),
        missing_child: false,
    };
    let (code, out, err) = invoke(&api, "collect", path.to_str().unwrap());

    assert_eq!(code, 0);
    assert!(out.contains("Collection complete for thread 100"));
    assert!(!out.contains("processing pipeline"));
    assert!(err.is_empty());
    assert_eq!(api.calls.load(Ordering::SeqCst), 1);
    assert!(Store::open(path).unwrap().get_item(100).unwrap().is_some());
}

fn fixture(missing_child: bool) -> FixtureApi {
    FixtureApi {
        calls: AtomicUsize::new(0),
        missing_child,
    }
}

#[test]
fn process_requires_explicit_model_configuration() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("source.sqlite");
    let api = fixture(false);
    let (code, _, _) = invoke(&api, "collect", path.to_str().unwrap());
    assert_eq!(code, 0);
    assert_eq!(api.calls.load(Ordering::SeqCst), 1);

    let (code, out, err) = invoke(&api, "process", path.to_str().unwrap());
    assert_eq!(code, 1);
    assert!(out.contains("Using collection run"));
    assert!(!out.contains("Collection complete"));
    assert!(err.contains("WRKGN_MODEL") || err.contains("OPENAI_API_KEY"));
    assert_eq!(api.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn process_without_collection_fails_without_fetching() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("source.sqlite");
    let api = fixture(false);
    let (code, _, err) = invoke(&api, "process", path.to_str().unwrap());
    assert_eq!(code, 1);
    assert!(err.contains("wrkgn collect --thread 100"));
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn process_after_incomplete_collection_fails_without_fetching() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("source.sqlite");
    let api = fixture(true);
    let (code, _, _) = invoke(&api, "collect", path.to_str().unwrap());
    assert_eq!(code, 1);
    let calls = api.calls.load(Ordering::SeqCst);

    let (code, out, err) = invoke(&api, "process", path.to_str().unwrap());
    assert_eq!(code, 1);
    assert!(!out.contains("Using collection run"));
    assert!(err.contains("run `wrkgn collect --thread 100` again first"));
    assert_eq!(api.calls.load(Ordering::SeqCst), calls);
}

#[test]
fn incomplete_collection_identifies_failed_item_and_exits_nonzero() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("source.sqlite");
    let api = FixtureApi {
        calls: AtomicUsize::new(0),
        missing_child: true,
    };
    let (code, _, err) = invoke(&api, "collect", path.to_str().unwrap());

    assert_eq!(code, 1);
    assert!(err.contains("Collection incomplete for thread 100"));
    assert!(err.contains("item 101: unavailable: API returned null"));
    assert_eq!(Store::open(path).unwrap().get_item(101).unwrap(), None);
}

#[test]
fn invalid_arguments_and_help_do_not_fetch_or_create_database() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("unused.sqlite");
    let api = FixtureApi {
        calls: AtomicUsize::new(0),
        missing_child: false,
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_with_api(
        &["collect".into(), "--thread".into(), "0".into()],
        &api,
        &mut out,
        &mut err,
    )
    .unwrap();
    assert_eq!(code, 1);
    assert!(String::from_utf8(err).unwrap().contains("positive integer"));

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_with_api(&["--help".into()], &api, &mut out, &mut err).unwrap();
    assert_eq!(code, 0);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("wrkgn process --thread")
    );
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
    assert!(!path.exists());
}
