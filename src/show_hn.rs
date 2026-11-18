//! Show HN CLI selection and presentation; orchestration lives in workflow.
use crate::cli::{Status, failure_summary};
use crate::collector::{CollectionConfig, Collector, HnApi};
use crate::processing::{self, Model, RigModel};
use crate::publication;
use crate::show_extraction;
use crate::show_hn_discovery::{
    Batches, Candidate, DEFAULT_BATCH_SIZE, DateRange, Discovery, MAX_BATCH_SIZE, matches_title,
    utc,
};
use crate::store::{ExtractionMode, Store};
use crate::workflow::{self, ApprovalPolicy, Event, ThreadOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;

const HELP: &str = "Usage: wrkgn-show-hn --thread ID | --from YYYY-MM-DD --through YYYY-MM-DD
    [--batch-size N] [--auto-approve] [--reprocess] [--concurrency N] [--db PATH]
    [--export DIRECTORY]

Explicitly collect and process Show HN stories, newest first. A range is
discovered and handled in batches of --batch-size stories (default 100, at most
1000); each batch is exported once. A failed story is reported and the run
continues, then exits nonzero with a summary. Calendar ranges are inclusive UTC
dates. Results await manual review unless --auto-approve is supplied. Rerun to refresh and resume unchanged successful work.
Discovery depends on Algolia indexing; later reruns may discover more stories.
Uses the same .env/provider settings as wrkgn. Website staging remains explicit.";

enum Selection {
    Thread(i64),
    Range(DateRange),
}
struct Args {
    selection: Selection,
    db: Option<PathBuf>,
    export: PathBuf,
    options: processing::Options,
    reprocess: bool,
    approval: ApprovalPolicy,
    batch_size: usize,
}
impl Args {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut thread = None;
        let mut from = None;
        let mut through = None;
        let mut db = None;
        let mut export = std::env::var_os("WRKGN_EXPORT_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("local-data/public-export"));
        let mut options = processing::Options::default();
        let mut reprocess = false;
        let mut approval = ApprovalPolicy::Manual;
        let mut batch_size = None;
        let mut seen = BTreeSet::new();
        let mut args = args.iter();
        while let Some(flag) = args.next() {
            if !seen.insert(flag) {
                return Err(format!("duplicate argument {flag}"));
            }
            match flag.as_str() {
                "--auto-approve" => {
                    approval = ApprovalPolicy::ApproveThread;
                    continue;
                }
                "--reprocess" => {
                    reprocess = true;
                    continue;
                }
                "--thread" | "--from" | "--through" | "--db" | "--export" | "--concurrency"
                | "--batch-size" => {}
                _ => return Err(format!("unknown argument {flag}\n{HELP}")),
            }
            let value = args
                .next()
                .filter(|v| !v.starts_with("--"))
                .ok_or_else(|| format!("{flag} requires a value"))?;
            match flag.as_str() {
                "--thread" => {
                    thread = Some(
                        value
                            .parse::<i64>()
                            .ok()
                            .filter(|id| *id > 0)
                            .ok_or("--thread requires a positive item ID")?,
                    )
                }
                "--from" => from = Some(value.clone()),
                "--through" => through = Some(value.clone()),
                "--db" => db = Some(PathBuf::from(value)),
                "--export" => export = PathBuf::from(value),
                "--concurrency" => {
                    options.concurrency = value
                        .parse::<usize>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or("--concurrency requires a positive integer")?
                }
                "--batch-size" => {
                    batch_size = Some(
                        value
                            .parse::<usize>()
                            .ok()
                            .filter(|n| (1..=MAX_BATCH_SIZE).contains(n))
                            .ok_or_else(|| {
                                format!(
                                    "--batch-size requires an integer from 1 to {MAX_BATCH_SIZE}"
                                )
                            })?,
                    )
                }
                _ => unreachable!(),
            }
        }
        if thread.is_some() && batch_size.is_some() {
            return Err("--batch-size applies only to --from/--through ranges".into());
        }
        let selection = match (thread, from, through) {
            (Some(id), None, None) => Selection::Thread(id),
            (None, Some(from), Some(through)) => {
                Selection::Range(DateRange::parse(&from, &through)?)
            }
            _ => {
                return Err(format!(
                    "supply either --thread or both --from and --through\n{HELP}"
                ));
            }
        };
        Ok(Self {
            selection,
            db,
            export,
            options,
            reprocess,
            approval,
            batch_size: batch_size.unwrap_or(DEFAULT_BATCH_SIZE),
        })
    }
}

pub fn run<C: HnApi, D: Discovery>(
    args: &[String],
    api: C,
    discovery: D,
    out: &mut impl Write,
) -> Result<(), String> {
    if args.iter().any(|a| a == "--help") {
        return writeln!(out, "{HELP}").map_err(|e| e.to_string());
    }
    let args = Args::parse(args)?;
    let model = RigModel::from_env()?;
    let mut store = match &args.db {
        Some(path) => Store::open(path),
        None => Store::open_configured(),
    }
    .map_err(|e| e.to_string())?;
    execute(
        &mut store,
        &Collector::new(api, CollectionConfig::default()),
        &model,
        &discovery,
        &args,
        out,
    )
}
fn execute<C: HnApi, M: Model + Sync, D: Discovery>(
    store: &mut Store,
    collector: &Collector<C>,
    model: &M,
    discovery: &D,
    args: &Args,
    out: &mut impl Write,
) -> Result<(), String> {
    match args.selection {
        Selection::Thread(id) => execute_thread(store, collector, model, id, args, out),
        Selection::Range(range) => {
            execute_range(store, collector, model, discovery, range, args, out)
        }
    }
}

/// Decides after collection whether the story may be extracted and assigns its mode.
fn qualify(
    store: &mut Store,
    thread: i64,
    range: Option<DateRange>,
    direct: bool,
) -> Result<bool, String> {
    let item = store
        .get_item(thread)
        .map_err(|e| e.to_string())?
        .ok_or("collected story is missing")?;
    let source = &item.source;
    if source.deleted || source.dead {
        return Ok(false);
    }
    let qualifies = source.raw_payload.get("type").and_then(|v| v.as_str()) == Some("story")
        && source
            .raw_payload
            .get("title")
            .and_then(|v| v.as_str())
            .is_some_and(matches_title)
        && source
            .author
            .as_deref()
            .is_some_and(|a| !a.trim().is_empty())
        && source
            .created_at
            .is_some_and(|date| range.is_none_or(|r| r.contains(date)));
    if !qualifies {
        if direct {
            return Err(format!(
                "item {thread} must be a live Show HN: story with usable author and date"
            ));
        }
        return Ok(false);
    }
    store.assign_extraction_mode(thread, ExtractionMode::ShowHn)?;
    Ok(true)
}

fn monthly_conflict(thread: i64) -> String {
    format!("thread {thread} is assigned to monthly extraction; Show HN workflow cannot process it")
}

fn print_event(
    out: &mut impl Write,
    status: &mut Status,
    thread: i64,
    event: Event<'_>,
) -> Result<(), String> {
    match event {
        Event::Collected(c)=>writeln!(out,"Story {thread}: collected {} items, {} changed.",c.items_fetched,c.changed_item_ids.len()),
        Event::Progress(p)=>{status.update(p);Ok(())},
        Event::Processed(r)=>{status.finish();writeln!(out,"Story {thread}: {} processed, {} unchanged, {} failed, {} not attempted.",r.processed,r.skipped,r.failed.len(),r.not_attempted)},
        Event::Exported{approved,version}=>match approved {
            Some(count)=>writeln!(out,"Story {thread}: auto-approved {count} mentions. Public snapshot: {version}"),
            None=>writeln!(out,"Story {thread}: results await review. Public snapshot: {version}"),
        },
        Event::Skipped{version}=>writeln!(out,"Story {thread}: collected source no longer qualifies; skipped. Public snapshot: {version}"),
    }.map_err(|e|e.to_string())
}

fn execute_thread<C: HnApi, M: Model + Sync>(
    store: &mut Store,
    collector: &Collector<C>,
    model: &M,
    thread: i64,
    args: &Args,
    out: &mut impl Write,
) -> Result<(), String> {
    if store.extraction_mode(thread)? == Some(ExtractionMode::Monthly) {
        return Err(monthly_conflict(thread));
    }
    let mut status = Status::new();
    let result = workflow::sync_thread_checked(
        store,
        collector,
        model,
        thread,
        &ThreadOptions {
            processing: &args.options,
            reprocess: args.reprocess,
            approval: args.approval,
            destination: &args.export,
        },
        |store, thread| qualify(store, thread, None, true),
        |event| print_event(out, &mut status, thread, event),
    );
    if let Err(error) = result {
        status.finish();
        let message = match error {
            workflow::Error::Operation(e) => e,
            workflow::Error::ProcessingIncomplete(r) => failure_summary(&r),
        };
        return Err(format!(
            "story {thread}: {message}\nRerun the same command to resume; completed unchanged work is skipped."
        ));
    }
    out.flush().map_err(|e| e.to_string())
}

/// A per-story failure; `kind` groups the final summary.
struct StoryFailure {
    story: i64,
    kind: String,
}

fn short(reason: &str) -> String {
    let line = reason.lines().next().unwrap_or_default();
    match line.char_indices().nth(200) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_string(),
    }
}

