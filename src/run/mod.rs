//! Shared infrastructure for unattended, resumable runs: lock, interrupt
//! handling, failure circuit breaker, error grouping, live status and the
//! end-of-run report (Epic 09). `driver` is the shared run driver; both
//! `wrkgn-monthly` and `wrkgn-show-hn` use it.

pub mod control;
pub mod driver;
pub mod errors;
pub mod lock;
pub mod report;
pub mod signals;
pub mod status;

use std::path::Path;
use std::sync::Arc;

use crate::store::{RunCounts, Store};

pub use control::{Api, BREAKER_THRESHOLD, Breaker, RunControl};
pub use errors::{ErrorGroup, ErrorLog};
pub use lock::{LockError, RunLock};
pub use report::{LimitInfo, Outcome, RunSummary, render_report, write_report};

/// A unit or item that fails this many attempts stays `failed` and is listed
/// for manual action.
pub const MAX_UNIT_ATTEMPTS: u32 = 3;

/// Everything a driver holds for the duration of one run. Dropping it
/// releases the lock.
#[derive(Debug)]
pub struct RunSession {
    pub lock: RunLock,
    pub control: Arc<RunControl>,
    pub run_id: i64,
    /// Earlier `running` rows of this command that were found killed.
    pub killed_runs: Vec<i64>,
    pub took_over_pid: Option<u32>,
}

/// Takes the command lock, marks runs left `running` by a killed process as
/// `killed_detected` (the lock guarantees none is live), and records a new run.
pub fn begin(store: &mut Store, db: &Path, command: &str, now: i64) -> Result<RunSession, String> {
    let acquired = lock::acquire(db, command).map_err(|error| error.to_string())?;
    let killed_runs = store
        .mark_running_driver_runs_killed(command, now)
        .map_err(|error| error.to_string())?;
    if acquired.took_over || !killed_runs.is_empty() {
        let pid = acquired
            .took_over_pid
            .map(|pid| format!(" (PID {pid})"))
            .unwrap_or_default();
        eprintln!(
            "note: a previous {command} run was killed{pid}; took over its lock{}",
            if killed_runs.is_empty() {
                String::new()
            } else {
                format!(", marked run(s) {killed_runs:?} killed_detected")
            }
        );
    }
    let run_id = store
        .begin_driver_run(command, acquired.lock.pid(), now)
        .map_err(|error| error.to_string())?;
    Ok(RunSession {
        lock: acquired.lock,
        control: Arc::new(RunControl::default()),
        run_id,
        killed_runs,
        took_over_pid: acquired.took_over_pid,
    })
}

/// Records the outcome, counts and grouped errors of the run.
pub fn finish(
    store: &mut Store,
    session: &RunSession,
    outcome: Outcome,
    summary: &RunSummary,
    errors: &ErrorLog,
    now: i64,
) -> Result<(), String> {
    let failures = errors.total() + summary.failed_units.len() + summary.exhausted_units.len();
    let counts = RunCounts {
        units_done: summary.units_done.len() as i64,
        items_collected: summary.items_collected as i64,
        items_processed: summary.items_processed as i64,
        items_approved: summary.items_approved as i64,
        failures: failures as i64,
    };
    store
        .finish_driver_run(session.run_id, outcome.as_str(), now, &counts)
        .map_err(|error| error.to_string())?;
    for group in errors.groups_sorted() {
        store
            .record_run_error(session.run_id, group)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
