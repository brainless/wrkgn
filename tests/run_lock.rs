mod common;

use std::fs;
use std::process::Command;
use std::time::{Duration, SystemTime};

use wrkgn::run::{self, LockError, lock};

use common::fresh_store;

fn dead_pid() -> u32 {
    let mut child = Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn second_live_run_is_rejected_even_from_the_same_process() {
    let (_dir, db, _store) = fresh_store();
    let first = lock::acquire(&db, "monthly").unwrap();
    assert!(!first.took_over);
    match lock::acquire(&db, "monthly") {
        Err(LockError::Held { pid, path }) => {
            assert_eq!(pid, std::process::id());
            assert_eq!(path, lock::lock_path(&db, "monthly"));
        }
        other => panic!("expected Held, got {other:?}"),
    }
    // Another command has its own lock.
    assert!(lock::acquire(&db, "show_hn").is_ok());
    drop(first);
    assert!(lock::acquire(&db, "monthly").is_ok());
}

#[test]
fn held_message_explains_manual_removal() {
    let (_dir, db, _store) = fresh_store();
    let _first = lock::acquire(&db, "monthly").unwrap();
    let message = lock::acquire(&db, "monthly").unwrap_err().to_string();
    assert!(message.contains("delete the lock file"), "{message}");
}

#[test]
fn stale_lock_is_taken_over_and_the_killed_run_is_detected() {
    let (_dir, db, mut store) = fresh_store();
    let pid = dead_pid();
    fs::write(lock::lock_path(&db, "monthly"), pid.to_string()).unwrap();
    let old = store.begin_driver_run("monthly", pid, 100).unwrap();
    let other = store.begin_driver_run("show_hn", pid, 100).unwrap();

    let session = run::begin(&mut store, &db, "monthly", 200).unwrap();
    assert_eq!(session.killed_runs, vec![old]);
    assert_eq!(session.took_over_pid, Some(pid));
    let killed = store.driver_run(old).unwrap().unwrap();
    assert_eq!(killed.status, "killed_detected");
    assert_eq!(killed.finished_at, Some(200));
    // Only the same command's runs are touched.
    assert_eq!(store.driver_run(other).unwrap().unwrap().status, "running");
    let current = store.driver_run(session.run_id).unwrap().unwrap();
    assert_eq!(current.status, "running");
    assert_eq!(current.pid, std::process::id());
}

#[test]
fn begin_rejects_a_live_second_run_without_touching_records() {
    let (_dir, db, mut store) = fresh_store();
    let first = run::begin(&mut store, &db, "monthly", 10).unwrap();
    let error = run::begin(&mut store, &db, "monthly", 20).unwrap_err();
    assert!(error.contains("another run is active"), "{error}");
    assert_eq!(
        store.driver_run(first.run_id).unwrap().unwrap().status,
        "running"
    );
}

#[test]
fn drop_removes_only_a_lock_it_owns() {
    let (_dir, db, _store) = fresh_store();
    let path = lock::lock_path(&db, "monthly");
    let acquired = lock::acquire(&db, "monthly").unwrap();
    fs::write(&path, "1").unwrap();
    drop(acquired);
    assert_eq!(fs::read_to_string(&path).unwrap(), "1");
    fs::remove_file(&path).unwrap();

    let acquired = lock::acquire(&db, "monthly").unwrap();
    drop(acquired);
    assert!(!path.exists());
}

#[test]
fn unreadable_lock_is_held_when_fresh_and_stale_when_old() {
    let (_dir, db, _store) = fresh_store();
    let path = lock::lock_path(&db, "monthly");
    fs::write(&path, "not a pid").unwrap();
    assert!(matches!(
        lock::acquire(&db, "monthly"),
        Err(LockError::HeldUnknown { .. })
    ));
    let old = SystemTime::now() - Duration::from_secs(120);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let acquired = lock::acquire(&db, "monthly").unwrap();
    assert!(acquired.took_over);
    assert_eq!(acquired.took_over_pid, None);
}
