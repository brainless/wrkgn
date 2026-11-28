//! Maintainer commands over coverage units (`wrkgn runs ...`): list,
//! acknowledge, unacknowledge and retry. Local only: no network, no model.

use std::path::Path;

use super::report::{FailedItem, failure_lines};
use super::{MAX_UNIT_ATTEMPTS, lock};
use crate::monthly::Month;
use crate::show_hn_discovery::UtcDay;
use crate::store::{CoverageUnit, QueueCounts, Store, UnitState};

/// Maps `--command` (`monthly` or `show-hn`) to the stored command name.
pub fn command_name(value: &str) -> Result<&'static str, String> {
    match value {
        "monthly" => Ok("monthly"),
        "show-hn" | "show_hn" => Ok("show_hn"),
        other => Err(format!(
            "unknown command {other:?}; expected monthly or show-hn"
        )),
    }
}

/// Validates a unit identifier (`YYYY-MM` or `YYYY-MM-DD`) and returns its key.
pub fn normalize_unit(command: &str, value: &str) -> Result<String, String> {
    if command == "monthly" {
        Month::parse(value).map(|m| m.to_string())
    } else {
        UtcDay::parse(value).map(|d| d.to_string())
    }
    .map_err(|error| format!("invalid unit {value:?}: {error}"))
}

fn find(store: &Store, command: &str, unit: &str) -> Result<CoverageUnit, String> {
    store
        .unit(command, unit)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("unknown unit {unit}: no coverage recorded for {command}"))
}

fn require_failed(unit: &CoverageUnit, action: &str) -> Result<(), String> {
    if unit.state == UnitState::Failed {
        Ok(())
    } else {
        Err(format!(
            "cannot {action} {}: it is {}, only failed units can be changed",
            unit.unit_key,
            unit.state.as_str()
        ))
    }
}

/// Takes the command's run lock for a mutation; a live run refuses it.
fn lock_for_change(db: &Path, command: &str) -> Result<lock::RunLock, String> {
    lock::acquire(db, command)
        .map(|acquired| acquired.lock)
        .map_err(|error| format!("{error}; wait for the running {command} run to finish"))
}

pub fn acknowledge(
    store: &mut Store,
    db: &Path,
    command: &str,
    unit: &str,
    note: Option<&str>,
    now: i64,
) -> Result<String, String> {
    let unit = normalize_unit(command, unit)?;
    let _lock = lock_for_change(db, command)?;
    let row = find(store, command, &unit)?;
    require_failed(&row, "acknowledge")?;
    store
        .set_unit_acknowledgement(command, &unit, Some((note.unwrap_or(""), now)))
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Acknowledged {unit}: skipped by runs, no longer a failure, still listed in reports. Use `wrkgn runs unacknowledge` or `retry` to reopen it."
    ))
}

pub fn unacknowledge(
    store: &mut Store,
    db: &Path,
    command: &str,
    unit: &str,
) -> Result<String, String> {
    let unit = normalize_unit(command, unit)?;
    let _lock = lock_for_change(db, command)?;
    let row = find(store, command, &unit)?;
    if row.state != UnitState::Failed || row.acknowledged_at.is_none() {
        return Err(format!("{unit} is not acknowledged"));
    }
    store
        .set_unit_acknowledgement(command, &unit, None)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Unacknowledged {unit}: it counts as a failure again ({}/{MAX_UNIT_ATTEMPTS} attempts used).",
        row.attempts
    ))
}

pub fn retry(
    store: &mut Store,
    db: &Path,
    command: &str,
    unit: &str,
    now: i64,
) -> Result<String, String> {
    let unit = normalize_unit(command, unit)?;
    let _lock = lock_for_change(db, command)?;
    let row = find(store, command, &unit)?;
    require_failed(&row, "retry")?;
    let items = store
        .reset_failed_unit(command, &unit, now)
        .map_err(|e| e.to_string())?
        .unwrap_or(0);
    Ok(format!(
        "Reset {unit}: attempts back to 0, {items} failed queue item(s) pending. The next default run retries it."
    ))
}

/// Failed, pending, in-progress and acknowledged units, one per line.
pub fn list(store: &Store, command: &str) -> Result<String, String> {
    let mut out = String::new();
    for unit in store.all_units(command).map_err(|e| e.to_string())? {
        if unit.state == UnitState::Done {
            continue;
        }
        let counts = store
            .queue_counts(command, &unit.unit_key)
            .map_err(|e| e.to_string())?;
        let label = match (unit.state, unit.acknowledged_at) {
            (UnitState::Failed, Some(_)) => "acknowledged".to_owned(),
            (UnitState::Failed, None) if unit.attempts >= MAX_UNIT_ATTEMPTS => {
                "exhausted".to_owned()
            }
            (UnitState::Failed, None) => "failed (will retry)".to_owned(),
            (state, _) => state.as_str().to_owned(),
        };
        out.push_str(&format!(
            "{}  {label}  {}/{MAX_UNIT_ATTEMPTS} attempts  {}",
            unit.unit_key,
            unit.attempts,
            queue_text(&counts)
        ));
        if let Some(note) = &unit.acknowledged_note {
            out.push_str(&format!("  note: {note}"));
        }
        if let Some(error) = &unit.last_error {
            out.push_str(&format!("  last error: {error}"));
        }
        out.push('\n');
        if unit.state == UnitState::Failed {
            let label = if command == "monthly" {
                "thread"
            } else {
                "story"
            };
            let failures: Vec<FailedItem> = store
                .failed_items(command, &unit.unit_key)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|(id, error)| FailedItem {
                    id: Some(id),
                    error: error.unwrap_or_default(),
                })
                .collect();
            if !failures.is_empty() {
                out.push_str(&failure_lines(&unit.unit_key, label, &failures, "    "));
            }
        }
    }
    if out.is_empty() {
        out.push_str("No pending, failed or acknowledged units.\n");
    }
    Ok(out)
}

fn queue_text(counts: &QueueCounts) -> String {
    format!(
        "items: {} done, {} pending, {} failed",
        counts.done, counts.pending, counts.failed
    )
}
