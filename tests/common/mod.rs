//! Offline fakes shared by the run-infrastructure tests. Each test file that
//! uses this module declares `mod common;`.
#![allow(dead_code)]

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;
use wrkgn::collector::{FetchError, FetchErrorKind, HnApi};
use wrkgn::extraction::Extraction;
use wrkgn::processing::Model;
use wrkgn::run::status::Clock;
use wrkgn::show_extraction::ShowExtraction;
use wrkgn::show_hn_discovery::SearchHit;
use wrkgn::store::Store;

/// A fresh temporary database. Keep the `TempDir` alive for the test.
pub fn fresh_store() -> (TempDir, PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wrkgn.sqlite");
    let store = Store::open(&path).unwrap();
    (dir, path, store)
}

/// A thread root with `kids` as direct replies; each kid gets a body.
pub fn thread(root: i64, kids: &[i64]) -> HashMap<i64, Value> {
    let mut items = HashMap::new();
    items.insert(
        root,
        json!({"id": root, "by": "op", "time": 1000, "title": "Ask HN: Who is hiring?", "kids": kids}),
    );
    for (n, kid) in kids.iter().enumerate() {
        items.insert(
            *kid,
            json!({"id": kid, "parent": root, "by": "user", "time": 1001 + n as i64,
                   "text": format!("Comment {kid}")}),
        );
    }
    items
}

/// Like `thread`, with a monthly-thread title (for example
/// `What are you working on (March 2026)`).
pub fn titled_thread(root: i64, title: &str, kids: &[i64]) -> HashMap<i64, Value> {
    let mut items = thread(root, kids);
    items.get_mut(&root).unwrap()["title"] = json!(title);
    items
}

type FetchHook = Box<dyn Fn(i64) + Send + Sync>;

/// HN API fixtures with per-ID call counts and failure injection.
#[derive(Default)]
pub struct FakeHn {
    pub items: HashMap<i64, Value>,
    /// IDs that return JSON `null`.
    pub null: HashSet<i64>,
    /// IDs that fail with a retryable transport error on every call.
    pub transient: HashSet<i64>,
    /// IDs that fail with a permanent error on every call.
    pub permanent: HashSet<i64>,
    pub on_fetch: Option<FetchHook>,
    calls: Mutex<HashMap<i64, usize>>,
}

impl FakeHn {
    pub fn new(items: HashMap<i64, Value>) -> Self {
        Self {
            items,
            ..Self::default()
        }
    }

    pub fn calls(&self, id: i64) -> usize {
        self.calls.lock().unwrap().get(&id).copied().unwrap_or(0)
    }

    pub fn total_calls(&self) -> usize {
        self.calls.lock().unwrap().values().sum()
    }
}

impl HnApi for &FakeHn {
    fn fetch_item(&self, id: i64) -> Result<Option<Value>, FetchError> {
        *self.calls.lock().unwrap().entry(id).or_default() += 1;
        if let Some(hook) = &self.on_fetch {
            hook(id);
        }
        if self.transient.contains(&id) {
            return Err(FetchError::transient(
                FetchErrorKind::Transport,
                format!("error sending request for url (https://hn.test/v0/item/{id}.json)"),
            ));
        }
        if self.permanent.contains(&id) {
            return Err(FetchError::permanent(FetchErrorKind::Http, "status 404"));
        }
        if self.null.contains(&id) {
            return Ok(None);
        }
        Ok(self.items.get(&id).cloned())
    }
}

/// Monthly and Show HN discovery fixtures.
#[derive(Default)]
pub struct FakeDiscovery {
    /// Month (`YYYY-MM`) to thread IDs or an error.
    pub months: HashMap<String, Result<Vec<i64>, String>>,
    /// Show HN hits as `(id, title, created_at_i)`.
    pub stories: Vec<(i64, String, i64)>,
    pub search_calls: AtomicUsize,
    /// Show HN searches (1-based call number) that fail with this message.
    pub search_failures: Mutex<Vec<(usize, String)>>,
    /// Show HN search window `(start, end, limit)` of every call.
    pub search_log: Mutex<Vec<(i64, i64, usize)>>,
    /// When set, searches count toward the Algolia circuit breaker like the
    /// real discovery does.
    pub breaker: Mutex<Option<std::sync::Arc<wrkgn::run::RunControl>>>,
}

