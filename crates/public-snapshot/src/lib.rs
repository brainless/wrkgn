//! Immutable, allowlisted public SQLite snapshots shared by exporter and server.
//! This crate deliberately has no dependency on the working pipeline.
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 6;
pub const DATABASE_FILE: &str = "public.sqlite";
pub const VERSION_PREFIX: &[u8] = b"public-schema-6";
pub type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    pub id: i64,
    pub slug: String,
    pub name: Option<String>,
    pub description: String,
    pub problem: Option<String>,
    pub audience: Option<String>,
    pub builder_usernames: Vec<String>,
    pub categories: Vec<String>,
    pub audience_labels: Vec<String>,
    pub latest_shared_at: i64,
    pub comment_count: i64,
    pub searchable_text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Detail {
    pub id: i64,
    pub slug: String,
    pub name: Option<String>,
    pub mentions: Vec<Mention>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mention {
    pub name: Option<String>,
    pub description: String,
    pub problem: Option<String>,
    pub audience: Option<String>,
    pub source_item_id: i64,
    pub source_url: String,
    pub author: Option<String>,
    pub shared_at: i64,
    pub source_excerpt: String,
    pub builder_usernames: Vec<String>,
    pub links: Vec<String>,
    pub evidence: Vec<Evidence>,
}
/// Evidence support mappings and authorization dependencies are private to the pipeline.
/// Deserializing schema-6 JSON intentionally discards its legacy `supports` member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub source_item_id: i64,
    pub source_field: String,
    pub value: String,
    pub author: Option<String>,
    pub shared_at: i64,
    pub source_url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Taxonomy {
    pub slug: String,
    pub label: String,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub schema_version: u32,
    pub version: String,
    pub exported_at: i64,
    pub categories: Vec<Taxonomy>,
    pub audiences: Vec<Taxonomy>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pointer {
    pub schema_version: u32,
    pub version: String,
    pub snapshot: String,
}
#[derive(Debug, Clone)]
pub struct Selection {
    pub database: PathBuf,
    pub version: Option<String>,
}

fn sql<T>(value: rusqlite::Result<T>) -> Result<T> {
    value.map_err(|e| e.to_string())
}
fn json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err("snapshot paths must be regular files".into());
    }
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
pub fn valid_version(value: &str) -> bool {
    value.len() == 16
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}
pub fn web_url(value: &str) -> bool {
    url::Url::parse(value)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}
/// Resolve a completed export root or a single database. No arbitrary pointer paths.
pub fn select(path: &Path) -> Result<Selection> {
    if path.is_file() {
        return Ok(Selection {
            database: path.to_owned(),
            version: None,
        });
    }
    let pointer: Pointer = json(&path.join("current.json"))?;
    if pointer.schema_version != SCHEMA_VERSION
        || !valid_version(&pointer.version)
        || pointer.snapshot != format!("snapshots/{}/manifest.json", pointer.version)
    {
        return Err("unsupported schema/version or unsafe snapshot pointer".into());
    }
    let manifest: serde_json::Value = json(&path.join(&pointer.snapshot))?;
    if manifest["schema_version"].as_u64() != Some(SCHEMA_VERSION.into())
        || manifest["version"].as_str() != Some(&pointer.version)
        || manifest["database"].as_str() != Some(DATABASE_FILE)
    {
        return Err("mismatched snapshot manifest".into());
    }
    Ok(Selection {
        database: path
            .join("snapshots")
            .join(&pointer.version)
            .join(DATABASE_FILE),
        version: Some(pointer.version),
    })
}
/// Open only an immutable snapshot; never creates files, journals, or WALs.
pub fn open_read_only(path: &Path) -> Result<Connection> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err("snapshot database must be a regular file".into());
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        if PathBuf::from(format!("{}{suffix}", path.display())).exists() {
            return Err("snapshot must be finalized without journal/WAL sidecars".into());
        }
    }
    let conn = sql(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ))?;
    sql(conn.execute_batch(
        "PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON;",
    ))?;
    Ok(conn)
}

