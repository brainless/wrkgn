use crate::extraction::{self, Extraction, Mention};
use crate::store::{ConversationNode, Store, StoredSourceItem};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_CONCURRENCY: usize = 10;

pub trait Model {
    fn name(&self) -> &str;
    fn extract(&self, prompt: &str) -> Result<Extraction, String>;
}
pub struct RigModel {
    provider: Provider,
    model: String,
    identity: String,
    api_key: String,
    base_url: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    OpenAi,
    Neuralwatt,
}
impl RigModel {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    fn from_lookup(mut get: impl FnMut(&str) -> Option<String>) -> Result<Self, String> {
        let provider = match get("WRKGN_PROVIDER").as_deref().unwrap_or("openai") {
            "openai" => Provider::OpenAi,
            "neuralwatt" => Provider::Neuralwatt,
            _ => return Err("WRKGN_PROVIDER must be openai or neuralwatt".into()),
        };
        let (provider_name, key_name, default_model, default_url) = match provider {
            Provider::OpenAi => (
                "openai",
                "OPENAI_API_KEY",
                None,
                "https://api.openai.com/v1",
            ),
            Provider::Neuralwatt => (
                "neuralwatt",
                "NEURALWATT_API_KEY",
                Some("glm-5.3-flash"),
                "https://api.neuralwatt.com/v1",
            ),
        };
        let model = get("WRKGN_MODEL")
            .or_else(|| default_model.map(str::to_owned))
            .filter(|value| !value.trim().is_empty())
            .ok_or("WRKGN_MODEL is required")?;
        let api_key = get(key_name)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("{key_name} is required"))?;
        let base_url = match provider {
            Provider::OpenAi => get("OPENAI_BASE_URL"),
            Provider::Neuralwatt => get("NEURALWATT_BASE_URL"),
        }
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default_url.to_owned());
        // Include the endpoint in the cache identity without persisting a URL
        // that might itself contain a private token.
        let endpoint_hash = format!("{:x}", Sha256::digest(base_url.as_bytes()));
        let identity = format!("{provider_name}/{model}@{}", &endpoint_hash[..16]);
        Ok(Self {
            provider,
            model,
            identity,
            api_key,
            base_url,
        })
    }
}
impl Model for RigModel {
    fn name(&self) -> &str {
        &self.identity
    }
    fn extract(&self, prompt: &str) -> Result<Extraction, String> {
        use rig_agent::extractor::ExtractorBuilder;
        use rig_core::providers::openai::OpenAIConfig;
        let client = OpenAIConfig::new(self.api_key.clone())
            .with_base_url(self.base_url.clone())
            .client();
        let model: rig_core::DynModel<rig_core::operation::Completion> = match self.provider {
            Provider::OpenAi => client.completion(self.model.as_str()).into(),
            Provider::Neuralwatt => client.chat(self.model.as_str()).into(),
        };
        let taxonomy = crate::category::metadata()
            .into_iter()
            .map(|c| format!("{} ({}): {}", c.slug, c.label, c.description))
            .collect::<Vec<_>>()
            .join("\n");
        let preamble = format!(
            "{}\nAvailable categories:\n{}",
            extraction::SYSTEM_PROMPT,
            taxonomy
        );
        let extractor = ExtractorBuilder::<Extraction>::new(model)
            .append_preamble(&preamble)
            .retries(1)
            .build();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        runtime
            .block_on(async { extractor.extract(prompt).await })
            .map(|r| r.output)
            .map_err(|e| e.to_string())
    }
}
#[derive(Default, Debug)]
pub struct Report {
    pub run_id: i64,
    pub processed: usize,
    pub skipped: usize,
    pub failed: Vec<(i64, String)>,
    /// Groups never sent to the model because the run stopped early.
    pub not_attempted: usize,
    pub stopped_early: bool,
}
pub struct Options {
    pub concurrency: usize,
    /// Model calls per batch, including the first, for rate-limit and connection errors.
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    /// Stop handing out groups once this many have exhausted their retries on
    /// rate-limit or connection errors.
    pub stop_after_transient_failures: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            concurrency: DEFAULT_CONCURRENCY,
            max_attempts: 6,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
            stop_after_transient_failures: 3,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub failed: usize,
}
/// Rate limits, provider overload, and transport failures; retrying may succeed.
pub fn is_transient(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    [
        "status 429",
        "too many requests",
        "rate_limit",
        "concurrent limit",
        "error sending request",
        "error decoding response body",
        "timed out",
        "connection reset",
        "status 500",
        "status 502",
        "status 503",
        "status 504",
    ]
    .iter()
    .any(|p| e.contains(p))
}
/// A short label for grouping failures in CLI output.
pub fn failure_kind(error: &str) -> String {
    let e = error.to_ascii_lowercase();
    if e.contains("429") || e.contains("rate_limit") || e.contains("concurrent limit") {
        "rate limited (429)".into()
    } else if is_transient(error) {
        "connection or provider error".into()
    } else {
        error.chars().take(80).collect()
    }
}
/// Processes groups one at a time, so model calls happen in group order.
pub fn process_thread<M: Model + Sync>(
    store: &mut Store,
    thread_id: i64,
    model: &M,
    reprocess: bool,
) -> Result<Report, String> {
    let options = Options {
        concurrency: 1,
        ..Options::default()
    };
    process_thread_with(store, thread_id, model, reprocess, &options, |_| {})
}
/// Runs up to `options.concurrency` group extractions in parallel. SQLite writes
/// stay on this thread and are applied in group order as results arrive.
pub fn process_thread_with<M: Model + Sync>(
    store: &mut Store,
    thread_id: i64,
    model: &M,
    reprocess: bool,
    options: &Options,
    mut progress: impl FnMut(Progress),
) -> Result<Report, String> {
    let latest = store
        .latest_run_for_thread(thread_id)
        .map_err(|e| e.to_string())?;
    if latest.as_ref().is_none_or(|run| run.status != "complete") {
        return Err("latest collection for thread is incomplete".into());
    }
    let tree = store
        .conversation_groups(thread_id)
        .map_err(|e| e.to_string())?;
    let now = unix_time();
    let conn = store.connection();
    conn.execute("UPDATE processing_runs SET status='interrupted',finished_at=?1 WHERE thread_id=?2 AND status='running'",params![now,thread_id]).map_err(|e|e.to_string())?;
    conn.execute("INSERT INTO processing_runs(thread_id,started_at,status,prompt_version,schema_version,model) VALUES(?1,?2,'running',?3,?4,?5)",params![thread_id,now,extraction::PROMPT_VERSION,extraction::SCHEMA_VERSION,model.name()]).map_err(|e|e.to_string())?;
    let run_id = conn.last_insert_rowid();
    let mut report = Report {
        run_id,
        ..Report::default()
    };
    let mut pending = Vec::new();
    for group in &tree.groups {
        let root = group.item.source.item_id;
        let fp = extraction::group_fingerprint(group);
        let previous:Option<(String,String,String,String,String)>=conn.query_row("SELECT fingerprint,prompt_version,schema_version,model,status FROM processing_groups WHERE root_item_id=?1",[root],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(|e|e.to_string())?;
        if !reprocess
            && previous.as_ref().is_some_and(|p| {
                p.0 == fp
                    && p.1 == extraction::PROMPT_VERSION
                    && p.2 == extraction::SCHEMA_VERSION
                    && p.3 == model.name()
                    && p.4 == "complete"
            })
        {
            report.skipped += 1;
            continue;
        }
        pending.push((group, fp));
    }
    let next = AtomicUsize::new(0);
    let transient_failures = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let mut received = 0;
    std::thread::scope(|scope| -> Result<(), String> {
        let (sender, results) = mpsc::channel();
        for _ in 0..options.concurrency.clamp(1, pending.len().max(1)) {
            let sender = sender.clone();
            let (next, pending, stop, transient_failures) =
                (&next, &pending, &stop, &transient_failures);
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((group, _)) = pending.get(i) else {
                        break;
                    };
                    let result = extract_group(group, model, options, stop);
                    if result.as_ref().is_err_and(|e| is_transient(e))
                        && transient_failures.fetch_add(1, Ordering::Relaxed) + 1
                            >= options.stop_after_transient_failures
                    {
                        stop.store(true, Ordering::Relaxed);
                    }
                    if sender.send((i, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        progress(Progress {
            done: 0,
            total: pending.len(),
            failed: 0,
        });
        let mut ready = BTreeMap::new();
        let mut saved = 0;
        for (i, result) in results {
            received += 1;
            ready.insert(i, result);
            while let Some(result) = ready.remove(&saved) {
                let (group, fp) = &pending[saved];
                saved += 1;
                let root = group.item.source.item_id;
                match result.and_then(|outputs| {
                    save_group(conn, run_id, thread_id, group, model, fp, &outputs)
                }) {
                    Ok(()) => report.processed += 1,
                    Err(e) => {
                        let mut all = Vec::new();
                        extraction::flatten(group, &mut all);
                        conn.execute("INSERT INTO processing_failures(run_id,root_item_id,item_count,processed_item_count,error) VALUES(?1,?2,?3,0,?4)",params![run_id,root,all.len() as i64,e]).map_err(|x|x.to_string())?;
                        report.failed.push((root, e));
                    }
                }
            }
            progress(Progress {
                done: received,
                total: pending.len(),
                failed: report.failed.len(),
            });
        }
        Ok(())
    })?;
    // Workers claim groups in order, so unreceived groups form a suffix that was
    // never sent to the model.
    report.not_attempted = pending.len() - received;
    report.stopped_early = stop.load(Ordering::Relaxed);
    let complete = report.failed.is_empty() && report.not_attempted == 0;
    conn.execute(
        "UPDATE processing_runs SET status=?1,finished_at=?2,error=?3 WHERE id=?4",
        params![
            if complete { "complete" } else { "incomplete" },
            unix_time(),
            if complete {
                None
            } else {
                Some(format!(
                    "{} group(s) failed, {} not attempted",
                    report.failed.len(),
                    report.not_attempted
                ))
            },
            run_id
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(report)
}
/// Retries rate-limit and connection errors with jittered exponential backoff.
fn extract_with_retry<M: Model>(
    model: &M,
    prompt: &str,
    options: &Options,
    stop: &AtomicBool,
) -> Result<Extraction, String> {
    let mut delay = options.initial_backoff;
    let mut attempt = 1;
    loop {
        match model.extract(prompt) {
            Err(e)
                if is_transient(&e)
                    && attempt < options.max_attempts
                    && !stop.load(Ordering::Relaxed) =>
            {
                std::thread::sleep(jitter(delay));
                delay = (delay * 2).min(options.max_backoff);
                attempt += 1;
            }
            result => return result,
        }
    }
}
/// Scales a delay into [50%, 100%) so parallel workers do not retry in lockstep.
fn jitter(delay: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    delay.mul_f64(0.5 + f64::from(nanos % 1000) / 2000.0)
}
/// Calls the model for every batch of one group. Touches no database state.
fn extract_group<M: Model>(
    group: &ConversationNode,
    model: &M,
    options: &Options,
    stop: &AtomicBool,
) -> Result<Vec<Mention>, String> {
    let mut items = Vec::new();
    extraction::flatten(group, &mut items);
    let source: BTreeMap<i64, &StoredSourceItem> =
        items.iter().map(|i| (i.source.item_id, *i)).collect();
    let batches = extraction::batches(group)?;
    let mut outputs = Vec::new();
    let mut covered: BTreeSet<i64> = BTreeSet::new();
    for batch in &batches {
        let out = extract_with_retry(model, &batch.prompt, options, stop)?;
        extraction::validate(&out, &batch.ids, &source)?;
        covered.extend(&batch.ids);
        outputs.extend(out.mentions);
    }
    if covered.len() != source.len() {
        return Err("not every source item was included in extraction batches".into());
    }
    Ok(outputs)
}
fn save_group<M: Model>(
    conn: &mut rusqlite::Connection,
    run_id: i64,
    thread_id: i64,
    group: &ConversationNode,
    model: &M,
    fp: &str,
    outputs: &[Mention],
) -> Result<(), String> {
    let root = group.item.source.item_id;
    let mut items = Vec::new();
    extraction::flatten(group, &mut items);
    let source: BTreeMap<i64, &StoredSourceItem> =
        items.iter().map(|i| (i.source.item_id, *i)).collect();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut prior: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    {
        let mut st = tx
            .prepare("SELECT source_item_id,project_id FROM mentions WHERE root_item_id=?1")
            .map_err(|e| e.to_string())?;
        for row in st
            .query_map([root], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?
        {
            let (sid, pid) = row.map_err(|e| e.to_string())?;
            prior.entry(sid).or_default().push(pid);
        }
    }
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    for m in outputs {
        *counts.entry(m.source_item_id).or_default() += 1;
    }
    tx.execute("DELETE FROM mentions WHERE root_item_id=?1", [root])
        .map_err(|e| e.to_string())?;
    let mut seen = BTreeSet::new();
    for mention in outputs {
        let key = if counts[&mention.source_item_id] == 1 {
            format!("{}:sole", mention.source_item_id)
        } else {
            mention_key(mention)
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        let old = prior
            .get(&mention.source_item_id)
            .filter(|p| p.len() == 1)
            .map(|p| p[0]);
        save_mention(&tx, root, mention, &key, &source, old)?;
    }
    tx.execute("INSERT INTO processing_groups(root_item_id,thread_id,fingerprint,prompt_version,schema_version,model,status,run_id,item_count,processed_item_count,error) VALUES(?1,?2,?3,?4,?5,?6,'complete',?7,?8,?8,NULL) ON CONFLICT(root_item_id) DO UPDATE SET fingerprint=excluded.fingerprint,prompt_version=excluded.prompt_version,schema_version=excluded.schema_version,model=excluded.model,status='complete',run_id=excluded.run_id,item_count=excluded.item_count,processed_item_count=excluded.processed_item_count,error=NULL",params![root,thread_id,fp,extraction::PROMPT_VERSION,extraction::SCHEMA_VERSION,model.name(),run_id,source.len() as i64]).map_err(|e|e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}
fn mention_key(m: &Mention) -> String {
    let basis = m
        .urls
        .first()
        .map(|x| x.to_lowercase())
        .or_else(|| m.name.as_ref().map(|x| x.to_lowercase()))
        .unwrap_or_else(|| format!("unnamed:{}", &hash(&m.description)[..12]));
    format!("{}:{}", m.source_item_id, basis)
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn identity_key(m: &Mention, builder: Option<&String>) -> String {
    // Cross-conversation reuse requires both an exact URL and supported builder identity.
    if let (Some(url), Some(builder)) = (m.urls.first(), builder) {
        return format!("url:{}:{}", builder.to_lowercase(), url.to_lowercase());
    }
    mention_key(m)
}
fn save_mention(
    tx: &rusqlite::Transaction<'_>,
    root: i64,
    m: &Mention,
    key: &str,
    source: &BTreeMap<i64, &StoredSourceItem>,
    old: Option<i64>,
) -> Result<(), String> {
    // The builder is the source item's author, and only when the model found
    // that author claiming the project; commenters are never assumed to be builders.
    let builder = m
        .author_is_builder
        .then(|| source[&m.source_item_id].source.author.clone())
        .flatten();
    let identity = identity_key(m, builder.as_ref());
    let slug_base = m.name.as_deref().unwrap_or("project");
    let slug = format!("{}-{}", slugify(slug_base), &hash(&identity)[..10]);
    let pid = if let Some(id) = old {
        id
    } else if let Some(mapped) = tx
        .query_row(
            "SELECT project_id FROM project_matches WHERE identity_key=?1",
            [&identity],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
    {
        mapped
    } else {
        tx.execute(
            "INSERT OR IGNORE INTO projects(identity_key,slug,created_at) VALUES(?1,?2,?3)",
            params![identity, slug, unix_time()],
        )
        .map_err(|e| e.to_string())?;
        tx.query_row(
            "SELECT id FROM projects WHERE identity_key=?1",
            [&identity],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?
    };
    let sf = &source[&m.source_item_id].content_fingerprint;
    let review = "pending";
    tx.execute("INSERT INTO mentions(project_id,root_item_id,source_item_id,mention_key,name,description,problem,audience,builder_username,review_state,source_fingerprint) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![pid,root,m.source_item_id,key,m.name,m.description,m.problem,m.audience,builder,review,sf]).map_err(|e|e.to_string())?;
    let mid = tx.last_insert_rowid();
    for url in &m.urls {
        tx.execute(
            "INSERT OR IGNORE INTO mention_links VALUES(?1,?2)",
            params![mid, url],
        )
        .map_err(|e| e.to_string())?;
    }
    for label in &m.categories {
        tx.execute(
            "INSERT INTO mention_labels VALUES(?1,'category',?2)",
            params![mid, label.slug()],
        )
        .map_err(|e| e.to_string())?;
    }
    for label in &m.audience_labels {
        if let Some(value) = normalize("audience", label) {
            tx.execute(
                "INSERT OR IGNORE INTO mention_labels VALUES(?1,'audience',?2)",
                params![mid, value],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    for request in &m.requests {
        tx.execute(
            "INSERT OR IGNORE INTO mention_requests VALUES(?1,?2,?3,?4)",
            params![
                mid,
                request.kind.as_str(),
                request.source_item_id,
                request.evidence
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    for support in &m.supporting {
        tx.execute(
            "INSERT OR IGNORE INTO supporting_contributions VALUES(?1,?2,?3,?4)",
            params![
                mid,
                support.source_item_id,
                support.summary,
                support.excerpt
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c)
        } else if !out.ends_with('-') {
            out.push('-')
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "project".into()
    } else {
        trimmed.to_string()
    }
}
fn normalize(kind: &str, s: &str) -> Option<String> {
    let key = s.trim().to_lowercase();
    if key.is_empty() {
        return None;
    }
    let value = match (kind, key.as_str()) {
        ("audience", "developers" | "software developers") => "Developers",
        ("audience", "students" | "learners") => "Students",
        _ => return Some(s.trim().to_string()),
    };
    Some(value.into())
}
fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod rig_configuration_tests {
    use super::{Model, Provider, RigModel};
    use std::collections::HashMap;

    fn configured(values: &[(&str, &str)]) -> Result<RigModel, String> {
        let values: HashMap<_, _> = values.iter().copied().collect();
        RigModel::from_lookup(|key| values.get(key).map(|value| (*value).to_owned()))
    }

    #[test]
    fn openai_remains_default_and_requires_explicit_model() {
        assert_eq!(
            configured(&[("OPENAI_API_KEY", "test")]).err().as_deref(),
            Some("WRKGN_MODEL is required")
        );
        let model = configured(&[("OPENAI_API_KEY", "test"), ("WRKGN_MODEL", "gpt-test")]).unwrap();
        assert_eq!(model.provider, Provider::OpenAi);
        assert_eq!(model.model, "gpt-test");
        assert!(model.name().starts_with("openai/gpt-test@"));
    }

    #[test]
    fn neuralwatt_defaults_and_provider_identity() {
        let model = configured(&[
            ("WRKGN_PROVIDER", "neuralwatt"),
            ("NEURALWATT_API_KEY", "test"),
        ])
        .unwrap();
        assert_eq!(model.provider, Provider::Neuralwatt);
        assert_eq!(model.model, "glm-5.3-flash");
        assert_eq!(model.base_url, "https://api.neuralwatt.com/v1");
        assert!(model.name().starts_with("neuralwatt/glm-5.3-flash@"));
        let other_endpoint = configured(&[
            ("WRKGN_PROVIDER", "neuralwatt"),
            ("NEURALWATT_API_KEY", "test"),
            ("NEURALWATT_BASE_URL", "https://example.invalid/v1"),
        ])
        .unwrap();
        assert_ne!(model.name(), other_endpoint.name());
        assert!(!other_endpoint.name().contains("example.invalid"));
    }

    #[test]
    fn provider_specific_credentials_and_invalid_provider() {
        assert_eq!(
            configured(&[("WRKGN_PROVIDER", "neuralwatt"), ("OPENAI_API_KEY", "test")])
                .err()
                .as_deref(),
            Some("NEURALWATT_API_KEY is required")
        );
        assert_eq!(
            configured(&[("WRKGN_PROVIDER", "other")]).err().as_deref(),
            Some("WRKGN_PROVIDER must be openai or neuralwatt")
        );
    }
}
