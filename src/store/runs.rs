//! Driver run records, coverage units, the item queue and grouped run errors
//! (migration 008). All methods run on the calling thread.

use rusqlite::{OptionalExtension, params};

use super::Store;
use crate::run::errors::ErrorGroup;

/// Longest stored `last_error` (characters); matches the table CHECK.
pub const LAST_ERROR_MAX_CHARS: usize = 2000;
/// Longest stored error sample (characters); matches the table CHECK.
pub const SAMPLE_STORE_MAX_CHARS: usize = 2100;

fn cap(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunCounts {
    pub units_done: i64,
    pub items_collected: i64,
    pub items_processed: i64,
    pub items_approved: i64,
    pub failures: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverRun {
    pub id: i64,
    pub command: String,
    pub pid: u32,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: String,
    pub counts: RunCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitState {
    Pending,
    InProgress,
    Done,
    Failed,
}

impl UnitState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> rusqlite::Result<Self> {
        Ok(match value {
            "pending" => Self::Pending,
            "in_progress" => Self::InProgress,
            "done" => Self::Done,
            "failed" => Self::Failed,
            other => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    format!("unknown unit state {other}").into(),
                ));
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageUnit {
    pub command: String,
    pub unit_key: String,
    pub state: UnitState,
    pub attempts: u32,
    pub first_seen_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
    pub last_run_id: Option<i64>,
    pub last_error: Option<String>,
    /// Set when the maintainer acknowledged (skipped) this failed unit.
    pub acknowledged_at: Option<i64>,
    pub acknowledged_note: Option<String>,
}

/// Longest stored acknowledgement note (characters); matches the column CHECK.
pub const NOTE_MAX_CHARS: usize = 500;

/// Queue rows of one unit by state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueueCounts {
    pub pending: usize,
    pub failed: usize,
    pub done: usize,
}

impl QueueCounts {
    pub fn total(&self) -> usize {
        self.pending + self.failed + self.done
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitUpdate<'a> {
    pub command: &'a str,
    pub unit: &'a str,
    pub state: UnitState,
    pub run_id: Option<i64>,
    pub error: Option<&'a str>,
    pub now: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueKind {
    Thread,
    Story,
}

impl QueueKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Thread => "thread",
            Self::Story => "story",
        }
    }

    fn parse(value: &str) -> Self {
        if value == "story" {
            Self::Story
        } else {
            Self::Thread
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemState {
    Pending,
    Done,
    Failed,
}

impl ItemState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "done" => Self::Done,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueItem {
    pub command: String,
    pub kind: QueueKind,
    pub item_id: i64,
    pub unit_key: String,
    pub position: i64,
    pub state: ItemState,
    pub attempts: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemUpdate<'a> {
    pub command: &'a str,
    pub kind: QueueKind,
    pub item_id: i64,
    pub state: ItemState,
    pub run_id: Option<i64>,
    pub error: Option<&'a str>,
    pub now: i64,
}

const UNIT_COLUMNS: &str = "command, unit_key, state, attempts, first_seen_at, updated_at,
     completed_at, last_run_id, last_error, acknowledged_at, acknowledged_note";

fn read_unit(row: &rusqlite::Row<'_>) -> rusqlite::Result<CoverageUnit> {
    let state: String = row.get(2)?;
    Ok(CoverageUnit {
        command: row.get(0)?,
        unit_key: row.get(1)?,
        state: UnitState::parse(&state)?,
        attempts: row.get(3)?,
        first_seen_at: row.get(4)?,
        updated_at: row.get(5)?,
        completed_at: row.get(6)?,
        last_run_id: row.get(7)?,
        last_error: row.get(8)?,
        acknowledged_at: row.get(9)?,
        acknowledged_note: row.get(10)?,
    })
}

impl Store {
    pub fn begin_driver_run(&mut self, command: &str, pid: u32, now: i64) -> rusqlite::Result<i64> {
        self.connection.execute(
            "INSERT INTO driver_runs (command, pid, started_at, status)
             VALUES (?1, ?2, ?3, 'running')",
            params![command, pid, now],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn finish_driver_run(
        &mut self,
        id: i64,
        status: &str,
        now: i64,
        counts: &RunCounts,
    ) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "UPDATE driver_runs SET status = ?2, finished_at = ?3, units_done = ?4,
                 items_collected = ?5, items_processed = ?6, items_approved = ?7,
                 failures = ?8
             WHERE id = ?1",
            params![
                id,
                status,
                now,
                counts.units_done,
                counts.items_collected,
                counts.items_processed,
                counts.items_approved,
                counts.failures
            ],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// Marks every `running` row of `command` as `killed_detected`. The caller
    /// holds the command lock, so no such run can still be live.
    pub fn mark_running_driver_runs_killed(
        &mut self,
        command: &str,
        now: i64,
    ) -> rusqlite::Result<Vec<i64>> {
        let transaction = self.connection.transaction()?;
        let ids: Vec<i64> = {
            let mut statement = transaction.prepare(
                "SELECT id FROM driver_runs WHERE command = ?1 AND status = 'running' ORDER BY id",
            )?;
            statement
                .query_map([command], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for id in &ids {
            transaction.execute(
                "UPDATE driver_runs SET status = 'killed_detected', finished_at = ?2
                 WHERE id = ?1",
                params![id, now],
            )?;
        }
        transaction.commit()?;
        Ok(ids)
    }

    pub fn driver_run(&self, id: i64) -> rusqlite::Result<Option<DriverRun>> {
        self.connection
            .query_row(
                "SELECT id, command, pid, started_at, finished_at, status, units_done,
                    items_collected, items_processed, items_approved, failures
                 FROM driver_runs WHERE id = ?1",
                [id],
                |row| {
                    Ok(DriverRun {
                        id: row.get(0)?,
                        command: row.get(1)?,
                        pid: row.get(2)?,
                        started_at: row.get(3)?,
                        finished_at: row.get(4)?,
                        status: row.get(5)?,
                        counts: RunCounts {
                            units_done: row.get(6)?,
                            items_collected: row.get(7)?,
                            items_processed: row.get(8)?,
                            items_approved: row.get(9)?,
                            failures: row.get(10)?,
                        },
                    })
                },
            )
            .optional()
    }

    /// Creates the unit or changes its state. A `done` unit is never lowered.
    pub fn upsert_unit(
        &mut self,
        command: &str,
        unit: &str,
        state: UnitState,
        now: i64,
    ) -> rusqlite::Result<()> {
        upsert_unit(&self.connection, command, unit, state, now)
    }

    /// Marks the unit `in_progress` and counts one attempt; returns the total.
    pub fn begin_unit_attempt(
        &mut self,
        command: &str,
        unit: &str,
        run_id: i64,
        now: i64,
    ) -> rusqlite::Result<u32> {
        let transaction = self.connection.transaction()?;
        ensure_unit(&transaction, command, unit, now)?;
        transaction.execute(
            "UPDATE coverage_units SET state = 'in_progress', attempts = attempts + 1,
                 updated_at = ?3, last_run_id = ?4,
                 acknowledged_at = NULL, acknowledged_note = NULL
             WHERE command = ?1 AND unit_key = ?2",
            params![command, unit, now, run_id],
        )?;
        let attempts = transaction.query_row(
            "SELECT attempts FROM coverage_units WHERE command = ?1 AND unit_key = ?2",
            params![command, unit],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok(attempts)
    }

    pub fn finish_unit(&mut self, update: &UnitUpdate<'_>) -> rusqlite::Result<()> {
        let error = update.error.map(|e| cap(e, LAST_ERROR_MAX_CHARS));
        let transaction = self.connection.transaction()?;
        ensure_unit(&transaction, update.command, update.unit, update.now)?;
        transaction.execute(
            "UPDATE coverage_units SET state = ?3, updated_at = ?4,
                 completed_at = CASE WHEN ?3 = 'done' THEN ?4 ELSE completed_at END,
                 attempts = CASE WHEN ?3 = 'done' THEN 0 ELSE attempts END,
                 last_run_id = COALESCE(?5, last_run_id), last_error = ?6
             WHERE command = ?1 AND unit_key = ?2",
            params![
                update.command,
                update.unit,
                update.state.as_str(),
                update.now,
                update.run_id,
                error
            ],
        )?;
        transaction.commit()
    }

    /// Starts a forced re-walk: the unit becomes `in_progress` on its first
    /// attempt and every queued item is pending again, so the whole unit is
    /// collected and processed once more.
    pub fn requeue_unit(&mut self, command: &str, unit: &str, now: i64) -> rusqlite::Result<()> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "UPDATE run_queue SET state = 'pending', attempts = 0, last_error = NULL,
                 updated_at = ?3
             WHERE command = ?1 AND unit_key = ?2",
            params![command, unit, now],
        )?;
        transaction.execute(
            "UPDATE coverage_units SET state = 'in_progress', attempts = 1, updated_at = ?3
             WHERE command = ?1 AND unit_key = ?2",
            params![command, unit, now],
        )?;
        transaction.commit()
    }

    /// Gives back the attempt of a unit that was cut short by a limit or stop.
    pub fn refund_unit_attempt(&mut self, command: &str, unit: &str) -> rusqlite::Result<()> {
        self.connection.execute(
            "UPDATE coverage_units SET attempts = MAX(attempts - 1, 0)
             WHERE command = ?1 AND unit_key = ?2",
            params![command, unit],
        )?;
        Ok(())
    }

    pub fn queue_counts(&self, command: &str, unit: &str) -> rusqlite::Result<QueueCounts> {
        let mut counts = QueueCounts::default();
        let mut statement = self.connection.prepare(
            "SELECT state, COUNT(*) FROM run_queue
             WHERE command = ?1 AND unit_key = ?2 GROUP BY state",
        )?;
        let rows = statement.query_map(params![command, unit], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?;
        for row in rows {
            let (state, count) = row?;
            match state.as_str() {
                "pending" => counts.pending = count,
                "failed" => counts.failed = count,
                _ => counts.done = count,
            }
        }
        Ok(counts)
    }

    /// Creates a unit with already-known items unless the unit exists. Returns
    /// whether it was created; an existing unit is never changed.
    pub fn seed_unit(
        &mut self,
        command: &str,
        kind: QueueKind,
        unit: &str,
        state: UnitState,
        items: &[(i64, bool)],
        now: i64,
    ) -> rusqlite::Result<bool> {
        let transaction = self.connection.transaction()?;
        let created = transaction.execute(
            "INSERT OR IGNORE INTO coverage_units
             (command, unit_key, state, first_seen_at, updated_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?4, CASE WHEN ?3 = 'done' THEN ?4 END)",
            params![command, unit, state.as_str(), now],
        )? > 0;
        if created {
            for (position, (item, done)) in items.iter().enumerate() {
                transaction.execute(
                    "INSERT OR IGNORE INTO run_queue
                     (command, kind, item_id, unit_key, position, state, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        command,
                        kind.as_str(),
                        item,
                        unit,
                        position as i64,
                        if *done { "done" } else { "pending" },
                        now
                    ],
                )?;
            }
        }
        transaction.commit()?;
        Ok(created)
    }

    pub fn unit(&self, command: &str, unit: &str) -> rusqlite::Result<Option<CoverageUnit>> {
        self.connection
            .query_row(
                &format!(
                    "SELECT {UNIT_COLUMNS} FROM coverage_units
                     WHERE command = ?1 AND unit_key = ?2"
                ),
                params![command, unit],
                read_unit,
            )
            .optional()
    }

    /// Units in any of `states`, ordered by key. `failed` units whose attempts
    /// reached `max_attempts` are excluded; see `exhausted_units`.
    pub fn units_in_state(
        &self,
        command: &str,
        states: &[UnitState],
        max_attempts: u32,
    ) -> rusqlite::Result<Vec<CoverageUnit>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {UNIT_COLUMNS} FROM coverage_units WHERE command = ?1 ORDER BY unit_key"
        ))?;
        let units = statement
            .query_map([command], read_unit)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(units
            .into_iter()
            .filter(|unit| states.contains(&unit.state))
            .filter(|unit| unit.state != UnitState::Failed || unit.attempts < max_attempts)
            .collect())
    }

    /// Failed units that used all their attempts and need manual action.
    /// Acknowledged units are listed by `acknowledged_units` instead.
    pub fn exhausted_units(
        &self,
        command: &str,
        max_attempts: u32,
    ) -> rusqlite::Result<Vec<CoverageUnit>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {UNIT_COLUMNS} FROM coverage_units
             WHERE command = ?1 AND state = 'failed' AND attempts >= ?2
               AND acknowledged_at IS NULL ORDER BY unit_key"
        ))?;
        statement
            .query_map(params![command, max_attempts], read_unit)?
            .collect()
    }

    /// Failed units the maintainer acknowledged, ordered by key.
    pub fn acknowledged_units(&self, command: &str) -> rusqlite::Result<Vec<CoverageUnit>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {UNIT_COLUMNS} FROM coverage_units
             WHERE command = ?1 AND state = 'failed' AND acknowledged_at IS NOT NULL
             ORDER BY unit_key"
        ))?;
        statement.query_map([command], read_unit)?.collect()
    }

    /// Every unit of a command, ordered by key.
    pub fn all_units(&self, command: &str) -> rusqlite::Result<Vec<CoverageUnit>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {UNIT_COLUMNS} FROM coverage_units WHERE command = ?1 ORDER BY unit_key"
        ))?;
        statement.query_map([command], read_unit)?.collect()
    }

    /// Marks a `failed` unit acknowledged (`Some((note, now))`) or clears the
    /// mark (`None`). Returns whether a failed unit was changed; the state and
    /// attempts are untouched.
    pub fn set_unit_acknowledgement(
        &mut self,
        command: &str,
        unit: &str,
        acknowledgement: Option<(&str, i64)>,
    ) -> rusqlite::Result<bool> {
        let (note, at) = match acknowledgement {
            Some((note, now)) if !note.is_empty() => (Some(cap(note, NOTE_MAX_CHARS)), Some(now)),
            Some((_, now)) => (None, Some(now)),
            None => (None, None),
        };
        Ok(self.connection.execute(
            "UPDATE coverage_units SET acknowledged_at = ?3, acknowledged_note = ?4
             WHERE command = ?1 AND unit_key = ?2 AND state = 'failed'",
            params![command, unit, at, note],
        )? > 0)
    }

    /// Reopens a `failed` unit for the next default run: attempts return to 0,
    /// the acknowledgement is cleared, its failed queue items become pending
    /// with no attempts, and the unit is `pending`. Done and pending items are
    /// untouched. Returns the number of queue items reset, or `None` when the
    /// unit is not failed.
    pub fn reset_failed_unit(
        &mut self,
        command: &str,
        unit: &str,
        now: i64,
    ) -> rusqlite::Result<Option<usize>> {
        let transaction = self.connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE coverage_units SET state = 'pending', attempts = 0, last_error = NULL,
                 acknowledged_at = NULL, acknowledged_note = NULL, updated_at = ?3
             WHERE command = ?1 AND unit_key = ?2 AND state = 'failed'",
            params![command, unit, now],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let items = transaction.execute(
            "UPDATE run_queue SET state = 'pending', attempts = 0, last_error = NULL,
                 updated_at = ?3
             WHERE command = ?1 AND unit_key = ?2 AND state = 'failed'",
            params![command, unit, now],
        )?;
        transaction.commit()?;
        Ok(Some(items))
    }

    /// Detects exhausted units whose failed items were settled by a manual
    /// `wrkgn collect --thread` then `wrkgn process --thread`: for every failed
    /// item the latest collection of the thread is complete and began after the
    /// item last failed, and the latest processing run began after that
    /// collection finished and completed. Such a unit is reopened like
    /// `reset_failed_unit`, so the next pass re-collects and approves the
    /// threads and skips already-extracted groups without model calls.
    /// Acknowledged units are left alone. Returns the reopened unit keys.
    pub fn reopen_manually_resolved_units(
        &mut self,
        command: &str,
        max_attempts: u32,
        now: i64,
    ) -> rusqlite::Result<Vec<String>> {
        let candidates = self.exhausted_units(command, max_attempts)?;
        let mut reopened = Vec::new();
        for unit in candidates {
            let mut statement = self.connection.prepare(
                "SELECT item_id, updated_at FROM run_queue
                 WHERE command = ?1 AND unit_key = ?2 AND state = 'failed'",
            )?;
            let failed = statement
                .query_map(params![command, unit.unit_key], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            if failed.is_empty() {
                continue;
            }
            let mut settled = true;
            for (item, failed_at) in failed {
                if !self.manually_settled(item, failed_at)? {
                    settled = false;
                    break;
                }
            }
            if settled
                && self
                    .reset_failed_unit(command, &unit.unit_key, now)?
                    .is_some()
            {
                reopened.push(unit.unit_key);
            }
        }
        Ok(reopened)
    }

    fn manually_settled(&self, thread: i64, failed_at: i64) -> rusqlite::Result<bool> {
        let collected: Option<i64> = self
            .connection
            .query_row(
                "SELECT finished_at FROM collection_runs
                 WHERE thread_id = ?1 AND started_at >= ?2 AND status = 'complete'
                   AND id = (SELECT MAX(id) FROM collection_runs WHERE thread_id = ?1)",
                params![thread, failed_at],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let Some(collected) = collected else {
            return Ok(false);
        };
        let processed: Option<String> = self
            .connection
            .query_row(
                "SELECT status FROM processing_runs
                 WHERE thread_id = ?1 AND started_at >= ?2
                   AND id = (SELECT MAX(id) FROM processing_runs WHERE thread_id = ?1)",
                params![thread, collected],
                |row| row.get(0),
            )
            .optional()?;
        Ok(processed.as_deref() == Some("complete"))
    }

    /// Ensures the unit exists and queues new items; existing items keep their
    /// state. Returns how many items were newly queued.
    pub fn enqueue_items(
        &mut self,
        command: &str,
        kind: QueueKind,
        unit: &str,
        items: &[i64],
        now: i64,
    ) -> rusqlite::Result<usize> {
        let transaction = self.connection.transaction()?;
        ensure_unit(&transaction, command, unit, now)?;
        // New items queue after any existing ones, so a later discovery pass
        // never reorders work that is already waiting.
        let base: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM run_queue
             WHERE command = ?1 AND unit_key = ?2",
            params![command, unit],
            |row| row.get(0),
        )?;
        let mut added = 0;
        {
            let mut statement = transaction.prepare(
                "INSERT OR IGNORE INTO run_queue
                 (command, kind, item_id, unit_key, position, state, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
            )?;
            for (position, item) in items.iter().enumerate() {
                added += statement.execute(params![
                    command,
                    kind.as_str(),
                    item,
                    unit,
                    base + position as i64,
                    now
                ])?;
            }
        }
        transaction.commit()?;
        Ok(added)
    }

    /// Pending items, plus failed items below the attempt cap, in queue order.
    pub fn pending_items(
        &self,
        command: &str,
        unit: &str,
        max_attempts: u32,
    ) -> rusqlite::Result<Vec<QueueItem>> {
        let mut statement = self.connection.prepare(
            "SELECT command, kind, item_id, unit_key, position, state, attempts, last_error
             FROM run_queue
             WHERE command = ?1 AND unit_key = ?2
               AND (state = 'pending' OR (state = 'failed' AND attempts < ?3))
             ORDER BY position, item_id",
        )?;
        statement
            .query_map(params![command, unit, max_attempts], |row| {
                let kind: String = row.get(1)?;
                let state: String = row.get(5)?;
                Ok(QueueItem {
                    command: row.get(0)?,
                    kind: QueueKind::parse(&kind),
                    item_id: row.get(2)?,
                    unit_key: row.get(3)?,
                    position: row.get(4)?,
                    state: ItemState::parse(&state),
                    attempts: row.get(6)?,
                    last_error: row.get(7)?,
                })
            })?
            .collect()
    }

    /// Records the outcome of one attempt. Any state other than `pending`
    /// counts an attempt.
    pub fn finish_item(&mut self, update: &ItemUpdate<'_>) -> rusqlite::Result<()> {
        let error = update.error.map(|e| cap(e, LAST_ERROR_MAX_CHARS));
        let changed = self.connection.execute(
            "UPDATE run_queue SET state = ?4,
                 attempts = attempts + CASE WHEN ?4 = 'pending' THEN 0 ELSE 1 END,
                 last_error = ?5, updated_at = ?6, last_run_id = COALESCE(?7, last_run_id)
             WHERE command = ?1 AND kind = ?2 AND item_id = ?3",
            params![
                update.command,
                update.kind.as_str(),
                update.item_id,
                update.state.as_str(),
                error,
                update.now,
                update.run_id
            ],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn record_run_error(&mut self, run_id: i64, group: &ErrorGroup) -> rusqlite::Result<()> {
        self.connection.execute(
            "INSERT INTO run_errors
             (run_id, signature, stage, kind, count, first_item_id, last_item_id, sample)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(run_id, signature, stage, kind) DO UPDATE SET
                 count = excluded.count, first_item_id = excluded.first_item_id,
                 last_item_id = excluded.last_item_id, sample = excluded.sample",
            params![
                run_id,
                group.signature,
                group.stage,
                group.kind,
                group.count as i64,
                group.first_item,
                group.last_item,
                cap(&group.sample, SAMPLE_STORE_MAX_CHARS)
            ],
        )?;
        Ok(())
    }

    /// Stored error groups for a run: count descending, then signature.
    pub fn run_errors(&self, run_id: i64) -> rusqlite::Result<Vec<ErrorGroup>> {
        let mut statement = self.connection.prepare(
            "SELECT signature, stage, kind, count, first_item_id, last_item_id, sample
             FROM run_errors WHERE run_id = ?1 ORDER BY count DESC, signature, stage, kind",
        )?;
        statement
            .query_map([run_id], |row| {
                Ok(ErrorGroup {
                    signature: row.get(0)?,
                    stage: row.get(1)?,
                    kind: row.get(2)?,
                    count: row.get::<_, i64>(3)? as usize,
                    first_item: row.get(4)?,
                    last_item: row.get(5)?,
                    sample: row.get(6)?,
                })
            })?
            .collect()
    }
}

/// Inserts a missing unit; for an existing unit applies `state` unless it
/// would lower `done`.
fn upsert_unit(
    connection: &rusqlite::Connection,
    command: &str,
    unit: &str,
    state: UnitState,
    now: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO coverage_units (command, unit_key, state, first_seen_at, updated_at,
             completed_at)
         VALUES (?1, ?2, ?3, ?4, ?4, CASE WHEN ?3 = 'done' THEN ?4 END)
         ON CONFLICT(command, unit_key) DO UPDATE SET
             state = CASE WHEN coverage_units.state = 'done' THEN 'done' ELSE excluded.state END,
             updated_at = CASE WHEN coverage_units.state = 'done' THEN coverage_units.updated_at
                               ELSE excluded.updated_at END,
             completed_at = CASE WHEN coverage_units.state = 'done' THEN coverage_units.completed_at
                                 WHEN excluded.state = 'done' THEN excluded.updated_at
                                 ELSE coverage_units.completed_at END",
        params![command, unit, state.as_str(), now],
    )?;
    Ok(())
}

/// Creates a missing `pending` unit and leaves an existing one untouched.
fn ensure_unit(
    connection: &rusqlite::Connection,
    command: &str,
    unit: &str,
    now: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT OR IGNORE INTO coverage_units (command, unit_key, state, first_seen_at, updated_at)
         VALUES (?1, ?2, 'pending', ?3, ?3)",
        params![command, unit, now],
    )?;
    Ok(())
}