fn describe(error: workflow::Error) -> String {
    match error {
        workflow::Error::Operation(e) => e,
        workflow::Error::ProcessingIncomplete(report) => report
            .failed
            .first()
            .map(|(_, e)| e.clone())
            .unwrap_or_else(|| {
                format!(
                    "processing incomplete: {} group(s) not attempted",
                    report.not_attempted
                )
            }),
    }
}

/// Logs a per-story failure and records it for the final summary.
fn fail_story(
    out: &mut impl Write,
    failures: &mut Vec<StoryFailure>,
    story: i64,
    error: workflow::Error,
) -> Result<(), String> {
    let reason = describe(error);
    writeln!(out, "Story {story}: failed: {}", short(&reason)).map_err(|e| e.to_string())?;
    failures.push(StoryFailure {
        story,
        kind: processing::failure_kind(&reason),
    });
    Ok(())
}

fn execute_range<C: HnApi, M: Model + Sync, D: Discovery>(
    store: &mut Store,
    collector: &Collector<C>,
    model: &M,
    discovery: &D,
    range: DateRange,
    args: &Args,
    out: &mut impl Write,
) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    let mut batches = Batches::new(discovery, range, args.batch_size)?;
    let (mut number, mut succeeded, mut skipped) = (0, 0, 0);
    let mut failures: Vec<StoryFailure> = Vec::new();
    let mut discovery_error = None;
    let mut export_error: Option<String> = None;
    while !batches.finished() {
        writeln!(
            out,
            "Batch {}: searching stories on or before {}…",
            number + 1,
            utc(batches.cursor())
        )
        .map_err(io)?;
        out.flush().map_err(io)?;
        let stories = match batches.next_batch() {
            Ok(Some(stories)) => stories,
            Ok(None) => {
                writeln!(out, "No more stories in the range.").map_err(io)?;
                break;
            }
            // Older batches cannot be located without this search.
            Err(error) => {
                writeln!(out, "Batch {}: discovery failed: {error}", number + 1).map_err(io)?;
                discovery_error = Some(error);
                break;
            }
        };
        number += 1;
        match (stories.first(), stories.last()) {
            (Some(newest), Some(oldest)) => writeln!(
                out,
                "Batch {number}: {} stories ({} → {}).",
                stories.len(),
                &utc(newest.timestamp)[..10],
                &utc(oldest.timestamp)[..10]
            ),
            _ => writeln!(out, "Batch {number}: 0 stories."),
        }
        .map_err(io)?;
        let mut approved = 0;
        // Collect sequentially, then extract the batch's stories concurrently
        // (SQLite stays on this thread), then approve each success in order.
        let mut ready = Vec::new();
        for Candidate { id: thread, .. } in &stories {
            let thread = *thread;
            let mut output_error = None;
            let result = match store.extraction_mode(thread) {
                Ok(Some(ExtractionMode::Monthly)) => {
                    Err(workflow::Error::Operation(monthly_conflict(thread)))
                }
                Err(error) => Err(workflow::Error::Operation(error.to_string())),
                Ok(_) => workflow::collect_prepare(
                    store,
                    collector,
                    thread,
                    |store, thread| qualify(store, thread, Some(range), false),
                    |event| {
                        print_event(out, &mut Status::new(), thread, event).inspect_err(|e| {
                            output_error.get_or_insert(e.clone());
                        })
                    },
                ),
            };
            if let Some(error) = output_error {
                return Err(error);
            }
            match result {
                Ok(true) => ready.push(thread),
                Ok(false) => {
                    skipped += 1;
                    writeln!(
                        out,
                        "Story {thread}: collected source no longer qualifies; skipped."
                    )
                    .map_err(io)?;
                }
                Err(error) => fail_story(out, &mut failures, thread, error)?,
            }
            out.flush().map_err(io)?;
        }
        let mut status = Status::new();
        let results = show_extraction::process_batch(
            store,
            &ready,
            model,
            args.reprocess,
            &args.options,
            |progress| status.update(progress),
        );
        status.finish();
        for (thread, result) in ready.into_iter().zip(results) {
            let staged = match result {
                Ok(report) => {
                    print_event(out, &mut status, thread, Event::Processed(&report))?;
                    workflow::conclude(store, thread, report, args.approval, None)
                }
                Err(error) => Err(workflow::Error::Operation(error)),
            };
            match staged {
                Ok(workflow::Staged::Ready { approved: count }) => {
                    succeeded += 1;
                    approved += count.unwrap_or(0);
                }
                Ok(workflow::Staged::Withheld) => unreachable!("withheld stories are not ready"),
                Err(error) => fail_story(out, &mut failures, thread, error)?,
            }
            out.flush().map_err(io)?;
        }
        // One export per batch; failed stories are withheld by freshness
        // checks, and a failed export is retried after the next batch.
        if !stories.is_empty() || export_error.is_some() {
            match publication::export(store, &args.export) {
                Ok(version) => {
                    export_error = None;
                    match args.approval {
                        ApprovalPolicy::ApproveThread => writeln!(
                            out,
                            "Batch {number}: approved {approved} mentions. Public snapshot: {version}"
                        ),
                        ApprovalPolicy::Manual => writeln!(
                            out,
                            "Batch {number}: results await review. Public snapshot: {version}"
                        ),
                    }
                    .map_err(io)?;
                }
                Err(error) => {
                    writeln!(
                        out,
                        "Batch {number}: export failed: {}; retrying after the next batch.",
                        short(&error)
                    )
                    .map_err(io)?;
                    export_error = Some(error);
                }
            }
        }
        out.flush().map_err(io)?;
    }
    let mut kinds: BTreeMap<&str, (usize, i64)> = BTreeMap::new();
    for failure in &failures {
        kinds.entry(&failure.kind).or_insert((0, failure.story)).0 += 1;
    }
    let mut kinds: Vec<_> = kinds.into_iter().collect();
    kinds.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));
    writeln!(
        out,
        "Show HN summary: {number} batch(es), {succeeded} story(ies) succeeded, {} failed, {skipped} skipped.",
        failures.len()
    )
    .map_err(io)?;
    for (kind, (count, story)) in kinds {
        writeln!(out, "  {count} x {kind} (e.g. story {story})").map_err(io)?;
    }
    out.flush().map_err(io)?;
    if failures.is_empty() && discovery_error.is_none() && export_error.is_none() {
        return Ok(());
    }
    let mut message = format!(
        "Show HN run incomplete: {} story failure(s)",
        failures.len()
    );
    if let Some(error) = discovery_error {
        message.push_str(&format!(
            "; discovery stopped before reaching older stories: {error}"
        ));
    }
    if let Some(error) = export_error {
        message.push_str(&format!("; final export failed: {}", short(&error)));
    }
    message.push_str("\nRerun the same command to retry; completed unchanged work is skipped.");
    Err(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_is_explicit_and_validated_before_network_or_model() {
        let args = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        for invalid in [
            "",
            "--thread 0",
            "--thread 1 --from 2026-11-01 --through 2026-11-02",
            "--from 2026-11-01",
            "--through 2026-11-01",
            "--from 2026-11-02 --through 2026-11-01",
            "--thread 1 --concurrency 0",
            "--thread 1 --batch-size 10",
            "--from 2026-11-01 --through 2026-11-02 --batch-size 0",
            "--from 2026-11-01 --through 2026-11-02 --batch-size 1001",
            "--thread 1 --auto-approve --auto-approve",
        ] {
            assert!(Args::parse(&args(invalid)).is_err(), "{invalid}");
        }
        let manual = Args::parse(&args("--thread 1")).unwrap();
        assert_eq!(manual.approval, ApprovalPolicy::Manual);
        assert!(!manual.reprocess);
        let automated = Args::parse(&args(
            "--from 2026-11-01 --through 2026-11-02 --auto-approve --reprocess",
        ))
        .unwrap();
        assert_eq!(automated.approval, ApprovalPolicy::ApproveThread);
        assert!(automated.reprocess);
    }
}

