//! Deterministic HN item-tree reader. Network access is behind `HnApi`.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::store::{FetchFailure, SourceItem, Store};

const HN_ITEM_URL: &str = "https://hacker-news.firebaseio.com/v0/item";

pub trait HnApi: Send + Sync {
    /// `None` means the API returned JSON `null`; it is not a deletion signal.
    fn fetch_item(&self, item_id: i64) -> Result<Option<Value>, FetchError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchError {
    pub kind: FetchErrorKind,
    pub detail: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchErrorKind {
    Http,
    Transport,
    InvalidPayload,
}

impl FetchErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Transport => "transport",
            Self::InvalidPayload => "invalid_payload",
        }
    }
}

impl FetchError {
    pub fn transient(kind: FetchErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            retryable: true,
        }
    }

    pub fn permanent(kind: FetchErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            retryable: false,
        }
    }
}

/// Production HTTP transport. Constructing it does not make a request.
pub struct HttpHnApi {
    agent: ureq::Agent,
}

impl Default for HttpHnApi {
    fn default() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .new_agent();
        Self { agent }
    }
}

impl HnApi for HttpHnApi {
    fn fetch_item(&self, item_id: i64) -> Result<Option<Value>, FetchError> {
        let url = format!("{HN_ITEM_URL}/{item_id}.json");
        let mut response = self.agent.get(&url).call().map_err(|error| {
            let retryable = match error {
                ureq::Error::StatusCode(code) => code == 429 || code >= 500,
                ureq::Error::Timeout(_) | ureq::Error::Io(_) | ureq::Error::HostNotFound => true,
                _ => false,
            };
            FetchError {
                kind: FetchErrorKind::Http,
                detail: error.to_string(),
                retryable,
            }
        })?;
        response
            .body_mut()
            .read_json::<Option<Value>>()
            .map_err(|error| {
                FetchError::permanent(FetchErrorKind::InvalidPayload, error.to_string())
            })
    }
}

#[derive(Debug, Clone)]
pub struct CollectionConfig {
    pub max_concurrency: usize,
    pub max_attempts: u32,
    pub retry_base_delay: Duration,
}

