use crate::extraction::{self, Extraction, Mention};
use crate::store::{ConversationNode, Store, StoredSourceItem};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub trait Model {
    fn name(&self) -> &str;
    fn extract(&self, prompt: &str) -> Result<Extraction, String>;
}
pub struct RigModel {
    name: String,
}
impl RigModel {
    pub fn from_env() -> Result<Self, String> {
        let name =
            std::env::var("WRKGN_MODEL").map_err(|_| "WRKGN_MODEL is required".to_string())?;
        if std::env::var_os("OPENAI_API_KEY").is_none() {
            return Err("OPENAI_API_KEY is required".into());
        }
        Ok(Self { name })
    }
}
impl Model for RigModel {
    fn name(&self) -> &str {
        &self.name
    }
    fn extract(&self, prompt: &str) -> Result<Extraction, String> {
        use rig_agent::extractor::ExtractorBuilder;
        use rig_core::providers::openai::OpenAI;
        let client = OpenAI::from_env().map_err(|e| e.to_string())?;
        let model = client.completion(self.name.as_str());
        let extractor = ExtractorBuilder::<Extraction>::new(model)
            .append_preamble(extraction::SYSTEM_PROMPT)
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
    pub processed: usize,
    pub skipped: usize,
    pub failed: Vec<(i64, String)>,
}
pub fn process_thread<M: Model>(
    store: &mut Store,
    thread_id: i64,
    model: &M,
    reprocess: bool,
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
    let mut report = Report::default();
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
        match process_group(conn, run_id, thread_id, group, model, &fp) {
            Ok(()) => report.processed += 1,
            Err(e) => {
                let mut all = Vec::new();
                extraction::flatten(group, &mut all);
                conn.execute("INSERT INTO processing_failures(run_id,root_item_id,item_count,processed_item_count,error) VALUES(?1,?2,?3,0,?4)",params![run_id,root,all.len() as i64,e]).map_err(|x|x.to_string())?;
                report.failed.push((root, e));
            }
        }
    }
    let status = if report.failed.is_empty() {
        "complete"
    } else {
        "incomplete"
    };
    conn.execute(
        "UPDATE processing_runs SET status=?1,finished_at=?2,error=?3 WHERE id=?4",
        params![
            status,
            unix_time(),
            if report.failed.is_empty() {
                None
            } else {
                Some(format!("{} group(s) failed", report.failed.len()))
            },
            run_id
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(report)
}
fn process_group<M: Model>(
    conn: &mut rusqlite::Connection,
    run_id: i64,
    thread_id: i64,
    group: &ConversationNode,
    model: &M,
    fp: &str,
) -> Result<(), String> {
    let root = group.item.source.item_id;
    let mut items = Vec::new();
    extraction::flatten(group, &mut items);
    let source: BTreeMap<i64, &StoredSourceItem> =
        items.iter().map(|i| (i.source.item_id, *i)).collect();
    let batches = extraction::batches(group)?;
    let mut outputs = Vec::new();
    let mut covered: BTreeSet<i64> = BTreeSet::new();
    for batch in &batches {
        let out = model.extract(&batch.prompt)?;
        extraction::validate(&out, &batch.ids, &source)?;
        covered.extend(&batch.ids);
        outputs.extend(out.mentions);
    }
    if covered.len() != source.len() {
        return Err("not every source item was included in extraction batches".into());
    }
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
    for m in &outputs {
        *counts.entry(m.source_item_id).or_default() += 1;
    }
    tx.execute("DELETE FROM mentions WHERE root_item_id=?1", [root])
        .map_err(|e| e.to_string())?;
    let mut seen = BTreeSet::new();
    for mention in &outputs {
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
fn identity_key(m: &Mention) -> String {
    // Cross-conversation reuse requires both an exact URL and supported builder identity.
    if let (Some(url), Some(builder)) = (m.urls.first(), m.builder_username.as_ref()) {
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
    let identity = identity_key(m);
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
    tx.execute("INSERT INTO mentions(project_id,root_item_id,source_item_id,mention_key,name,description,problem,audience,builder_username,review_state,source_fingerprint) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![pid,root,m.source_item_id,key,m.name,m.description,m.problem,m.audience,m.builder_username,review,sf]).map_err(|e|e.to_string())?;
    let mid = tx.last_insert_rowid();
    for url in &m.urls {
        tx.execute(
            "INSERT OR IGNORE INTO mention_links VALUES(?1,?2)",
            params![mid, url],
        )
        .map_err(|e| e.to_string())?;
    }
    for label in &m.categories {
        if let Some(value) = normalize("category", label) {
            tx.execute(
                "INSERT OR IGNORE INTO mention_labels VALUES(?1,'category',?2)",
                params![mid, value],
            )
            .map_err(|e| e.to_string())?;
        }
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
            params![mid, request.kind, request.source_item_id, request.evidence],
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
        ("category", "dev tools" | "developer tool" | "developer tools") => "Developer tools",
        ("category", "productivity" | "productivity tools") => "Productivity",
        ("category", "education" | "learning") => "Education",
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