const SCHEMA: &str = r#"
PRAGMA foreign_keys=ON;
PRAGMA journal_mode=DELETE;
CREATE TABLE metadata (schema_version INTEGER NOT NULL, version TEXT NOT NULL, exported_at INTEGER NOT NULL) STRICT;
CREATE TABLE taxonomy (kind TEXT NOT NULL, slug TEXT NOT NULL, label TEXT NOT NULL, description TEXT NOT NULL, PRIMARY KEY(kind,slug)) STRICT;
CREATE TABLE projects (id INTEGER PRIMARY KEY, slug TEXT NOT NULL UNIQUE, name TEXT, description TEXT NOT NULL, problem TEXT, audience TEXT, latest_shared_at INTEGER NOT NULL, comment_count INTEGER NOT NULL, searchable_text TEXT NOT NULL) STRICT;
CREATE TABLE project_builders (project_id INTEGER NOT NULL REFERENCES projects(id), username TEXT NOT NULL, PRIMARY KEY(project_id,username)) STRICT;
CREATE TABLE project_labels (project_id INTEGER NOT NULL REFERENCES projects(id), kind TEXT NOT NULL, slug TEXT NOT NULL, PRIMARY KEY(project_id,kind,slug), FOREIGN KEY(kind,slug) REFERENCES taxonomy(kind,slug)) STRICT;
CREATE TABLE mentions (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id), position INTEGER NOT NULL, name TEXT, description TEXT NOT NULL, problem TEXT, audience TEXT, source_item_id INTEGER NOT NULL, source_url TEXT NOT NULL, author TEXT, shared_at INTEGER NOT NULL, source_excerpt TEXT NOT NULL, UNIQUE(project_id,position)) STRICT;
CREATE TABLE mention_builders (mention_id INTEGER NOT NULL REFERENCES mentions(id), position INTEGER NOT NULL, username TEXT NOT NULL, PRIMARY KEY(mention_id,position)) STRICT;
CREATE TABLE mention_links (mention_id INTEGER NOT NULL REFERENCES mentions(id), position INTEGER NOT NULL, url TEXT NOT NULL, PRIMARY KEY(mention_id,position)) STRICT;
CREATE TABLE evidence (mention_id INTEGER NOT NULL REFERENCES mentions(id), position INTEGER NOT NULL, source_item_id INTEGER NOT NULL, source_field TEXT NOT NULL, value TEXT NOT NULL, author TEXT, shared_at INTEGER NOT NULL, source_url TEXT NOT NULL, PRIMARY KEY(mention_id,position)) STRICT;
CREATE INDEX projects_discovery ON projects(latest_shared_at DESC,id);
CREATE INDEX builders_username ON project_builders(username,project_id);
CREATE INDEX labels_lookup ON project_labels(kind,slug,project_id);
CREATE INDEX mentions_project ON mentions(project_id,position);
"#;
const TABLES: &[(&str, &[&str])] = &[
    ("metadata", &["schema_version", "version", "exported_at"]),
    ("taxonomy", &["kind", "slug", "label", "description"]),
    (
        "projects",
        &[
            "id",
            "slug",
            "name",
            "description",
            "problem",
            "audience",
            "latest_shared_at",
            "comment_count",
            "searchable_text",
        ],
    ),
    ("project_builders", &["project_id", "username"]),
    ("project_labels", &["project_id", "kind", "slug"]),
    (
        "mentions",
        &[
            "id",
            "project_id",
            "position",
            "name",
            "description",
            "problem",
            "audience",
            "source_item_id",
            "source_url",
            "author",
            "shared_at",
            "source_excerpt",
        ],
    ),
    ("mention_builders", &["mention_id", "position", "username"]),
    ("mention_links", &["mention_id", "position", "url"]),
    (
        "evidence",
        &[
            "mention_id",
            "position",
            "source_item_id",
            "source_field",
            "value",
            "author",
            "shared_at",
            "source_url",
        ],
    ),
];
const INDEXES: &[(&str, &str, &[&str])] = &[
    (
        "projects_discovery",
        "projects",
        &["latest_shared_at", "id"],
    ),
    (
        "builders_username",
        "project_builders",
        &["username", "project_id"],
    ),
    (
        "labels_lookup",
        "project_labels",
        &["kind", "slug", "project_id"],
    ),
    ("mentions_project", "mentions", &["project_id", "position"]),
];

