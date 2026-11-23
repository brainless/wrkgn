//! Shared sequential collection, processing, approval, and export services.
//!
//! Discovery, argument parsing, and terminal presentation belong to callers.
use crate::collector::{CollectionReport, Collector, HnApi};
use crate::processing::{self, Model};
use crate::{publication, store::Store};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalPolicy {
    Manual,
    ApproveThread,
}

pub struct ThreadOptions<'a> {
    pub processing: &'a processing::Options,
    pub reprocess: bool,
    pub approval: ApprovalPolicy,
    pub destination: &'a Path,
}

pub enum Event<'a> {
    Collected(&'a CollectionReport),
    Progress(processing::Progress),
    Processed(&'a processing::Report),
    Exported {
        approved: Option<usize>,
        version: &'a str,
    },
    Skipped {
        version: &'a str,
    },
}

#[derive(Debug)]
pub enum Error {
    Operation(String),
    ProcessingIncomplete(processing::Report),
    /// The run was interrupted or the circuit breaker tripped mid-collection.
    /// The thread's collection is incomplete and nothing was processed.
    Stopped,
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Operation(value)
    }
}

/// Complete one thread before beginning another. Approval is deliberately
/// separate from export: an export failure can be retried after approval commits.
pub fn sync_thread<C: HnApi, M: Model + Sync>(
    store: &mut Store,
    collector: &Collector<C>,
    model: &M,
    thread: i64,
    options: &ThreadOptions<'_>,
    event: impl FnMut(Event<'_>) -> Result<(), String>,
) -> Result<(), Error> {
    sync_thread_checked(
        store,
        collector,
        model,
        thread,
        options,
        |_, _| Ok(true),
        event,
    )
}

/// Validate the refreshed source and select its extraction mode before any
/// model calls. Returning false withholds extraction and exports fresh state.
pub fn sync_thread_checked<C: HnApi, M: Model + Sync>(
    store: &mut Store,
    collector: &Collector<C>,
    model: &M,
    thread: i64,
    options: &ThreadOptions<'_>,
    prepare: impl FnOnce(&mut Store, i64) -> Result<bool, String>,
    mut event: impl FnMut(Event<'_>) -> Result<(), String>,
) -> Result<(), Error> {
    let staged = stage_thread(
        store,
        collector,
        model,
        thread,
        &StageOptions {
            processing: options.processing,
            reprocess: options.reprocess,
            approval: options.approval,
        },
        prepare,
        &mut event,
    );
    match staged {
        Ok(Staged::Withheld) => {
            let version = publication::export(store, options.destination)?;
            event(Event::Skipped { version: &version })?;
        }
        Ok(Staged::Ready { approved }) => {
            let version = publication::export(store, options.destination)?;
            event(Event::Exported {
                approved,
                version: &version,
            })?;
        }
        Err(Error::ProcessingIncomplete(report)) => {
            // Even failed reruns must remove stale source results from publication.
            publication::export(store, options.destination)?;
            return Err(Error::ProcessingIncomplete(report));
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

pub struct StageOptions<'a> {
    pub processing: &'a processing::Options,
    pub reprocess: bool,
    pub approval: ApprovalPolicy,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Staged {
    /// The refreshed source no longer qualifies; nothing was processed.
    Withheld,
    /// Processing completed and approval (if authorized) committed.
    Ready { approved: Option<usize> },
}

/// Collect, prepare, process and approve one thread without exporting, so a
/// caller can export once for several threads. Callers must export afterwards
/// (including after errors) so stale results of failed threads are withheld.
pub fn stage_thread<C: HnApi, M: Model + Sync>(
    store: &mut Store,
    collector: &Collector<C>,
    model: &M,
    thread: i64,
    options: &StageOptions<'_>,
    prepare: impl FnOnce(&mut Store, i64) -> Result<bool, String>,
    mut event: impl FnMut(Event<'_>) -> Result<(), String>,
) -> Result<Staged, Error> {
    if !collect_prepare(store, collector, thread, prepare, &mut event)? {
        return Ok(Staged::Withheld);
    }
    // Progress reporting is observational; presentation errors are handled by
    // the caller after the processing invocation, before approval.
    let mut progress_error = None;
    let result = processing::process_thread_with(
        store,
        thread,
        model,
        options.reprocess,
        options.processing,
        |p| {
            if progress_error.is_none() {
                progress_error = event(Event::Progress(p)).err();
            }
        },
    );
    let report = result?;
    event(Event::Processed(&report))?;
    conclude(store, thread, report, options.approval, progress_error)
}

/// Refresh one thread from HN and let `prepare` decide whether it may be
/// processed. Returns false when the refreshed source no longer qualifies.
pub fn collect_prepare<C: HnApi>(
    store: &mut Store,
    collector: &Collector<C>,
    thread: i64,
    prepare: impl FnOnce(&mut Store, i64) -> Result<bool, String>,
    mut event: impl FnMut(Event<'_>) -> Result<(), String>,
) -> Result<bool, Error> {
    let collection = collector
        .collect(store, thread)
        .map_err(|e| e.to_string())?;
    event(Event::Collected(&collection))?;
    // A stop that came with real fetch failures is that item's own failure
    // (for example the failures that tripped the breaker); a bare stop is not.
    if collection.stopped && collection.failures.is_empty() {
        return Err(Error::Stopped);
    }
    if !collection.complete {
        return Err(format!(
            "collection incomplete for thread {thread}: {} failed items; rerun to resume",
            collection.failures.len()
        )
        .into());
    }
    Ok(prepare(store, thread)?)
}

/// Turn a processing report into success, or an incomplete-processing error,
/// then approve the thread when authorized. `deferred` is a presentation error
/// raised during processing, reported only after processing itself succeeded.
pub fn conclude(
    store: &mut Store,
    thread: i64,
    report: processing::Report,
    approval: ApprovalPolicy,
    deferred: Option<String>,
) -> Result<Staged, Error> {
    if !report.failed.is_empty() || report.not_attempted > 0 {
        return Err(Error::ProcessingIncomplete(report));
    }
    if let Some(error) = deferred {
        return Err(error.into());
    }
    let approved = match approval {
        ApprovalPolicy::Manual => None,
        ApprovalPolicy::ApproveThread => Some(publication::approve_thread(store, thread)?),
    };
    Ok(Staged::Ready { approved })
}