impl Default for CollectionConfig {
    fn default() -> Self {
        Self {
            max_concurrency: 8,
            max_attempts: 3,
            retry_base_delay: Duration::from_millis(200),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionReport {
    pub run_id: i64,
    pub resumed_from_run_id: Option<i64>,
    pub thread_id: i64,
    pub items_fetched: usize,
    pub changed_item_ids: Vec<i64>,
    pub failures: Vec<FetchFailure>,
    pub complete: bool,
}

pub struct Collector<C> {
    api: C,
    config: CollectionConfig,
}

impl<C: HnApi> Collector<C> {
    pub fn new(api: C, config: CollectionConfig) -> Self {
        Self { api, config }
    }

    /// Refreshes the supplied item and every reachable descendant. Each item
    /// is fetched once per run, apart from bounded retries for transient errors.
    pub fn collect(&self, store: &mut Store, thread_id: i64) -> rusqlite::Result<CollectionReport> {
        if thread_id <= 0 || self.config.max_concurrency == 0 || self.config.max_attempts == 0 {
            return Err(rusqlite::Error::InvalidParameterName(
                "thread ID, max concurrency and max attempts must be positive".into(),
            ));
        }
        let resumed_from_run_id = store
            .latest_run_for_thread(thread_id)?
            .filter(|run| run.status != "complete")
            .map(|run| run.id);
        let run_id = store.begin_run(thread_id, unix_time())?;
        let mut queue = VecDeque::from([Pending {
            id: thread_id,
            parent_id: None,
            position: 0,
        }]);
        let mut seen = HashSet::from([thread_id]);
        let mut fetched = 0;
        let mut failures = Vec::new();

        thread::scope(|scope| -> rusqlite::Result<()> {
            let (sender, receiver) = mpsc::channel();
            let mut active = 0;
            while !queue.is_empty() || active > 0 {
                while active < self.config.max_concurrency {
                    let Some(pending) = queue.pop_front() else {
                        break;
                    };
                    let sender = sender.clone();
                    scope.spawn(move || {
                        let response = fetch_with_retries(&self.api, pending.id, &self.config);
                        let _ = sender.send((pending, response));
                    });
                    active += 1;
                }
                let (pending, response) = receiver.recv().expect("scoped fetch worker terminated");
                active -= 1;
                let value = match response {
                    Ok(Some(value)) => value,
                    Ok(None) => {
                        record_failure(
                            store,
                            run_id,
                            pending.id,
                            "unavailable",
                            "API returned null",
                            &mut failures,
                        )?;
                        continue;
                    }
                    Err(error) => {
                        record_failure(
                            store,
                            run_id,
                            pending.id,
                            error.kind.as_str(),
                            &error.detail,
                            &mut failures,
                        )?;
                        continue;
                    }
                };
                let (item, children) = match parse_item(value, thread_id, &pending) {
                    Ok(parsed) => parsed,
                    Err(detail) => {
                        record_failure(
                            store,
                            run_id,
                            pending.id,
                            "invalid_payload",
                            &detail,
                            &mut failures,
                        )?;
                        continue;
                    }
                };
                store.upsert_item(run_id, unix_time(), &item)?;
                fetched += 1;
                for (position, child_id) in children.into_iter().enumerate() {
                    if seen.insert(child_id) {
                        queue.push_back(Pending {
                            id: child_id,
                            parent_id: Some(item.item_id),
                            position: position as i64,
                        });
                    } else {
                        record_failure(
                            store,
                            run_id,
                            child_id,
                            "invalid_payload",
                            "duplicate or cyclic child reference",
                            &mut failures,
                        )?;
                    }
                }
            }
            Ok(())
        })?;

        store.finish_run(run_id, unix_time())?;
        Ok(CollectionReport {
            run_id,
            resumed_from_run_id,
            thread_id,
            items_fetched: fetched,
            changed_item_ids: store.changed_item_ids_for_run(run_id)?,
            complete: failures.is_empty(),
            failures,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    id: i64,
    parent_id: Option<i64>,
    position: i64,
}

fn fetch_with_retries<C: HnApi>(
    api: &C,
    id: i64,
    config: &CollectionConfig,
) -> Result<Option<Value>, FetchError> {
    for attempt in 1..config.max_attempts {
        match api.fetch_item(id) {
            Err(error) if error.retryable => {
                thread::sleep(config.retry_base_delay.saturating_mul(attempt));
            }
            result => return result,
        }
    }
    api.fetch_item(id)
}

fn record_failure(
    store: &Store,
    run_id: i64,
    item_id: i64,
    kind: &str,
    detail: &str,
    failures: &mut Vec<FetchFailure>,
) -> rusqlite::Result<()> {
    let failure = FetchFailure {
        item_id,
        kind: kind.into(),
        detail: detail.into(),
        occurred_at: unix_time(),
    };
    store.record_fetch_failure(run_id, &failure)?;
    failures.push(failure);
    Ok(())
}

fn parse_item(
    value: Value,
    thread_id: i64,
    pending: &Pending,
) -> Result<(SourceItem, Vec<i64>), String> {
    let object = value.as_object().ok_or("item is not an object")?;
    let id = object
        .get("id")
        .and_then(Value::as_i64)
        .ok_or("missing or invalid item ID")?;
    if id != pending.id {
        return Err(format!("expected item {}, received {id}", pending.id));
    }
    let parent_id = optional_i64(object.get("parent"), "parent")?;
    if pending.parent_id.is_some() && parent_id != pending.parent_id {
        return Err(format!("item {id} has unexpected parent {parent_id:?}"));
    }
    let author = optional_string(object.get("by"), "by")?;
    let created_at = optional_i64(object.get("time"), "time")?;
    let body = optional_string(object.get("text"), "text")?;
    let deleted = optional_bool(object.get("deleted"), "deleted")?.unwrap_or(false);
    let dead = optional_bool(object.get("dead"), "dead")?.unwrap_or(false);
    let children = match object.get("kids") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_i64()
                    .filter(|id| *id > 0)
                    .ok_or_else(|| "invalid child ID".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("kids is not an array".into()),
    };
    Ok((
        SourceItem {
            item_id: id,
            thread_id,
            parent_id,
            sibling_position: pending.position,
            author,
            created_at,
            body,
            raw_payload: value,
            deleted,
            dead,
        },
        children,
    ))
}

fn optional_i64(value: Option<&Value>, field: &str) -> Result<Option<i64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("invalid {field}")),
    }
}

fn optional_string(value: Option<&Value>, field: &str) -> Result<Option<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(format!("invalid {field}")),
    }
}

fn optional_bool(value: Option<&Value>, field: &str) -> Result<Option<bool>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err(format!("invalid {field}")),
    }
}

fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