/// Write/finalize inside the caller's unpublished staging directory. The caller
/// publishes its directory and current.json only after this succeeds.
pub fn write_database(
    path: &Path,
    metadata: &Metadata,
    discovery: &[Discovery],
    details: &[Detail],
) -> Result<()> {
    if path.exists() {
        return Err("refusing to overwrite a completed public database".into());
    }
    let temporary = path.with_extension("sqlite.tmp");
    // A stale file from an interrupted export is never selected.
    if temporary.exists() {
        fs::remove_file(&temporary).map_err(|e| e.to_string())?;
    }
    let mut conn = sql(Connection::open(&temporary))?;
    sql(conn.execute_batch(SCHEMA))?;
    sql(conn.pragma_update(None, "user_version", SCHEMA_VERSION))?;
    let tx = sql(conn.transaction())?;
    sql(tx.execute(
        "INSERT INTO metadata VALUES(?1,?2,?3)",
        params![
            metadata.schema_version,
            metadata.version,
            metadata.exported_at
        ],
    ))?;
    for (kind, taxonomy) in [
        ("category", &metadata.categories),
        ("audience", &metadata.audiences),
    ] {
        for item in taxonomy {
            sql(tx.execute(
                "INSERT INTO taxonomy VALUES(?1,?2,?3,?4)",
                params![kind, item.slug, item.label, item.description],
            ))?;
        }
    }
    for p in discovery {
        sql(tx.execute(
            "INSERT INTO projects VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                p.id,
                p.slug,
                p.name,
                p.description,
                p.problem,
                p.audience,
                p.latest_shared_at,
                p.comment_count,
                p.searchable_text
            ],
        ))?;
        for username in &p.builder_usernames {
            sql(tx.execute(
                "INSERT INTO project_builders VALUES(?1,?2)",
                params![p.id, username],
            ))?;
        }
        for (kind, labels) in [
            ("category", &p.categories),
            ("audience", &p.audience_labels),
        ] {
            for slug in labels {
                sql(tx.execute(
                    "INSERT INTO project_labels VALUES(?1,?2,?3)",
                    params![p.id, kind, slug],
                ))?;
            }
        }
    }
    if discovery.len() != details.len() {
        return Err("discovery/detail count mismatch".into());
    }
    let by_id: BTreeMap<i64, &Discovery> = discovery.iter().map(|p| (p.id, p)).collect();
    let mut mention_id = 0i64;
    let mut seen = BTreeSet::new();
    for d in details {
        let p = by_id.get(&d.id).ok_or("detail not in discovery")?;
        if !seen.insert(d.id) || p.slug != d.slug || p.name != d.name {
            return Err("detail/discovery identity mismatch".into());
        }
        for (position, m) in d.mentions.iter().enumerate() {
            mention_id += 1;
            sql(tx.execute(
                "INSERT INTO mentions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![
                    mention_id,
                    d.id,
                    position as i64,
                    m.name,
                    m.description,
                    m.problem,
                    m.audience,
                    m.source_item_id,
                    m.source_url,
                    m.author,
                    m.shared_at,
                    m.source_excerpt
                ],
            ))?;
            for (i, b) in m.builder_usernames.iter().enumerate() {
                sql(tx.execute(
                    "INSERT INTO mention_builders VALUES(?1,?2,?3)",
                    params![mention_id, i as i64, b],
                ))?;
            }
            for (i, url) in m.links.iter().enumerate() {
                sql(tx.execute(
                    "INSERT INTO mention_links VALUES(?1,?2,?3)",
                    params![mention_id, i as i64, url],
                ))?;
            }
            for (i, e) in m.evidence.iter().enumerate() {
                sql(tx.execute(
                    "INSERT INTO evidence VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        mention_id,
                        i as i64,
                        e.source_item_id,
                        e.source_field,
                        e.value,
                        e.author,
                        e.shared_at,
                        e.source_url
                    ],
                ))?;
            }
        }
    }
    sql(tx.commit())?;
    sql(conn.execute_batch("VACUUM;"))?;
    validate_connection(&conn, Some(&metadata.version))?;
    sql(conn.close().map_err(|(_, e)| e))?;
    // DELETE journal mode leaves a single self-contained file.
    for suffix in ["-wal", "-shm", "-journal"] {
        if PathBuf::from(format!("{}{suffix}", temporary.display())).exists() {
            return Err("public database has unfinished sidecar files".into());
        }
    }
    fs::rename(temporary, path).map_err(|e| e.to_string())
}

