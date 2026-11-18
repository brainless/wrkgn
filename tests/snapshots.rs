use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use tempfile::tempdir;
use wrkgn::cli::run_with_api;
use wrkgn::collector::{FetchError, HnApi};

struct NoApi;
impl HnApi for NoApi {
    fn fetch_item(&self, _: i64) -> Result<Option<serde_json::Value>, FetchError> {
        panic!("snapshots prune must not touch HN");
    }
}

fn version(n: u8) -> String {
    format!("{n:016x}")
}

/// Creates a snapshot whose directory mtime is `age` seconds old.
fn snapshot(export: &Path, name: &str, age: u64) {
    let dir = export.join("snapshots").join(name);
    fs::create_dir_all(dir.join("projects")).unwrap();
    fs::write(dir.join("manifest.json"), "{}").unwrap();
    fs::write(dir.join("public.sqlite"), vec![0u8; 2048]).unwrap();
    let file = fs::File::open(&dir).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(age))
        .unwrap();
}

fn point(export: &Path, v: &str) {
    fs::write(
        export.join("current.json"),
        format!(
            r#"{{"schema_version":6,"version":"{v}","snapshot":"snapshots/{v}/manifest.json"}}"#
        ),
    )
    .unwrap();
}

fn run(args: &[&str]) -> (u8, String, String) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run_with_api(&args, NoApi, &mut out, &mut err).unwrap();
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn names(export: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(export.join("snapshots"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

#[test]
fn keeps_current_plus_newest_others_and_dry_run_changes_nothing() {
    let dir = tempdir().unwrap();
    let export = dir.path();
    // Version 1 is oldest but selected by current.json.
    for (n, age) in [(1, 5000), (2, 4000), (3, 3000), (4, 2000), (5, 1000)] {
        snapshot(export, &version(n), age);
    }
    point(export, &version(1));
    let e = export.to_str().unwrap();

    let (code, out, _) = run(&["snapshots", "prune", "--export", e, "--dry-run"]);
    assert_eq!(code, 0);
    assert!(out.contains("Would remove snapshot"), "{out}");
    assert_eq!(names(export).len(), 5);

    let (code, out, _) = run(&["snapshots", "prune", "--export", e, "--keep", "2"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(names(export), vec![version(1), version(4), version(5)]);
    assert!(out.contains("reclaimed"), "{out}");
    assert!(export.join("current.json").exists());
}

#[test]
fn keep_zero_leaves_only_current() {
    let dir = tempdir().unwrap();
    for n in 1..=3 {
        snapshot(dir.path(), &version(n), 100 * n as u64);
    }
    point(dir.path(), &version(2));
    let (code, _, err) = run(&[
        "snapshots",
        "prune",
        "--export",
        dir.path().to_str().unwrap(),
        "--keep",
        "0",
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(names(dir.path()), vec![version(2)]);
}

#[test]
fn removes_dead_staging_but_refuses_when_owner_is_running() {
    let dir = tempdir().unwrap();
    let export = dir.path();
    snapshot(export, &version(1), 10);
    point(export, &version(1));
    let dead = export
        .join("snapshots")
        .join(format!(".staging-4294967290-{}", version(9)));
    fs::create_dir_all(dead.join("projects")).unwrap();
    fs::write(dead.join("x"), "x").unwrap();
    fs::create_dir(export.join("snapshots/notes")).unwrap();
    fs::create_dir(export.join("snapshots/.staging-garbage")).unwrap();
    let e = export.to_str().unwrap();

    let (code, out, err) = run(&["snapshots", "prune", "--export", e]);
    assert_eq!(code, 0, "{err}");
    assert!(!dead.exists());
    assert!(export.join("snapshots/notes").exists());
    assert!(export.join("snapshots/.staging-garbage").exists());
    assert!(out.contains("stale staging"), "{out}");

    let live =
        export
            .join("snapshots")
            .join(format!(".staging-{}-{}", std::process::id(), version(8)));
    fs::create_dir_all(&live).unwrap();
    snapshot(export, &version(2), 20);
    let (code, _, err) = run(&["snapshots", "prune", "--export", e, "--keep", "0"]);
    assert_eq!(code, 1);
    assert!(err.contains("running process"), "{err}");
    assert!(live.exists() && export.join("snapshots").join(version(2)).exists());
}

#[test]
fn refuses_without_a_valid_pointer_or_selected_snapshot_and_bad_arguments() {
    let dir = tempdir().unwrap();
    let export = dir.path();
    let e = export.to_str().unwrap();
    snapshot(export, &version(1), 10);
    assert_eq!(run(&["snapshots", "prune", "--export", e]).0, 1);
    point(export, &version(7)); // selected snapshot does not exist
    assert_eq!(run(&["snapshots", "prune", "--export", e]).0, 1);
    fs::write(
        export.join("current.json"),
        r#"{"version":"../..","snapshot":"x"}"#,
    )
    .unwrap();
    assert_eq!(run(&["snapshots", "prune", "--export", e]).0, 1);
    assert_eq!(names(export), vec![version(1)]);
    point(export, &version(1));
    assert_eq!(
        run(&["snapshots", "prune", "--export", e, "--keep", "x"]).0,
        1
    );
    assert_eq!(run(&["snapshots", "prune", "--export", e, "--bogus"]).0, 1);
    assert_eq!(run(&["snapshots", "nope"]).0, 1);
}

#[cfg(unix)]
#[test]
fn never_follows_symlinks_out_of_snapshots() {
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("keep.txt"), "x").unwrap();
    let export = dir.path();
    snapshot(export, &version(1), 10);
    point(export, &version(1));
    std::os::unix::fs::symlink(outside.path(), export.join("snapshots").join(version(2))).unwrap();
    let (code, out, _) = run(&[
        "snapshots",
        "prune",
        "--export",
        export.to_str().unwrap(),
        "--keep",
        "0",
    ]);
    assert_eq!(code, 0);
    assert!(out.contains("Skipped"), "{out}");
    assert!(outside.path().join("keep.txt").exists());
}
