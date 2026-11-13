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
    let collection = collector
        .collect(store, thread)
        .map_err(|e| e.to_string())?;
    event(Event::Collected(&collection))?;
    if !collection.complete {
        return Err(format!(
            "collection incomplete for thread {thread}: {} failed items; rerun to resume",
            collection.failures.len()
        )
        .into());
    }
    if !prepare(store, thread)? {
        let version = publication::export(store, options.destination)?;
        event(Event::Skipped { version: &version })?;
        return Ok(());
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
    if !report.failed.is_empty() || report.not_attempted > 0 {
        // Even failed reruns must remove stale source results from publication.
        publication::export(store, options.destination)?;
        return Err(Error::ProcessingIncomplete(report));
    }
    if let Some(error) = progress_error {
        return Err(error.into());
    }
    let approved = match options.approval {
        ApprovalPolicy::Manual => None,
        ApprovalPolicy::ApproveThread => Some(publication::approve_thread(store, thread)?),
    };
    let version = publication::export(store, options.destination)?;
    event(Event::Exported {
        approved,
        version: &version,
    })?;
    Ok(())
}