/// Validate and open a selected snapshot. Call on a blocking thread.
pub fn open_validated(path: &Path) -> Result<(Connection, Metadata)> {
    let selection = select(path)?;
    let conn = open_read_only(&selection.database)?;
    let metadata = validate_connection(&conn, selection.version.as_deref())?;
    Ok((conn, metadata))
}

pub fn metadata(conn: &Connection) -> Result<Metadata> {
    let (schema_version, version, exported_at) = sql(conn.query_row(
        "SELECT schema_version,version,exported_at FROM metadata",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ))?;
    let taxonomy = |kind| -> Result<Vec<Taxonomy>> {
        let mut st = sql(
            conn.prepare("SELECT slug,label,description FROM taxonomy WHERE kind=?1 ORDER BY slug")
        )?;
        sql(sql(st.query_map([kind], |r| {
            Ok(Taxonomy {
                slug: r.get(0)?,
                label: r.get(1)?,
                description: r.get(2)?,
            })
        }))?
        .collect())
    };
    Ok(Metadata {
        schema_version,
        version,
        exported_at,
        categories: taxonomy("category")?,
        audiences: taxonomy("audience")?,
    })
}

fn strings(conn: &Connection, query: &str, id: i64) -> Result<Vec<String>> {
    let mut st = sql(conn.prepare(query))?;
    sql(sql(st.query_map([id], |r| r.get(0)))?.collect())
}
pub fn discovery(conn: &Connection) -> Result<Vec<Discovery>> {
    let mut st=sql(conn.prepare("SELECT id,slug,name,description,problem,audience,latest_shared_at,comment_count,searchable_text FROM projects ORDER BY latest_shared_at DESC,id"))?;
    let mut projects: Vec<Discovery> = sql(sql(st.query_map([], |r| {
        Ok(Discovery {
            id: r.get(0)?,
            slug: r.get(1)?,
            name: r.get(2)?,
            description: r.get(3)?,
            problem: r.get(4)?,
            audience: r.get(5)?,
            latest_shared_at: r.get(6)?,
            comment_count: r.get(7)?,
            searchable_text: r.get(8)?,
            builder_usernames: vec![],
            categories: vec![],
            audience_labels: vec![],
        })
    }))?
    .collect())?;
    // Load relationships in three linear passes, avoiding per-project SQL calls.
    let positions: BTreeMap<i64, usize> = projects
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id, i))
        .collect();
    let mut st =
        sql(conn.prepare("SELECT project_id,username FROM project_builders ORDER BY username"))?;
    for row in sql(st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))))? {
        let (id, value) = sql(row)?;
        let i = *positions.get(&id).ok_or("orphan project builder")?;
        projects[i].builder_usernames.push(value);
    }
    let mut st =
        sql(conn.prepare("SELECT project_id,kind,slug FROM project_labels ORDER BY slug"))?;
    for row in sql(st.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    }))? {
        let (id, kind, value) = sql(row)?;
        let i = *positions.get(&id).ok_or("orphan project label")?;
        match kind.as_str() {
            "category" => projects[i].categories.push(value),
            "audience" => projects[i].audience_labels.push(value),
            _ => return Err("unknown label kind".into()),
        }
    }
    Ok(projects)
}
pub fn detail(conn: &Connection, slug: &str) -> Result<Option<Detail>> {
    let identity = sql(conn
        .query_row(
            "SELECT id,slug,name FROM projects WHERE slug=?1",
            [slug],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional())?;
    let Some((id, slug, name)) = identity else {
        return Ok(None);
    };
    let mut st=sql(conn.prepare("SELECT id,name,description,problem,audience,source_item_id,source_url,author,shared_at,source_excerpt FROM mentions WHERE project_id=?1 ORDER BY position"))?;
    let rows = sql(sql(st.query_map([id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            Mention {
                name: r.get(1)?,
                description: r.get(2)?,
                problem: r.get(3)?,
                audience: r.get(4)?,
                source_item_id: r.get(5)?,
                source_url: r.get(6)?,
                author: r.get(7)?,
                shared_at: r.get(8)?,
                source_excerpt: r.get(9)?,
                builder_usernames: vec![],
                links: vec![],
                evidence: vec![],
            },
        ))
    }))?
    .collect::<rusqlite::Result<Vec<_>>>())?;
    let mut mentions = Vec::with_capacity(rows.len());
    for (mid, mut m) in rows {
        m.builder_usernames = strings(
            conn,
            "SELECT username FROM mention_builders WHERE mention_id=?1 ORDER BY position",
            mid,
        )?;
        m.links = strings(
            conn,
            "SELECT url FROM mention_links WHERE mention_id=?1 ORDER BY position",
            mid,
        )?;
        let mut st=sql(conn.prepare("SELECT source_item_id,source_field,value,author,shared_at,source_url FROM evidence WHERE mention_id=?1 ORDER BY position"))?;
        m.evidence = sql(sql(st.query_map([mid], |r| {
            Ok(Evidence {
                source_item_id: r.get(0)?,
                source_field: r.get(1)?,
                value: r.get(2)?,
                author: r.get(3)?,
                shared_at: r.get(4)?,
                source_url: r.get(5)?,
            })
        }))?
        .collect())?;
        mentions.push(m);
    }
    Ok(Some(Detail {
        id,
        slug,
        name,
        mentions,
    }))
}
/// Related projects in discovery order, excluding the current project.
pub fn same_builder_projects(
    conn: &Connection,
    project_id: i64,
) -> Result<Vec<(i64, String, Option<String>)>> {
    let mut st=sql(conn.prepare("SELECT p.id,p.slug,p.name FROM projects p WHERE p.id<>?1 AND EXISTS(SELECT 1 FROM project_builders b JOIN project_builders own ON b.username=own.username WHERE b.project_id=p.id AND own.project_id=?1) ORDER BY p.latest_shared_at DESC,p.id"))?;
    sql(sql(st.query_map([project_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))))?.collect())
}
pub fn project_slugs(conn: &Connection) -> Result<Vec<String>> {
    strings(
        conn,
        "SELECT slug FROM projects WHERE id>=?1 ORDER BY latest_shared_at DESC,id",
        0,
    )
}

