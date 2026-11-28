//! The shared run driver of Epic 09. A workload supplies discovery and the
//! per-item work; the driver supplies coverage, the item queue, resume, head
//! refresh, backfill, limits, export points, live status and the end report.
//! It neither takes the lock nor installs signal handlers: the caller does
//! `run::begin` and passes the session in.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;
use std::io::Write;
use std::sync::Arc;

use super::control::RunControl;
use super::errors::ErrorLog;
use super::report::{FailedItem, LimitInfo, Outcome, RunSummary, write_report};
use super::status::{Clock, Snapshot, StatusLine};
use super::{RunSession, finish};
use crate::processing;
use crate::store::{ItemState, ItemUpdate, QueueKind, Store, UnitState, UnitUpdate};
use crate::workflow;

/// One error recorded for an item. Each becomes a count in the grouped report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemError {
    pub stage: &'static str,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemOutcome {
    /// Collected, processed and approved per policy.
    Done { approved: usize },
    /// The item failed; the errors are recorded by the driver (at least one).
    Failed { errors: Vec<ItemError> },
    /// The run was stopped before the item finished. Not a failure and no
    /// attempt is counted; the item stays pending.
    Stopped,
}

/// Maps a workflow error for one item to its outcome, shared by workloads.
/// `first_failure` is the first fetch failure of an incomplete collection.
pub fn outcome_from_error(
    error: workflow::Error,
    control: &RunControl,
    errors: &mut ErrorLog,
    id: i64,
    first_failure: Option<String>,
) -> ItemOutcome {
    match error {
        workflow::Error::Stopped => ItemOutcome::Stopped,
        workflow::Error::Operation(message) => {
            let error = if message.starts_with("collection incomplete") {
                ItemError {
                    stage: "collect",
                    kind: "hn_fetch".into(),
                    message: match first_failure {
                        Some(detail) => format!("{message}; first failure: {detail}"),
                        None => message,
                    },
                }
            } else {
                ItemError {
                    stage: "process",
                    kind: processing::failure_kind(&message),
                    message,
                }
            };
            ItemOutcome::Failed {
                errors: vec![error],
            }
        }
        workflow::Error::ProcessingIncomplete(report) => {
            let interrupted_only =
                report.not_attempted > 0 && control.interrupted() && !control.breaker.is_tripped();
            let item_errors: Vec<ItemError> = report
                .failed
                .iter()
                .map(|(root, message)| ItemError {
                    stage: "extract",
                    kind: processing::failure_kind(message),
                    message: format!("group {root}: {message}"),
                })
                .collect();
            if item_errors.is_empty() {
                // Groups never sent to the model because the run stopped.
                ItemOutcome::Stopped
            } else if interrupted_only {
                for error in &item_errors {
                    errors.record(error.stage, &error.kind, Some(id), &error.message);
                }
                ItemOutcome::Stopped
            } else {
                ItemOutcome::Failed {
                    errors: item_errors,
                }
            }
        }
    }
}

/// Observational events from a workload while it handles a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressEvent {
    Item(i64),
    Collected,
    Extract(usize, usize),
}

pub struct ChunkCx<'a> {
    pub store: &'a mut Store,
    pub control: &'a Arc<RunControl>,
    /// For errors that are not a whole-item failure (they do not mark the item).
    pub errors: &'a mut ErrorLog,
    pub progress: &'a mut dyn FnMut(ProgressEvent),
}

#[derive(Debug, Clone, Default)]
pub struct SeedReport {
    pub units: usize,
    pub items: usize,
    pub notes: Vec<String>,
}

pub trait Workload {
    type Unit: Copy + Ord + Display;
    /// `driver_runs.command` and the coverage key prefix.
    const COMMAND: &'static str;
    const ITEM_KIND: QueueKind;
    /// For messages: "thread" or "story".
    const ITEM_LABEL: &'static str;
    /// For messages: "month" or "day".
    const UNIT_LABEL: &'static str;