#[cfg(test)]
mod workflow_tests {
    use super::*;
    use crate::collector::FetchError;
    use crate::extraction::{Extraction, Mention};
    use crate::show_extraction::{ShowEvidence, ShowExtraction, ShowProject};
    use crate::show_hn_discovery::SearchHit;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Api {
        title: &'static str,
    }
    impl HnApi for Api {
        fn fetch_item(&self, id: i64) -> Result<Option<serde_json::Value>, FetchError> {
            Ok(Some(
                serde_json::json!({"id":id,"type":"story","by":"alice","time":1000,"title":self.title,"url":"https://alpha.test","kids":[]}),
            ))
        }
    }
    struct Fixture(AtomicUsize);
    impl Model for Fixture {
        fn name(&self) -> &str {
            "show-cli-fixture"
        }
        fn extract(&self, _: &str) -> Result<Extraction, String> {
            panic!("monthly extraction must not run")
        }
        fn extract_show(&self, prompt: &str) -> Result<ShowExtraction, String> {
            let story = serde_json::from_str::<serde_json::Value>(prompt).unwrap()["story_id"]
                .as_i64()
                .unwrap();
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ShowExtraction {
                project: Some(ShowProject {
                    mention: Mention {
                        source_item_id: story,
                        name: Some("Alpha".into()),
                        description: "Alpha organizes work".into(),
                        urls: vec![],
                        problem: None,
                        audience: None,
                        source_excerpt: "Show HN: Alpha organizes work".into(),
                        builders: vec![],
                        categories: vec![],
                        audience_labels: vec![],
                    },
                    evidence: vec![ShowEvidence {
                        source_item_id: story,
                        source_field: "title".into(),
                        value: "Show HN: Alpha organizes work".into(),
                        supports: vec!["name".into(), "description".into()],
                        authorization_source_item_id: None,
                    }],
                }),
                no_project_reason: None,
            })
        }
    }
    struct NoDiscovery;
    impl Discovery for NoDiscovery {
        fn search(&self, _: i64, _: i64, _: usize) -> Result<Vec<SearchHit>, String> {
            panic!("direct selection must not discover")
        }
    }
    fn args(dir: &std::path::Path, approval: ApprovalPolicy) -> Args {
        Args {
            selection: Selection::Thread(100),
            db: None,
            export: dir.join("export"),
            options: processing::Options::default(),
            reprocess: false,
            approval,
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }
    #[test]
    fn manual_default_and_explicit_approval_resume_unchanged_story() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("db")).unwrap();
        let model = Fixture(AtomicUsize::new(0));
        let collector = Collector::new(
            Api {
                title: "Show HN: Alpha organizes work",
            },
            CollectionConfig::default(),
        );
        execute(
            &mut store,
            &collector,
            &model,
            &NoDiscovery,
            &args(dir.path(), ApprovalPolicy::Manual),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            crate::publication::pending_mentions(&mut store)
                .unwrap()
                .len(),
            1
        );
        execute(
            &mut store,
            &collector,
            &model,
            &NoDiscovery,
            &args(dir.path(), ApprovalPolicy::ApproveThread),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            crate::publication::pending_mentions(&mut store)
                .unwrap()
                .is_empty()
        );
        assert_eq!(model.0.load(Ordering::SeqCst), 1);
        let mut other = args(dir.path(), ApprovalPolicy::Manual);
        other.selection = Selection::Thread(200);
        execute(
            &mut store,
            &collector,
            &model,
            &NoDiscovery,
            &other,
            &mut Vec::new(),
        )
        .unwrap();
        execute(
            &mut store,
            &collector,
            &model,
            &NoDiscovery,
            &args(dir.path(), ApprovalPolicy::ApproveThread),
            &mut Vec::new(),
        )
        .unwrap();
        let pending = crate::publication::pending_mentions(&mut store).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].source_item_id, 200);
        assert_eq!(
            store.extraction_mode(100).unwrap(),
            Some(ExtractionMode::ShowHn)
        );
    }
    #[test]
    fn non_show_story_and_conflicting_mode_fail_before_model_calls() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("db")).unwrap();
        let model = Fixture(AtomicUsize::new(0));
        let collector = Collector::new(
            Api {
                title: "Ask HN: Alpha",
            },
            CollectionConfig::default(),
        );
        let err = execute(
            &mut store,
            &collector,
            &model,
            &NoDiscovery,
            &args(dir.path(), ApprovalPolicy::ApproveThread),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.contains("live Show HN:"));
        assert_eq!(store.extraction_mode(100).unwrap(), None);
        store
            .assign_extraction_mode(100, ExtractionMode::Monthly)
            .unwrap();
        let err = execute(
            &mut store,
            &collector,
            &model,
            &NoDiscovery,
            &args(dir.path(), ApprovalPolicy::ApproveThread),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.contains("assigned to monthly"));
        assert_eq!(model.0.load(Ordering::SeqCst), 0);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::collector::FetchError;
    use crate::extraction::Extraction;
    use crate::show_extraction::ShowExtraction;
    use crate::show_hn_discovery::SearchHit;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Api(serde_json::Value);
    impl HnApi for Api {
        fn fetch_item(&self, _: i64) -> Result<Option<serde_json::Value>, FetchError> {
            Ok(Some(self.0.clone()))
        }
    }
    struct Never(AtomicUsize);
    impl Model for Never {
        fn name(&self) -> &str {
            "never"
        }
        fn extract(&self, _: &str) -> Result<Extraction, String> {
            panic!("monthly model called")
        }
        fn extract_show(&self, _: &str) -> Result<ShowExtraction, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("ineligible source must never call model")
        }
    }
    struct Candidates;
    impl Discovery for Candidates {
        fn search(&self, _: i64, end: i64, _: usize) -> Result<Vec<SearchHit>, String> {
            Ok(if end >= 2500 {
                vec![SearchHit {
                    id: "100".into(),
                    title: Some("Show HN: Alpha".into()),
                    created_at_i: Some(2500),
                }]
            } else {
                vec![]
            })
        }
    }
    #[test]
    fn collected_date_recheck_and_deleted_sources_export_without_extraction() {
        for deleted in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = Store::open(dir.path().join("db")).unwrap();
            let model = Never(AtomicUsize::new(0));
            let collector = Collector::new(
                Api(
                    serde_json::json!({"id":100,"type":"story","by":"alice","time":1000,"title":"Show HN: Alpha","deleted":deleted,"kids":[]}),
                ),
                CollectionConfig::default(),
            );
            if deleted {
                collector.collect(&mut store, 100).unwrap();
                store
                    .assign_extraction_mode(100, ExtractionMode::ShowHn)
                    .unwrap();
            }
            let args = Args {
                selection: Selection::Range(DateRange {
                    start: 2000,
                    end: 3000,
                }),
                db: None,
                export: dir.path().join("export"),
                options: processing::Options::default(),
                reprocess: false,
                approval: ApprovalPolicy::ApproveThread,
                batch_size: DEFAULT_BATCH_SIZE,
            };
            let mut output = vec![];
            execute(
                &mut store,
                &collector,
                &model,
                &Candidates,
                &args,
                &mut output,
            )
            .unwrap();
            assert_eq!(model.0.load(Ordering::SeqCst), 0);
            assert!(args.export.join("current.json").exists());
            assert!(String::from_utf8(output).unwrap().contains("skipped"));
        }
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use crate::collector::FetchError;
    use crate::extraction::{Extraction, Mention};
    use crate::show_extraction::{ShowEvidence, ShowExtraction, ShowProject};
    use crate::show_hn_discovery::SearchHit;
    use std::sync::Mutex;

    /// Story 6 shares story 7's timestamp to exercise batch-boundary ties.
    fn time(id: i64) -> i64 {
        1000 + if id == 6 { 7 } else { id } * 10
    }
    struct Api;
    impl HnApi for Api {
        fn fetch_item(&self, id: i64) -> Result<Option<serde_json::Value>, FetchError> {
            Ok(Some(
                serde_json::json!({"id":id,"type":"story","by":"alice","time":time(id),"title":format!("Show HN: Story {id}"),"url":"https://alpha.test","kids":[]}),
            ))
        }
    }
    #[derive(Default)]
    struct Fixture {
        fail: Vec<i64>,
        order: Mutex<Vec<i64>>,
    }
    impl Model for Fixture {
        fn name(&self) -> &str {
            "range-fixture"
        }
        fn extract(&self, _: &str) -> Result<Extraction, String> {
            panic!("monthly extraction must not run")
        }
        fn extract_show(&self, prompt: &str) -> Result<ShowExtraction, String> {
            let story = serde_json::from_str::<serde_json::Value>(prompt).unwrap()["story_id"]
                .as_i64()
                .unwrap();
            self.order.lock().unwrap().push(story);
            if self.fail.contains(&story) {
                return Err("permanent failure".into());
            }
            let title = format!("Show HN: Story {story}");
            Ok(ShowExtraction {
                project: Some(ShowProject {
                    mention: Mention {
                        source_item_id: story,
                        name: Some(format!("Project {story}")),
                        description: "Organizes work".into(),
                        urls: vec![],
                        problem: None,
                        audience: None,
                        source_excerpt: title.clone(),
                        builders: vec![],
                        categories: vec![],
                        audience_labels: vec![],
                    },
                    evidence: vec![ShowEvidence {
                        source_item_id: story,
                        source_field: "title".into(),
                        value: title,
                        supports: vec!["name".into(), "description".into()],
                        authorization_source_item_id: None,
                    }],
                }),
                no_project_reason: None,
            })
        }
    }
    /// A corpus served like search_by_date; optionally fails from the Nth request.
    struct Corpus {
        stories: Vec<i64>,
        fail_from_call: Option<usize>,
        calls: Mutex<usize>,
    }
    impl Corpus {
        fn new(stories: &[i64]) -> Self {
            Self {
                stories: stories.to_vec(),
                fail_from_call: None,
                calls: Mutex::new(0),
            }
        }
    }
    impl Discovery for Corpus {
        fn search(&self, start: i64, end: i64, limit: usize) -> Result<Vec<SearchHit>, String> {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            if self.fail_from_call.is_some_and(|n| *calls >= n) {
                return Err("search offline".into());
            }
            let mut ids: Vec<_> = self
                .stories
                .iter()
                .copied()
                .filter(|id| (start..=end).contains(&time(*id)))
                .collect();
            ids.sort_by_key(|id| std::cmp::Reverse((time(*id), *id)));
            Ok(ids
                .into_iter()
                .take(limit)
                .map(|id| SearchHit {
                    id: id.to_string(),
                    title: Some(format!("Show HN: Story {id}")),
                    created_at_i: Some(time(id)),
                })
                .collect())
        }
    }
    fn range_args(dir: &std::path::Path, approval: ApprovalPolicy, batch: usize) -> Args {
        Args {
            selection: Selection::Range(DateRange {
                start: 0,
                end: 100_000,
            }),
            db: None,
            export: dir.join("export"),
            // Sequential extraction keeps model-call order assertable.
            options: processing::Options {
                concurrency: 1,
                ..processing::Options::default()
            },
            reprocess: false,
            approval,
            batch_size: batch,
        }
    }
    fn run(
        store: &mut Store,
        model: &Fixture,
        discovery: &Corpus,
        args: &Args,
    ) -> (Result<(), String>, String) {
        let collector = Collector::new(Api, CollectionConfig::default());
        let mut out = Vec::new();
        let result = execute(store, &collector, model, discovery, args, &mut out);
        (result, String::from_utf8(out).unwrap())
    }
    fn setup() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("db")).unwrap();
        (dir, store)
    }

    #[test]
    fn batches_run_newest_first_with_one_export_each() {
        let (dir, mut store) = setup();
        let model = Fixture::default();
        let discovery = Corpus::new(&[1, 2, 3, 4, 5]);
        let args = range_args(dir.path(), ApprovalPolicy::ApproveThread, 3);
        let (result, out) = run(&mut store, &model, &discovery, &args);
        result.unwrap();
        assert_eq!(*model.order.lock().unwrap(), [5, 4, 3, 2, 1]);
        assert!(out.contains("Batch 1: searching stories on or before"));
        assert!(out.contains("Batch 1: 3 stories ("));
        assert!(out.contains("Batch 2: 2 stories ("));
        for n in 1..=2 {
            assert!(out.contains(&format!("Batch {n}: approved ")), "{out}");
        }
        // Exports happen per batch, not per story.
        assert_eq!(out.matches("Public snapshot:").count(), 2, "{out}");
        assert!(out.contains("Show HN summary: 2 batch(es), 5 story(ies) succeeded, 0 failed"));
        assert!(
            crate::publication::pending_mentions(&mut store)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn tie_at_batch_boundary_loses_no_story() {
        let (dir, mut store) = setup();
        let model = Fixture::default();
        // Stories 7 and 6 share a timestamp that straddles the first batch edge.
        let discovery = Corpus::new(&[9, 8, 7, 6, 4]);
        let args = range_args(dir.path(), ApprovalPolicy::Manual, 3);
        let (result, out) = run(&mut store, &model, &discovery, &args);
        result.unwrap();
        assert_eq!(*model.order.lock().unwrap(), [9, 8, 7, 6, 4], "{out}");
    }

    #[test]
    fn failed_story_is_reported_and_later_work_continues() {
        let (dir, mut store) = setup();
        let model = Fixture {
            fail: vec![4],
            ..Fixture::default()
        };
        let discovery = Corpus::new(&[1, 2, 3, 4, 5]);
        let args = range_args(dir.path(), ApprovalPolicy::ApproveThread, 3);
        let (result, out) = run(&mut store, &model, &discovery, &args);
        let error = result.unwrap_err();
        assert!(error.contains("1 story failure"), "{error}");
        assert_eq!(*model.order.lock().unwrap(), [5, 4, 3, 2, 1]);
        assert!(out.contains("Story 4: failed: permanent failure"), "{out}");
        assert!(
            out.contains("1 x permanent failure (e.g. story 4)"),
            "{out}"
        );
        assert!(out.contains("4 story(ies) succeeded, 1 failed"), "{out}");
        // The failed story is never approved; the others are.
        let approved: Vec<i64> = store
            .connection()
            .prepare("SELECT root_item_id FROM mentions WHERE review_state='approved' ORDER BY 1")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(approved, [1, 2, 3, 5]);
        assert!(args.export.join("current.json").exists());
    }

    #[test]
    fn monthly_conflict_is_a_per_story_failure() {
        let (dir, mut store) = setup();
        Collector::new(Api, CollectionConfig::default())
            .collect(&mut store, 2)
            .unwrap();
        store
            .assign_extraction_mode(2, ExtractionMode::Monthly)
            .unwrap();
        let model = Fixture::default();
        let discovery = Corpus::new(&[1, 2, 3]);
        let args = range_args(dir.path(), ApprovalPolicy::Manual, 10);
        let (result, out) = run(&mut store, &model, &discovery, &args);
        assert!(result.is_err());
        assert!(out.contains("Story 2: failed: thread 2 is assigned to monthly"));
        assert_eq!(*model.order.lock().unwrap(), [3, 1]);
    }

    #[test]
    fn concurrent_extraction_within_a_batch_processes_every_story() {
        let (dir, mut store) = setup();
        let model = Fixture::default();
        let discovery = Corpus::new(&[1, 2, 3, 4, 5]);
        let mut args = range_args(dir.path(), ApprovalPolicy::ApproveThread, 10);
        args.options.concurrency = 3;
        let (result, out) = run(&mut store, &model, &discovery, &args);
        result.unwrap();
        let mut seen = model.order.lock().unwrap().clone();
        seen.sort();
        assert_eq!(seen, [1, 2, 3, 4, 5]);
        assert_eq!(out.matches("Public snapshot:").count(), 1, "{out}");
        assert!(out.contains("Batch 1: approved 5 mentions."), "{out}");
    }

    #[test]
    fn manual_runs_never_approve() {
        let (dir, mut store) = setup();
        let model = Fixture::default();
        let discovery = Corpus::new(&[1, 2]);
        let args = range_args(dir.path(), ApprovalPolicy::Manual, 10);
        let (result, out) = run(&mut store, &model, &discovery, &args);
        result.unwrap();
        assert!(out.contains("Batch 1: results await review. Public snapshot:"));
        assert_eq!(
            crate::publication::pending_mentions(&mut store)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn discovery_failure_keeps_completed_batches_and_fails_the_run() {
        let (dir, mut store) = setup();
        let model = Fixture::default();
        let mut discovery = Corpus::new(&[1, 2, 3, 4]);
        discovery.fail_from_call = Some(2);
        let args = range_args(dir.path(), ApprovalPolicy::ApproveThread, 2);
        let (result, out) = run(&mut store, &model, &discovery, &args);
        let error = result.unwrap_err();
        assert!(error.contains("discovery stopped"), "{error}");
        assert!(out.contains("Batch 2: discovery failed: search offline"));
        assert_eq!(*model.order.lock().unwrap(), [4, 3]);
        assert!(args.export.join("current.json").exists());
    }

    #[test]
    fn empty_range_succeeds_without_batches() {
        let (dir, mut store) = setup();
        let model = Fixture::default();
        let discovery = Corpus::new(&[]);
        let args = range_args(dir.path(), ApprovalPolicy::ApproveThread, 10);
        let (result, out) = run(&mut store, &model, &discovery, &args);
        result.unwrap();
        assert!(out.contains("0 batch(es)"), "{out}");
        assert!(!args.export.exists());
        assert!(model.order.lock().unwrap().is_empty());
    }

    #[test]
    fn direct_thread_still_stops_on_failure() {
        let (dir, mut store) = setup();
        let model = Fixture {
            fail: vec![100],
            ..Fixture::default()
        };
        let mut args = range_args(dir.path(), ApprovalPolicy::ApproveThread, 10);
        args.selection = Selection::Thread(100);
        let (result, _) = run(&mut store, &model, &Corpus::new(&[]), &args);
        let error = result.unwrap_err();
        assert!(error.starts_with("story 100:"), "{error}");
        assert!(error.contains("Rerun the same command"));
    }
}