/// Strict publication boundary validation, shared by startup, reload, and `check`.
pub fn validate_connection(conn: &Connection, expected_version: Option<&str>) -> Result<Metadata> {
    let integrity: String = sql(conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)))?;
    if integrity != "ok" {
        return Err(format!("snapshot integrity check failed: {integrity}"));
    }
    let version: u32 = sql(conn.pragma_query_value(None, "user_version", |r| r.get(0)))?;
    if version != SCHEMA_VERSION {
        return Err("unsupported SQLite public schema".into());
    }
    let mut st = sql(conn.prepare("SELECT type,name,tbl_name FROM sqlite_schema ORDER BY name"))?;
    let objects = sql(sql(st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    }))?
    .collect::<rusqlite::Result<Vec<_>>>())?;
    let mut tables = BTreeSet::new();
    for (kind, name, table) in &objects {
        match kind.as_str() {
            "table" if TABLES.iter().any(|(allowed, _)| name == allowed) => {
                tables.insert(name.as_str());
            }
            "index"
                if name.starts_with("sqlite_autoindex_")
                    && TABLES.iter().any(|(allowed, _)| table == allowed) => {}
            "index"
                if INDEXES
                    .iter()
                    .any(|(allowed, t, _)| name == allowed && table == t) => {}
            _ => return Err(format!("non-public database object {kind} {name}")),
        }
    }
    if tables.len() != TABLES.len() {
        return Err("missing public snapshot tables".into());
    }
    for (table, columns) in TABLES {
        let mut st = sql(conn.prepare(&format!("PRAGMA table_xinfo('{table}')")))?;
        let found: Vec<String> = sql(sql(st.query_map([], |r| r.get(1)))?.collect())?;
        if found.iter().map(String::as_str).collect::<Vec<_>>() != *columns {
            return Err(format!("non-public/missing columns in {table}"));
        }
    }
    for (name, table, columns) in INDEXES {
        if !objects
            .iter()
            .any(|(kind, n, t)| kind == "index" && n == name && t == table)
        {
            return Err(format!("missing snapshot index {name}"));
        }
        let mut st = sql(conn.prepare(&format!("PRAGMA index_info('{name}')")))?;
        let found: Vec<String> = sql(sql(st.query_map([], |r| r.get(2)))?.collect())?;
        if found.iter().map(String::as_str).collect::<Vec<_>>() != *columns {
            return Err(format!("invalid index {name}"));
        }
    }
    let foreign_key_error = sql(conn.prepare("PRAGMA foreign_key_check"))?
        .exists([])
        .map_err(|e| e.to_string())?;
    if foreign_key_error {
        return Err("snapshot contains broken foreign keys".into());
    }
    let count: i64 = sql(conn.query_row("SELECT count(*) FROM metadata", [], |r| r.get(0)))?;
    if count != 1 {
        return Err("expected exactly one metadata record".into());
    }
    let metadata = metadata(conn)?;
    if metadata.schema_version != SCHEMA_VERSION
        || !valid_version(&metadata.version)
        || metadata.exported_at < 0
        || expected_version.is_some_and(|expected| expected != metadata.version)
    {
        return Err("snapshot metadata/version mismatch".into());
    }
    validate_taxonomies(&metadata)?;
    let invalid_queries = [
        "SELECT EXISTS(SELECT 1 FROM taxonomy WHERE kind NOT IN ('category','audience') OR trim(label)='' OR trim(description)='')",
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id<=0 OR latest_shared_at<0 OR comment_count<0 OR trim(description)='')",
        "SELECT EXISTS(SELECT id FROM projects GROUP BY id HAVING count(*)>1)",
        "SELECT EXISTS(SELECT slug FROM projects GROUP BY slug HAVING count(*)>1)",
        "SELECT EXISTS(SELECT project_id FROM project_builders GROUP BY project_id,username HAVING count(*)>1)",
        "SELECT EXISTS(SELECT project_id FROM project_labels GROUP BY project_id,kind,slug HAVING count(*)>1)",
        "SELECT EXISTS(SELECT 1 FROM project_labels l LEFT JOIN taxonomy t ON t.kind=l.kind AND t.slug=l.slug WHERE t.slug IS NULL)",
        "SELECT EXISTS(SELECT 1 FROM mentions WHERE id<=0 OR position<0 OR shared_at<0 OR source_item_id<=0 OR trim(description)='')",
        "SELECT EXISTS(SELECT id FROM mentions GROUP BY id HAVING count(*)>1)",
        "SELECT EXISTS(SELECT project_id FROM mentions GROUP BY project_id,position HAVING count(*)>1)",
        "SELECT EXISTS(SELECT 1 FROM projects p WHERE NOT EXISTS(SELECT 1 FROM mentions m WHERE m.project_id=p.id AND m.position=0 AND m.shared_at=p.latest_shared_at AND m.name IS p.name AND m.description=p.description AND m.problem IS p.problem AND m.audience IS p.audience))",
        "SELECT EXISTS(SELECT 1 FROM mentions m JOIN mentions next ON next.project_id=m.project_id AND next.position=m.position+1 WHERE m.shared_at<next.shared_at)",
        "SELECT EXISTS(SELECT 1 FROM mentions m WHERE NOT EXISTS(SELECT 1 FROM evidence e WHERE e.mention_id=m.id))",
        "SELECT EXISTS(SELECT 1 FROM evidence WHERE position<0 OR source_item_id<=0 OR shared_at<0 OR source_field NOT IN ('title','url','body') OR trim(value)='')",
        "SELECT EXISTS(SELECT 1 FROM project_builders WHERE trim(username)='')",
        "SELECT EXISTS(SELECT 1 FROM mention_builders WHERE position<0 OR trim(username)='')",
        "SELECT EXISTS(SELECT 1 FROM mention_links WHERE position<0)",
        "SELECT EXISTS(SELECT 1 FROM project_builders b LEFT JOIN projects p ON p.id=b.project_id WHERE p.id IS NULL)",
        "SELECT EXISTS(SELECT 1 FROM project_labels b LEFT JOIN projects p ON p.id=b.project_id WHERE p.id IS NULL)",
        "SELECT EXISTS(SELECT 1 FROM mentions m LEFT JOIN projects p ON p.id=m.project_id WHERE p.id IS NULL)",
        "SELECT EXISTS(SELECT 1 FROM evidence e LEFT JOIN mentions m ON m.id=e.mention_id WHERE m.id IS NULL)",
        "SELECT EXISTS(SELECT 1 FROM mention_links e LEFT JOIN mentions m ON m.id=e.mention_id WHERE m.id IS NULL)",
        "SELECT EXISTS(SELECT 1 FROM mention_builders e LEFT JOIN mentions m ON m.id=e.mention_id WHERE m.id IS NULL)",
    ];
    for query in invalid_queries {
        if sql(conn.query_row(query, [], |r| r.get::<_, bool>(0)))? {
            return Err(format!("invalid public snapshot content ({query})"));
        }
    }
    for slug in project_slugs(conn)? {
        if !valid_slug(&slug) {
            return Err("unsafe project slug".into());
        }
    }
    let mut st=sql(conn.prepare("SELECT url FROM mention_links UNION ALL SELECT source_url FROM mentions UNION ALL SELECT source_url FROM evidence UNION ALL SELECT value FROM evidence WHERE source_field='url'"))?;
    for row in sql(st.query_map([], |r| r.get::<_, String>(0)))? {
        if !web_url(&sql(row)?) {
            return Err("snapshot links must use http(s)".into());
        }
    }
    // Match HN source URLs to their IDs; URLs are never inferred from text.
    let mut st=sql(conn.prepare("SELECT source_item_id,source_url FROM mentions UNION ALL SELECT source_item_id,source_url FROM evidence"))?;
    for row in sql(st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))))? {
        let (id, value) = sql(row)?;
        let url = url::Url::parse(&value).map_err(|e| e.to_string())?;
        if url.origin().ascii_serialization() != "https://news.ycombinator.com"
            || url.path() != "/item"
            || !url
                .query_pairs()
                .any(|(k, v)| k == "id" && v == id.to_string())
        {
            return Err("source URL differs from HN item ID".into());
        }
    }
    Ok(metadata)
}