    /// Parses a coverage key; `Display` of a unit is its key.
    fn parse_unit(&self, key: &str) -> Option<Self::Unit>;
    /// The newest `n` units, newest first, never older than the floor.
    fn head_units(&self, now: i64, n: usize) -> Vec<Self::Unit>;
    fn floor(&self) -> Self::Unit;
    /// The next older unit, or `None` below the representable range.
    fn previous(&self, unit: Self::Unit) -> Option<Self::Unit>;
    /// Every item of the unit, newest first. Empty means no discussion.
    fn discover(&self, unit: Self::Unit) -> Result<Vec<i64>, String>;
    /// How many queued items one `process_chunk` call takes at most.
    fn chunk_size(&self) -> usize {
        1
    }
    /// Collects, processes and approves the items. It does not export. It
    /// returns one outcome per item it handled, in any order; unhandled items
    /// stay pending. `unit` is `None` for explicitly named items.
    fn process_chunk(
        &self,
        cx: &mut ChunkCx<'_>,
        unit: Option<Self::Unit>,
        ids: &[i64],
    ) -> Vec<(i64, ItemOutcome)>;
    /// Exports approved data and returns the snapshot version.
    fn export(&self, store: &mut Store) -> Result<String, String>;
    /// Runs once at the start of every run (idempotent).
    fn seed(&self, _store: &mut Store, _now: i64) -> Result<SeedReport, String> {
        Ok(SeedReport::default())
    }
}

#[derive(Debug, Clone)]
pub struct DriverConfig<U> {
    /// Recent units re-walked every run, outside the item cap.
    pub head: usize,
    /// Uncovered units older than the head taken per run.
    pub backfill_units: usize,
    /// Items collected and processed per run by resume and backfill together.
    pub max_items: usize,
    /// A unit or item failing this many attempts needs manual action.
    pub max_attempts: u32,
    /// Overrides: exactly these units, forced, bypassing head, resume,
    /// backfill and the item cap.
    pub range: Option<Vec<U>>,
    /// Override: exactly these items, in one pass. No unit, coverage row or
    /// queue row is created; an existing queue row of the item is updated.
    /// Bypasses everything else, like `range`.
    pub items: Option<Vec<i64>>,
}

#[derive(Debug)]
pub struct DriverResult {
    pub summary: RunSummary,
    pub exit_code: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Resume and backfill: reuse queued items, count against the item cap.
    Capped,
    /// Head and overrides: rediscover, re-walk every item, no cap.
    Forced,
}

struct Driver<'a, W: Workload, O: Write, K: Clock, E: Write> {
    store: &'a mut Store,
    session: &'a RunSession,
    w: &'a W,
    cfg: &'a DriverConfig<W::Unit>,
    now: &'a dyn Fn() -> i64,
    status: &'a mut StatusLine<O, K>,
    err: &'a mut E,
    errors: ErrorLog,
    snap: Snapshot,
    units_done: Vec<String>,
    collected: usize,
    processed: usize,
    approved: usize,
    failed: usize,
    used: usize,
    limit_hit: bool,
    failed_units_now: BTreeSet<String>,
    dirty: bool,
    export_version: Option<String>,
    fatal: bool,
}

/// Runs one invocation: seed, resume, head, backfill, export, report.
pub fn run<W: Workload, O: Write, K: Clock>(
    store: &mut Store,
    session: &RunSession,
    w: &W,
    cfg: &DriverConfig<W::Unit>,
    now: &dyn Fn() -> i64,
    status: &mut StatusLine<O, K>,
    err: &mut impl Write,
) -> DriverResult {
    let started_at = now();
    let mut d = Driver {
        store,
        session,
        w,
        cfg,
        now,
        status,
        err,
        errors: ErrorLog::default(),
        snap: Snapshot {
            command: W::COMMAND.to_owned(),
            ..Snapshot::default()
        },
        units_done: Vec::new(),
        collected: 0,
        processed: 0,
        approved: 0,
        failed: 0,
        used: 0,
        limit_hit: false,
        failed_units_now: BTreeSet::new(),
        dirty: false,
        export_version: None,
        fatal: false,
    };
    let plan = d.plan();
    d.execute(&plan);
    d.finish(started_at, &plan)
}