impl wrkgn::monthly::Discovery for FakeDiscovery {
    fn threads(&self, month: wrkgn::monthly::Month) -> Result<Vec<i64>, String> {
        self.search_calls.fetch_add(1, Ordering::SeqCst);
        self.months
            .get(&month.to_string())
            .cloned()
            .unwrap_or_else(|| Ok(Vec::new()))
    }
}

impl wrkgn::show_hn_discovery::Discovery for FakeDiscovery {
    fn search(&self, start: i64, end: i64, limit: usize) -> Result<Vec<SearchHit>, String> {
        let call = self.search_calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.search_log.lock().unwrap().push((start, end, limit));
        let failure = self
            .search_failures
            .lock()
            .unwrap()
            .iter()
            .find(|(n, _)| *n == call)
            .map(|(_, message)| message.clone());
        if let Some(control) = self.breaker.lock().unwrap().as_ref() {
            if failure.is_some() {
                control.breaker.record_failure(wrkgn::run::Api::Algolia);
            } else {
                control.breaker.record_success(wrkgn::run::Api::Algolia);
            }
        }
        if let Some(message) = failure {
            return Err(message);
        }
        let mut hits: Vec<_> = self
            .stories
            .iter()
            .filter(|(_, _, ts)| (start..=end).contains(ts))
            .collect();
        hits.sort_by_key(|(id, _, ts)| (std::cmp::Reverse(*ts), std::cmp::Reverse(*id)));
        Ok(hits
            .into_iter()
            .take(limit)
            .map(|(id, title, ts)| SearchHit {
                id: id.to_string(),
                title: Some(title.clone()),
                created_at_i: Some(*ts),
            })
            .collect())
    }
}

type CallHook = Box<dyn Fn(usize) + Send + Sync>;

/// A model that replays scripted answers (then empty extractions) and counts calls.
#[derive(Default)]
pub struct ScriptModel {
    pub script: Mutex<VecDeque<Result<Extraction, String>>>,
    pub calls: AtomicUsize,
    pub show_calls: AtomicUsize,
    /// Called with the zero-based call number before each answer.
    pub on_call: Option<CallHook>,
    /// Error used for every call when set, ignoring the script.
    pub always: Option<String>,
}

impl ScriptModel {
    pub fn new(script: Vec<Result<Extraction, String>>) -> Self {
        Self {
            script: Mutex::new(script.into()),
            ..Self::default()
        }
    }

    pub fn failing(error: &str) -> Self {
        Self {
            always: Some(error.into()),
            ..Self::default()
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Model for ScriptModel {
    fn name(&self) -> &str {
        "script-v1"
    }

    fn extract(&self, _prompt: &str) -> Result<Extraction, String> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(hook) = &self.on_call {
            hook(n);
        }
        if let Some(error) = &self.always {
            return Err(error.clone());
        }
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(Extraction { mentions: vec![] }))
    }

    fn extract_show(&self, _prompt: &str) -> Result<ShowExtraction, String> {
        self.show_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ShowExtraction {
            project: None,
            no_project_reason: Some("fixture".into()),
        })
    }
}

/// A clock the test advances by hand.
pub struct FakeClock(pub Cell<Instant>);

impl FakeClock {
    pub fn new() -> Self {
        Self(Cell::new(Instant::now()))
    }

    pub fn advance(&self, by: Duration) {
        self.0.set(self.0.get() + by);
    }
}

impl Clock for &FakeClock {
    fn now(&self) -> Instant {
        self.0.get()
    }
}