fn validate_taxonomies(metadata: &Metadata) -> Result<()> {
    let expected: BTreeSet<&str> = [
        "ai-automation",
        "developer-tools",
        "productivity",
        "education",
        "games",
        "creative-media",
        "data-analytics",
        "business-finance",
        "community-civic",
        "hardware-diy",
        "infrastructure-hosting",
        "security-privacy",
        "science-research",
        "health-fitness",
        "lifestyle-hobbies",
    ]
    .into_iter()
    .collect();
    let found: BTreeSet<&str> = metadata
        .categories
        .iter()
        .map(|t| t.slug.as_str())
        .collect();
    if found != expected || found.len() != metadata.categories.len() {
        return Err("unknown, missing, or duplicate categories taxonomy".into());
    }
    let expected: BTreeSet<&str> = [
        "developers",
        "system-administrators",
        "students-learners",
        "educators",
        "researchers",
        "creative-professionals",
        "gamers",
        "makers-hobbyists",
        "business-professionals",
        "founders-business-owners",
        "community-organizers",
        "personal-use",
        "general-public",
    ]
    .into_iter()
    .collect();
    let found: BTreeSet<&str> = metadata.audiences.iter().map(|t| t.slug.as_str()).collect();
    if found != expected || found.len() != metadata.audiences.len() {
        return Err("unknown, missing, or duplicate audiences taxonomy".into());
    }
    Ok(())
}
