//! Show HN CLI selection and presentation; orchestration lives in workflow.
use crate::cli::{Status, failure_summary};
use crate::collector::{CollectionConfig, Collector, HnApi};
use crate::processing::{self, Model, RigModel};
use crate::show_hn_discovery::{Candidate, DateRange, Discovery, matches_title};
use crate::store::{ExtractionMode, Store};
use crate::workflow::{self, ApprovalPolicy, Event, ThreadOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;

const HELP: &str = "Usage: wrkgn-show-hn --thread ID | --from YYYY-MM-DD --through YYYY-MM-DD
    [--auto-approve] [--reprocess] [--concurrency N] [--db PATH] [--export DIRECTORY]

Explicitly collect, process and export each Show HN story, newest first.
Calendar ranges are inclusive UTC dates. Results await manual review unless
--auto-approve is supplied. Rerun to refresh and resume unchanged successful work.
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
                "--thread" | "--from" | "--through" | "--db" | "--export" | "--concurrency" => {}
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
                _ => unreachable!(),
            }
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
    let (stories, range, direct) = match args.selection {
        Selection::Thread(id) => (vec![Candidate { id, timestamp: 0 }], None, true),
        Selection::Range(range) => (discovery.stories(range)?, Some(range), false),
    };
    let mut unique: BTreeMap<i64, i64> = BTreeMap::new();
    for candidate in stories {
        unique.insert(candidate.id, candidate.timestamp);
    }
    let mut stories: Vec<_> = unique.into_iter().collect();
    stories.sort_by_key(|(id, date)| (std::cmp::Reverse(*date), std::cmp::Reverse(*id)));
    for (thread, _) in stories {
        if store.extraction_mode(thread)? == Some(ExtractionMode::Monthly) {
            return Err(format!(
                "thread {thread} is assigned to monthly extraction; Show HN workflow cannot process it"
            ));
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
            |store, thread| {
                let item = store
                    .get_item(thread)
                    .map_err(|e| e.to_string())?
                    .ok_or("collected story is missing")?;
                let source = &item.source;
                if source.deleted || source.dead {
                    return Ok(false);
                }
                let qualifies = source.raw_payload.get("type").and_then(|v| v.as_str())
                    == Some("story")
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
            },
            |event| {
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
            },
        );
        match result {
            Ok(()) => {}
            Err(error) => {
                status.finish();
                let message = match error {
                    workflow::Error::Operation(e) => e,
                    workflow::Error::ProcessingIncomplete(r) => failure_summary(&r),
                };
                return Err(format!(
                    "story {thread}: {message}\nRerun the same command to resume; completed unchanged work is skipped."
                ));
            }
        }
        out.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
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
        fn stories(&self, _: DateRange) -> Result<Vec<Candidate>, String> {
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
        fn stories(&self, _: DateRange) -> Result<Vec<Candidate>, String> {
            Ok(vec![Candidate {
                id: 100,
                timestamp: 1000,
            }])
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