struct Plan<U> {
    head: Vec<U>,
    resume: Vec<U>,
    backfill: Vec<U>,
    /// More uncovered units exist above the floor than the budget allows.
    backfill_more: Option<U>,
}

impl<W: Workload, O: Write, K: Clock, E: Write> Driver<'_, W, O, K, E> {
    fn control(&self) -> &Arc<RunControl> {
        &self.session.control
    }

    fn should_stop(&self) -> bool {
        self.control().should_stop() || self.fatal
    }

    fn note(&mut self, text: &str) {
        self.status.clear();
        let _ = writeln!(self.err, "{text}");
    }

    fn store_error(&mut self, what: &str, error: impl Display) {
        self.errors
            .record("store", "sqlite", None, &format!("{what}: {error}"));
        self.fatal = true;
    }

    fn refresh_status(&mut self) {
        self.snap.collected = self.collected;
        self.snap.processed = self.processed;
        self.snap.failed = self.failed;
        self.status.update(&self.snap);
    }

    fn plan(&mut self) -> Plan<W::Unit> {
        let now = (self.now)();
        match self.w.seed(self.store, now) {
            Ok(report) => {
                if report.units > 0 {
                    let text = format!(
                        "note: seeded coverage for {} {}(s) from {} stored {}(s)",
                        report.units,
                        W::UNIT_LABEL,
                        report.items,
                        W::ITEM_LABEL
                    );
                    self.note(&text);
                }
                for line in &report.notes {
                    self.note(&format!("note: {line}"));
                }
            }
            Err(error) => {
                self.errors
                    .record("seed", "store", None, &format!("seeding failed: {error}"))
            }
        }
        if self.cfg.items.is_some() {
            return Plan {
                head: Vec::new(),
                resume: Vec::new(),
                backfill: Vec::new(),
                backfill_more: None,
            };
        }
        let range = self.cfg.range.clone();
        if let Some(units) = range {
            return Plan {
                head: units,
                resume: Vec::new(),
                backfill: Vec::new(),
                backfill_more: None,
            };
        }
        match self
            .store
            .reopen_manually_resolved_units(W::COMMAND, self.cfg.max_attempts, now)
        {
            Ok(units) if !units.is_empty() => {
                let text = format!(
                    "note: {} {}(s) exhausted earlier now have a complete collection and processing from manual runs; reopened for approval and export: {}",
                    units.len(),
                    W::UNIT_LABEL,
                    units.join(", ")
                );
                self.note(&text);
            }
            Ok(_) => {}
            Err(error) => {
                self.errors.record(
                    "store",
                    "sqlite",
                    None,
                    &format!("checking manually resolved units: {error}"),
                );
            }
        }
        let head = self.w.head_units(now, self.cfg.head);
        let head_set: BTreeSet<W::Unit> = head.iter().copied().collect();
        let rows = match self.store.units_in_state(
            W::COMMAND,
            &[
                UnitState::Pending,
                UnitState::InProgress,
                UnitState::Failed,
                UnitState::Done,
            ],
            u32::MAX,
        ) {
            Ok(rows) => rows,
            Err(error) => {
                self.store_error("reading coverage", error);
                return Plan {
                    head: Vec::new(),
                    resume: Vec::new(),
                    backfill: Vec::new(),
                    backfill_more: None,
                };
            }
        };
        let mut known = BTreeSet::new();
        let mut resume = Vec::new();
        let mut acknowledged = BTreeSet::new();
        for row in &rows {
            let Some(unit) = self.w.parse_unit(&row.unit_key) else {
                continue;
            };
            known.insert(unit);
            // An acknowledged unit is skipped by resume and the head until the
            // maintainer unacknowledges or retries it, or an override runs it.
            if row.state == UnitState::Failed && row.acknowledged_at.is_some() {
                acknowledged.insert(unit);
                continue;
            }
            let open = match row.state {
                UnitState::Pending | UnitState::InProgress => true,
                UnitState::Failed => row.attempts < self.cfg.max_attempts,
                UnitState::Done => false,
            };
            if open && !head_set.contains(&unit) {
                resume.push(unit);
            }
        }
        resume.sort_by(|a, b| b.cmp(a));
        let head: Vec<W::Unit> = head
            .into_iter()
            .filter(|unit| !acknowledged.contains(unit))
            .collect();
        let mut backfill = Vec::new();
        let mut backfill_more = None;
        let floor = self.w.floor();
        let mut cursor = head.last().and_then(|oldest| self.w.previous(*oldest));
        while let Some(unit) = cursor {
            if unit < floor {
                break;
            }
            if !known.contains(&unit) {
                if backfill.len() < self.cfg.backfill_units {
                    backfill.push(unit);
                } else {
                    backfill_more = Some(unit);
                    break;
                }
            }
            cursor = self.w.previous(unit);
        }
        Plan {
            head,
            resume,
            backfill,
            backfill_more,
        }
    }

    fn execute(&mut self, plan: &Plan<W::Unit>) {
        if let Some(ids) = self.cfg.items.clone() {
            return self.run_direct(&ids);
        }
        // Capped phases stop at the item cap; the head never does.
        for unit in &plan.resume {
            if self.should_stop() || self.limit_hit {
                break;
            }
            self.run_unit(*unit, Mode::Capped);
        }
        for unit in &plan.head {
            if self.should_stop() {
                break;
            }
            self.run_unit(*unit, Mode::Forced);
        }
        for unit in &plan.backfill {
            if self.should_stop() || self.limit_hit {
                break;
            }
            self.run_unit(*unit, Mode::Capped);
        }
    }

    /// Explicit items: one uncapped pass without unit or queue bookkeeping.
    fn run_direct(&mut self, ids: &[i64]) {
        let mut seen = BTreeSet::new();
        let ids: Vec<i64> = ids.iter().copied().filter(|id| seen.insert(*id)).collect();
        let mut index = 0;
        while index < ids.len() {
            if self.should_stop() {
                break;
            }
            let take = self.w.chunk_size().max(1).min(ids.len() - index);
            let chunk = &ids[index..index + take];
            index += take;
            if !self.run_chunk(None, Mode::Forced, chunk) {
                break;
            }
        }
    }

    fn run_unit(&mut self, unit: W::Unit, mode: Mode) {
        if mode == Mode::Capped && self.used >= self.cfg.max_items {
            self.limit_hit = true;
            return;
        }
        let key = unit.to_string();
        let now = (self.now)();
        let run_id = self.session.run_id;
        if let Err(error) = self.store.begin_unit_attempt(W::COMMAND, &key, run_id, now) {
            return self.store_error("starting a unit", error);
        }
        self.snap.unit = Some(key.clone());
        self.snap.item = None;
        self.snap.extract = None;
        self.refresh_status();

        let known = match self.store.queue_counts(W::COMMAND, &key) {
            Ok(counts) => counts.total(),
            Err(error) => return self.store_error("reading the queue", error),
        };
        if mode == Mode::Forced || known == 0 {
            match self.w.discover(unit) {
                Err(error) => {
                    self.errors.record("discovery", "algolia", None, &error);
                    self.failed += 1;
                    self.failed_units_now.insert(key.clone());
                    let update = UnitUpdate {
                        command: W::COMMAND,
                        unit: &key,
                        state: UnitState::Failed,
                        run_id: Some(run_id),
                        error: Some(&error),
                        now: (self.now)(),
                    };
                    if let Err(error) = self.store.finish_unit(&update) {
                        self.store_error("finishing a unit", error);
                    }
                    return;
                }
                Ok(ids) => {
                    let mut ids = ids;
                    let mut seen = BTreeSet::new();
                    ids.retain(|id| seen.insert(*id));
                    if let Err(error) =
                        self.store
                            .enqueue_items(W::COMMAND, W::ITEM_KIND, &key, &ids, (self.now)())
                    {
                        return self.store_error("queueing items", error);
                    }
                }
            }
            if mode == Mode::Forced
                && let Err(error) = self.store.requeue_unit(W::COMMAND, &key, (self.now)())
            {
                return self.store_error("requeueing a unit", error);
            }
        }
        let rows = match self
            .store
            .pending_items(W::COMMAND, &key, self.cfg.max_attempts)
        {
            Ok(rows) => rows,
            Err(error) => return self.store_error("reading the queue", error),
        };
        let ids: Vec<i64> = rows.iter().map(|row| row.item_id).collect();

        let mut cut_short = false;
        let mut index = 0;
        while index < ids.len() {
            if self.should_stop() {
                cut_short = true;
                break;
            }
            let mut take = self.w.chunk_size().max(1).min(ids.len() - index);
            if mode == Mode::Capped {
                let remaining = self.cfg.max_items.saturating_sub(self.used);
                if remaining == 0 {
                    self.limit_hit = true;
                    cut_short = true;
                    break;
                }
                take = take.min(remaining);
            }
            let chunk = &ids[index..index + take];
            index += take;
            if !self.run_chunk(Some(unit), mode, chunk) {
                cut_short = true;
                break;
            }
        }

        let finished = (self.now)();
        if cut_short {
            // Only real failures and kills consume attempts.
            if let Err(error) = self.store.refund_unit_attempt(W::COMMAND, &key) {
                self.store_error("refunding an attempt", error);
            }
            return;
        }
        let counts = match self.store.queue_counts(W::COMMAND, &key) {
            Ok(counts) => counts,
            Err(error) => return self.store_error("reading the queue", error),
        };
        let (state, message) = if counts.pending == 0 && counts.failed == 0 {
            (UnitState::Done, None)
        } else {
            (
                UnitState::Failed,
                Some(format!(
                    "{} {}(s) failed, {} pending",
                    counts.failed,
                    W::ITEM_LABEL,
                    counts.pending
                )),
            )
        };
        let update = UnitUpdate {
            command: W::COMMAND,
            unit: &key,
            state,
            run_id: Some(run_id),
            error: message.as_deref(),
            now: finished,
        };
        match self.store.finish_unit(&update) {
            Ok(()) => {
                if state == UnitState::Done {
                    self.units_done.push(key);
                } else {
                    self.failed_units_now.insert(key);
                }
            }
            Err(error) => self.store_error("finishing a unit", error),
        }
        self.refresh_status();
    }

    /// Processes one chunk and records its outcomes. Returns false when the
    /// unit must not continue (a stopped item).
    fn run_chunk(&mut self, unit: Option<W::Unit>, mode: Mode, chunk: &[i64]) -> bool {
        let control = self.control().clone();
        let outcomes = {
            let (snap, status, collected) =
                (&mut self.snap, &mut *self.status, &mut self.collected);
            let (processed, failed) = (self.processed, self.failed);
            let mut progress = |event: ProgressEvent| {
                match event {
                    ProgressEvent::Item(id) => {
                        snap.item = Some((W::ITEM_LABEL, id));
                        snap.extract = None;
                    }
                    ProgressEvent::Collected => *collected += 1,
                    ProgressEvent::Extract(done, total) => snap.extract = Some((done, total)),
                }
                snap.collected = *collected;
                snap.processed = processed;
                snap.failed = failed;
                status.update(snap);
            };
            let mut cx = ChunkCx {
                store: &mut *self.store,
                control: &control,
                errors: &mut self.errors,
                progress: &mut progress,
            };
            self.w.process_chunk(&mut cx, unit, chunk)
        };
        let mut proceed = true;
        for (id, outcome) in outcomes {
            let run_id = self.session.run_id;
            let now = (self.now)();
            match outcome {
                ItemOutcome::Done { approved } => {
                    self.processed += 1;
                    self.approved += approved;
                    self.used += usize::from(mode == Mode::Capped);
                    let update = ItemUpdate {
                        command: W::COMMAND,
                        kind: W::ITEM_KIND,
                        item_id: id,
                        state: ItemState::Done,
                        run_id: Some(run_id),
                        error: None,
                        now,
                    };
                    self.record_item(&update, unit.is_none());
                }
                ItemOutcome::Failed { errors } => {
                    self.failed += 1;
                    self.used += usize::from(mode == Mode::Capped);
                    for error in &errors {
                        self.errors
                            .record(error.stage, &error.kind, Some(id), &error.message);
                    }
                    let message = errors.first().map(|e| e.message.clone());
                    let update = ItemUpdate {
                        command: W::COMMAND,
                        kind: W::ITEM_KIND,
                        item_id: id,
                        state: ItemState::Failed,
                        run_id: Some(run_id),
                        error: message.as_deref(),
                        now,
                    };
                    self.record_item(&update, unit.is_none());
                }
                ItemOutcome::Stopped => proceed = false,
            }
        }
        self.dirty = true;
        self.export();
        self.snap.item = None;
        self.snap.extract = None;
        self.refresh_status();
        proceed && !self.fatal
    }

    /// Explicit items need no queue row; only a missing row is tolerated then.
    fn record_item(&mut self, update: &ItemUpdate<'_>, direct: bool) {
        match self.store.finish_item(update) {
            Err(rusqlite::Error::QueryReturnedNoRows) if direct => {}
            Err(error) => self.store_error("finishing an item", error),
            Ok(()) => {}
        }
    }

    fn export(&mut self) {
        match self.w.export(self.store) {
            Ok(version) => {
                self.export_version = Some(version);
                self.dirty = false;
            }
            Err(error) => {
                self.errors.record("export", "publication", None, &error);
            }
        }
    }

    fn finish(mut self, started_at: i64, plan: &Plan<W::Unit>) -> DriverResult {
        // Approved data of an earlier killed run may be unexported too.
        if self.dirty || (!self.session.killed_runs.is_empty() && self.export_version.is_none()) {
            self.export();
        }
        self.status.clear();
        let control = self.control().clone();
        let interrupted = control.interrupted();
        let tripped = control.breaker.is_tripped();
        let outcome = if interrupted {
            Outcome::Interrupted
        } else if tripped {
            Outcome::StoppedOnFailure
        } else if self.limit_hit || plan.backfill_more.is_some() {
            Outcome::LimitReached
        } else {
            Outcome::Completed
        };

        let head_keys: BTreeSet<String> = plan.head.iter().map(ToString::to_string).collect();
        let mut pending_units = Vec::new();
        let mut failed_units = Vec::new();
        let mut exhausted_units = Vec::new();
        let mut acknowledged_units = Vec::new();
        let mut unit_failures = std::collections::BTreeMap::new();
        match self.store.units_in_state(
            W::COMMAND,
            &[UnitState::Pending, UnitState::InProgress, UnitState::Failed],
            u32::MAX,
        ) {
            Ok(rows) => {
                for row in rows {
                    match row.state {
                        UnitState::Failed if row.acknowledged_at.is_some() => {
                            acknowledged_units.push(row.unit_key);
                        }
                        UnitState::Failed => {
                            let mut failures: Vec<FailedItem> = self
                                .store
                                .failed_items(W::COMMAND, &row.unit_key)
                                .unwrap_or_default()
                                .into_iter()
                                .map(|(id, error)| FailedItem {
                                    id: Some(id),
                                    error: error.unwrap_or_default(),
                                })
                                .collect();
                            if failures.is_empty()
                                && let Some(error) = &row.last_error
                            {
                                failures.push(FailedItem {
                                    id: None,
                                    error: error.clone(),
                                });
                            }
                            unit_failures.insert(row.unit_key.clone(), failures);
                            if row.attempts >= self.cfg.max_attempts
                                && !head_keys.contains(&row.unit_key)
                            {
                                exhausted_units.push(row.unit_key);
                            } else {
                                failed_units.push((row.unit_key, row.attempts));
                            }
                        }
                        _ => {
                            let left = self
                                .store
                                .queue_counts(W::COMMAND, &row.unit_key)
                                .map(|c| c.pending)
                                .unwrap_or(0);
                            pending_units.push(if left > 0 {
                                format!("{} ({left} {}(s) left)", row.unit_key, W::ITEM_LABEL)
                            } else {
                                row.unit_key
                            });
                        }
                    }
                }
            }
            Err(error) => self.store_error("reading coverage", error),
        }

        let next_run = self.next_run(outcome, plan, &pending_units, &control);
        let finished_at = (self.now)();
        let summary = RunSummary {
            run_id: self.session.run_id,
            command: W::COMMAND.to_owned(),
            outcome,
            started_at,
            finished_at,
            errors: self.errors.groups_sorted().into_iter().cloned().collect(),
            units_done: std::mem::take(&mut self.units_done),
            items_collected: self.collected,
            items_processed: self.processed,
            items_approved: self.approved,
            export: self.export_version.clone(),
            pending_units,
            failed_units,
            exhausted_units,
            unit_failures,
            item_label: W::ITEM_LABEL.to_owned(),
            acknowledged_units,
            failed_this_run: self.failed_units_now.iter().cloned().collect(),
            limit: if outcome == Outcome::LimitReached {
                LimitInfo {
                    cap: self
                        .limit_hit
                        .then(|| (self.used, self.cfg.max_items, W::ITEM_LABEL.to_owned())),
                    backfill_next: plan.backfill_more.map(|unit| unit.to_string()),
                }
            } else {
                LimitInfo::default()
            },
            killed_runs: self.session.killed_runs.clone(),
            took_over_pid: self.session.took_over_pid,
            tripped_api: control.breaker.tripped_api(),
            next_run,
        };
        if let Err(error) = finish(
            self.store,
            self.session,
            outcome,
            &summary,
            &self.errors,
            finished_at,
        ) {
            let _ = writeln!(self.err, "warning: could not record the run: {error}");
        }
        // Presentation failures never change the exit code.
        let _ = write_report(self.err, &summary);
        let _ = self.err.flush();
        let exit_code = summary.exit_code();
        DriverResult { summary, exit_code }
    }

    fn next_run(
        &self,
        outcome: Outcome,
        plan: &Plan<W::Unit>,
        pending: &[String],
        control: &RunControl,
    ) -> String {
        let unit = W::UNIT_LABEL;
        let mut steps = Vec::new();
        match outcome {
            Outcome::Interrupted => steps.push("continue the interrupted unit".to_owned()),
            Outcome::StoppedOnFailure => {
                let api = control
                    .breaker
                    .tripped_api()
                    .map_or("the failing API", |api| api.label());
                steps.push(format!("resume once {api} recovers"));
            }
            _ => {}
        }
        if !pending.is_empty() && outcome != Outcome::Interrupted {
            steps.push(format!("finish {} pending {unit}(s) first", pending.len()));
        }
        if self.cfg.items.is_some() {
            steps.push(format!(
                "nothing else: an explicit {} was processed",
                W::ITEM_LABEL
            ));
        } else if self.cfg.range.is_some() {
            steps.push("nothing else: an explicit range was processed".to_owned());
        } else {
            if let (Some(first), Some(last)) = (plan.head.first(), plan.head.last()) {
                steps.push(if first == last {
                    format!("refresh the head ({first}) with full walks")
                } else {
                    format!("refresh the head ({last} to {first}) with full walks")
                });
            }
            let floor = self.w.floor();
            let next = plan
                .backfill_more
                .or_else(|| plan.backfill.last().and_then(|last| self.w.previous(*last)))
                .filter(|unit| *unit >= floor);
            let pending_backfill: Vec<&W::Unit> = plan
                .backfill
                .iter()
                .filter(|u| pending.iter().any(|p| p.starts_with(&u.to_string())))
                .collect();
            if let Some(first) = pending_backfill.first() {
                steps.push(format!(
                    "continue backfill at {first} (up to {} {unit}(s))",
                    self.cfg.backfill_units
                ));
            } else if let Some(next) = next {
                steps.push(format!(
                    "backfill up to {} more {unit}(s) starting at {next}",
                    self.cfg.backfill_units
                ));
            } else {
                steps.push(format!("backfill is complete to {floor}"));
            }
        }
        steps.join("; ")
    }
}

/// Convenience for tests and callers: summarizes units by key.
pub fn unit_states(
    store: &Store,
    command: &str,
) -> rusqlite::Result<BTreeMap<String, (UnitState, u32)>> {
    Ok(store
        .units_in_state(
            command,
            &[
                UnitState::Pending,
                UnitState::InProgress,
                UnitState::Failed,
                UnitState::Done,
            ],
            u32::MAX,
        )?
        .into_iter()
        .map(|unit| (unit.unit_key, (unit.state, unit.attempts)))
        .collect())
}
