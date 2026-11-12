//! Local review controls and allowlisted, versioned public JSON export.
use crate::audience::{self, Audience};
use crate::category::{self, Category};
use crate::store::Store;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Serialize)]
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
#[derive(Serialize)]
pub struct Detail {
    pub id: i64,
    pub slug: String,
    pub name: Option<String>,
    pub mentions: Vec<Value>,
}

pub fn pending(store: &mut Store) -> Result<Vec<Value>, String> {
    let conn = store.connection();
    let mut st=conn.prepare("SELECT m.id,m.mention_key,m.name,m.description,m.source_item_id,s.created_at FROM mentions m JOIN source_items s ON s.item_id=m.source_item_id WHERE m.review_state='pending' ORDER BY m.id").map_err(|e|e.to_string())?;
    st.query_map([],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"key":r.get::<_,String>(1)?,"name":r.get::<_,Option<String>>(2)?,"description":r.get::<_,String>(3)?,"source_item_id":r.get::<_,i64>(4)?,"shared_at":r.get::<_,Option<i64>>(5)?}))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())
}
/// A pending mention with corrections applied, for reading in the review grid.
pub struct PendingMention {
    pub id: i64,
    pub key: String,
    pub project_id: i64,
    pub thread_id: i64,
    pub source_item_id: i64,
    pub author: Option<String>,
    pub shared_at: Option<i64>,
    pub name: Option<String>,
    pub description: String,
    pub problem: Option<String>,
    pub audience: Option<String>,
    pub builder_username: Option<String>,
    pub links: Vec<String>,
    pub categories: Vec<String>,
    pub audience_labels: Vec<String>,
    pub builders: Vec<String>,
    pub source_excerpt: Option<String>,
    pub source_text: Option<String>,
}
pub fn pending_mentions(store: &mut Store) -> Result<Vec<PendingMention>, String> {
    let conn = store.connection();
    let strings = |sql: &str, id: i64| -> Result<Vec<String>, String> {
        let mut st = conn.prepare(sql).map_err(|e| e.to_string())?;
        st.query_map([id], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())
    };
    let mut st=conn.prepare("SELECT m.id,m.mention_key,m.project_id,s.thread_id,m.source_item_id,s.author,s.created_at,m.name,m.description,m.problem,m.audience,m.builder_username,s.body,m.source_excerpt FROM mentions m JOIN source_items s ON s.item_id=m.source_item_id WHERE m.review_state='pending' ORDER BY m.id").map_err(|e|e.to_string())?;
    let rows = st
        .query_map([], |r| {
            Ok(PendingMention {
                id: r.get(0)?,
                key: r.get(1)?,
                project_id: r.get(2)?,
                thread_id: r.get(3)?,
                source_item_id: r.get(4)?,
                author: r.get(5)?,
                shared_at: r.get(6)?,
                name: r.get(7)?,
                description: r.get(8)?,
                problem: r.get(9)?,
                audience: r.get(10)?,
                builder_username: r.get(11)?,
                source_text: r.get(12)?,
                links: Vec::new(),
                categories: Vec::new(),
                audience_labels: Vec::new(),
                builders: Vec::new(),
                source_excerpt: r.get(13)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(rows.len());
    for mut m in rows {
        let mut corrections = conn
            .prepare("SELECT field,value FROM review_corrections WHERE mention_key=?1")
            .map_err(|e| e.to_string())?;
        let corrections = corrections
            .query_map([&m.key], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        for (field, value) in corrections {
            match field.as_str() {
                "name" => m.name = value,
                "description" => m.description = value.unwrap_or_default(),
                "problem" => m.problem = value,
                "audience" => m.audience = value,
                "builder_username" => m.builder_username = value,
                _ => {}
            }
        }
        m.links = strings(
            "SELECT url FROM mention_links WHERE mention_id=?1 ORDER BY url",
            m.id,
        )?;
        m.categories = strings(
            "SELECT value FROM mention_labels WHERE mention_id=?1 AND kind='category' ORDER BY value",
            m.id,
        )?;
        m.audience_labels = strings(
            "SELECT value FROM mention_labels WHERE mention_id=?1 AND kind='audience' ORDER BY value",
            m.id,
        )?;
        m.builders = strings(
            "SELECT username FROM mention_builders WHERE mention_id=?1 ORDER BY username",
            m.id,
        )?;
        out.push(m);
    }
    Ok(out)
}
pub fn approve(store: &mut Store, id: i64) -> Result<(), String> {
    approve_ranges(store, &[(id, id)]).map(|_| ())
}
/// Explicit automatic publication policy used by the separate monthly binary.
/// Call only after a complete, successful processing pass of this thread.
pub fn approve_thread(store: &mut Store, thread_id: i64) -> Result<usize, String> {
    store
        .connection()
        .execute(
            "UPDATE mentions SET review_state='approved',reviewed_at=strftime('%s','now')
         WHERE review_state='pending' AND source_item_id IN
             (SELECT item_id FROM source_items WHERE thread_id=?1)",
            [thread_id],
        )
        .map_err(|e| e.to_string())
}
/// Approves pending mentions in each inclusive ID range, in one transaction. IDs inside a
/// range that are not pending are skipped, but a range without any pending mention aborts.
pub fn approve_ranges(store: &mut Store, ranges: &[(i64, i64)]) -> Result<usize, String> {
    let tx = store
        .connection()
        .transaction()
        .map_err(|e| e.to_string())?;
    let mut total = 0;
    for &(first, last) in ranges {
        let n=tx.execute("UPDATE mentions SET review_state='approved',reviewed_at=strftime('%s','now') WHERE review_state='pending' AND id BETWEEN ?1 AND ?2",[first,last]).map_err(|e|e.to_string())?;
        if n == 0 {
            return Err(if first == last {
                format!("pending mention {first} not found")
            } else {
                format!("no pending mentions in {first}-{last}")
            });
        }
        total += n;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(total)
}
pub fn suppress_source(store: &mut Store, id: i64, reason: &str) -> Result<(), String> {
    store.connection().execute("INSERT INTO source_suppression VALUES(?1,?2) ON CONFLICT(source_item_id) DO UPDATE SET reason=excluded.reason",params![id,reason]).map_err(|e|e.to_string())?;
    Ok(())
}
pub fn suppress_project(store: &mut Store, id: i64) -> Result<(), String> {
    let n = store
        .connection()
        .execute("UPDATE projects SET suppressed=1 WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    if n == 0 {
        Err("project not found".into())
    } else {
        Ok(())
    }
}
pub fn match_projects(store: &mut Store, from: i64, to: i64) -> Result<(), String> {
    if from == to {
        return Err("projects must differ".into());
    }
    let tx = store
        .connection()
        .transaction()
        .map_err(|e| e.to_string())?;
    let identity: String = tx
        .query_row(
            "SELECT identity_key FROM projects WHERE id=?1",
            [from],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
            [to],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !exists {
        return Err("target project not found".into());
    }
    let authors = |pid| -> Result<Vec<Option<String>>, String> {
        let mut st = tx.prepare("SELECT DISTINCT s.author FROM mentions m JOIN source_items s ON s.item_id=m.source_item_id WHERE m.project_id=?1").map_err(|e| e.to_string())?;
        st.query_map([pid], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    };
    let from_authors = authors(from)?;
    let to_authors = authors(to)?;
    if from_authors.len() != 1
        || to_authors.len() != 1
        || from_authors[0].is_none()
        || from_authors != to_authors
    {
        return Err("projects must have the same known source author".into());
    }
    let suppressed: i64 = tx.query_row("SELECT (SELECT suppressed FROM projects WHERE id=?1) + (SELECT suppressed FROM projects WHERE id=?2)", params![from,to], |r| r.get(0)).map_err(|e| e.to_string())?;
    if suppressed != 0 {
        return Err("suppressed projects cannot be matched".into());
    }
    tx.execute("INSERT INTO project_matches(identity_key,project_id,reason) VALUES(?1,?2,'manual review') ON CONFLICT(identity_key) DO UPDATE SET project_id=excluded.project_id,reason=excluded.reason",params![identity,to]).map_err(|e|e.to_string())?;
    tx.execute(
        "UPDATE mentions SET project_id=?1 WHERE project_id=?2",
        params![to, from],
    )
    .map_err(|e| e.to_string())?;
    tx.execute("INSERT OR IGNORE INTO project_url_aliases(author,canonical_url,project_id) SELECT author,canonical_url,?1 FROM project_url_aliases WHERE project_id=?2", params![to,from]).map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM project_url_aliases WHERE project_id=?1",
        [from],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE project_matches SET project_id=?1 WHERE project_id=?2",
        params![to, from],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}
pub fn correct(
    store: &mut Store,
    key: &str,
    field: &str,
    value: Option<&str>,
) -> Result<(), String> {
    if ![
        "name",
        "description",
        "problem",
        "audience",
        "builder_username",
    ]
    .contains(&field)
    {
        return Err("invalid correction field".into());
    }
    store.connection().execute("INSERT INTO review_corrections VALUES(?1,?2,?3) ON CONFLICT(mention_key,field) DO UPDATE SET value=excluded.value",params![key,field,value]).map_err(|e|e.to_string())?;
    Ok(())
}

type Record = (
    i64,
    String,
    String,
    Option<String>,
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
    String,
);
type Group = (
    String,
    Vec<Value>,
    BTreeSet<String>,
    BTreeSet<String>,
    BTreeSet<String>,
);

pub fn export(store: &mut Store, destination: &Path) -> Result<String, String> {
    let conn = store.connection();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let roots: Vec<i64> = {
        let mut st = tx
            .prepare("SELECT root_item_id FROM processing_groups")
            .map_err(|e| e.to_string())?;
        st.query_map([], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())?
    };
    let mut current_groups = BTreeMap::new();
    for root in roots {
        let mut st=tx.prepare("WITH RECURSIVE tree(item_id,parent_id,sibling_position,content_fingerprint,present_in_tree,path,thread_id) AS (
            SELECT item_id,parent_id,sibling_position,content_fingerprint,present_in_tree,printf('%020d:%020d',sibling_position,item_id),thread_id FROM source_items WHERE item_id=?1 AND present_in_tree=1
            UNION ALL
            SELECT s.item_id,s.parent_id,s.sibling_position,s.content_fingerprint,s.present_in_tree,t.path||'/'||printf('%020d:%020d',s.sibling_position,s.item_id),s.thread_id FROM source_items s JOIN tree t ON s.parent_id=t.item_id WHERE s.present_in_tree=1 AND s.thread_id=t.thread_id
        ) SELECT item_id,parent_id,sibling_position,content_fingerprint,present_in_tree FROM tree ORDER BY path").map_err(|e|e.to_string())?;
        let rows = st
            .query_map([root], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, bool>(4)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut digest = Sha256::new();
        let mut any = false;
        for row in rows {
            let (id, parent, position, fp, present) = row.map_err(|e| e.to_string())?;
            any = true;
            digest.update(id.to_be_bytes());
            digest.update(parent.unwrap_or(0).to_be_bytes());
            digest.update(position.to_be_bytes());
            digest.update(fp.as_bytes());
            digest.update([present as u8]);
        }
        if any {
            current_groups.insert(root, format!("{:x}", digest.finalize()));
        }
    }
    let records: Vec<Record> = {
        let mut st=tx.prepare("SELECT p.id,p.slug,m.mention_key,m.name,m.source_item_id,m.description,m.problem,m.audience,m.builder_username,s.author,s.created_at,m.root_item_id,g.fingerprint FROM mentions m JOIN projects p ON p.id=m.project_id JOIN source_items s ON s.item_id=m.source_item_id JOIN processing_groups g ON g.root_item_id=m.root_item_id WHERE m.review_state='approved' AND p.suppressed=0 AND s.present_in_tree=1 AND s.deleted=0 AND s.dead=0 AND s.content_fingerprint=m.source_fingerprint AND s.created_at IS NOT NULL AND g.status='complete' AND NOT EXISTS(SELECT 1 FROM source_suppression x WHERE x.source_item_id=s.item_id) ORDER BY p.id,s.created_at DESC,m.id").map_err(|e|e.to_string())?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                    r.get(12)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())?
    };
    let mut grouped: BTreeMap<i64, Group> = BTreeMap::new();
    for (
        pid,
        slug,
        key,
        name,
        source_id,
        description,
        problem,
        audience,
        builder,
        source_author,
        shared_at,
        root_id,
        group_fp,
    ) in records
    {
        if current_groups.get(&root_id) != Some(&group_fp) {
            continue;
        }
        let corrected = |field: &str, original: Option<String>| -> Result<Option<String>, String> {
            let found: Option<Option<String>> = tx
                .query_row(
                    "SELECT value FROM review_corrections WHERE mention_key=?1 AND field=?2",
                    params![key, field],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            Ok(found.unwrap_or(original))
        };
        let name = corrected("name", name)?;
        let description = corrected("description", Some(description))?.unwrap_or_default();
        if description.trim().is_empty() {
            continue;
        }
        let problem = corrected("problem", problem)?;
        let audience = corrected("audience", audience)?;
        let builder =
            corrected("builder_username", builder)?.filter(|b| source_author.as_deref() == Some(b));
        let links: Vec<String> = {
            let mut st=tx.prepare("SELECT url FROM mention_links WHERE mention_id=(SELECT id FROM mentions WHERE mention_key=?1) ORDER BY url").map_err(|e|e.to_string())?;
            st.query_map([&key], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| e.to_string())?
        };
        let labels: Vec<(String, String)> = {
            let mut st=tx.prepare("SELECT kind,value FROM mention_labels WHERE mention_id=(SELECT id FROM mentions WHERE mention_key=?1) ORDER BY kind,value").map_err(|e|e.to_string())?;
            st.query_map([&key], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())?
        };
        let mut builders: Vec<String> = {
            let mut st = tx.prepare("SELECT username FROM mention_builders WHERE mention_id=(SELECT id FROM mentions WHERE mention_key=?1) ORDER BY username").map_err(|e| e.to_string())?;
            st.query_map([&key], |r| r.get(0))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| e.to_string())?
        };
        if let Some(b) = builder.as_ref() {
            builders.push(b.clone());
        }
        builders.sort();
        builders.dedup();
        let source_excerpt: Option<String> = tx
            .query_row(
                "SELECT source_excerpt FROM mentions WHERE mention_key=?1",
                [&key],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if source_excerpt
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
        {
            return Err(format!(
                "mention {key} has no project source excerpt; reprocess this thread with the current extraction schema"
            ));
        }
        let entry = grouped.entry(pid).or_insert_with(|| {
            (
                slug,
                Vec::new(),
                BTreeSet::new(),
                BTreeSet::new(),
                BTreeSet::new(),
            )
        });
        entry.2.extend(builders.iter().cloned());
        for (kind, value) in labels {
            match kind.as_str() {
                "category" => {
                    if Category::from_slug(&value).is_none() {
                        return Err(format!(
                            "unknown stored category {value} on mention {key}; run `wrkgn categories normalize`"
                        ));
                    }
                    entry.3.insert(value);
                }
                "audience" => {
                    if Audience::from_slug(&value).is_none() {
                        return Err(format!(
                            "unknown stored audience {value} on mention {key}; reprocess this thread with the current extraction schema"
                        ));
                    }
                    entry.4.insert(value);
                }
                _ => {}
            }
        }
        entry.1.push(json!({"name":name,"description":description,"problem":problem,"audience":audience,"builder_usernames":builders,"source_excerpt":source_excerpt,"source_item_id":source_id,"source_url":source_url(source_id),"author":source_author,"shared_at":shared_at,"links":links}));
    }
    let mut discovery = Vec::new();
    let mut details = Vec::new();
    for (id, (slug, mentions, builders, categories, audiences)) in grouped {
        let first = &mentions[0];
        let name = first["name"].as_str().map(str::to_string);
        let description = first["description"].as_str().unwrap_or("").to_string();
        let problem = first["problem"].as_str().map(str::to_string);
        let audience = first["audience"].as_str().map(str::to_string);
        let latest = first["shared_at"].as_i64().unwrap_or(0);
        let builders: Vec<_> = builders.into_iter().collect();
        let categories: Vec<_> = categories.into_iter().collect();
        let audiences: Vec<_> = audiences.into_iter().collect();
        let searchable_text = format!(
            "{} {} {} {} {}",
            name.as_deref().unwrap_or(""),
            description,
            problem.as_deref().unwrap_or(""),
            audience.as_deref().unwrap_or(""),
            builders.join(" ")
        );
        // Count distinct replies across published mention subtrees. UNION also
        // deduplicates overlapping trees when a project has several mentions.
        let source_ids: Vec<_> = mentions
            .iter()
            .map(|m| m["source_item_id"].clone())
            .collect();
        let comment_count = tx.query_row(
            "WITH RECURSIVE replies(item_id,thread_id) AS (
                SELECT s.item_id,s.thread_id FROM source_items s
                JOIN source_items parent ON parent.item_id=s.parent_id AND parent.thread_id=s.thread_id
                WHERE parent.item_id IN (SELECT value FROM json_each(?1)) AND s.present_in_tree=1
                UNION
                SELECT s.item_id,s.thread_id FROM source_items s JOIN replies r
                ON s.parent_id=r.item_id AND s.thread_id=r.thread_id WHERE s.present_in_tree=1
            ) SELECT COUNT(*) FROM replies r JOIN source_items s ON s.item_id=r.item_id
              WHERE s.deleted=0 AND s.dead=0 AND NOT EXISTS
              (SELECT 1 FROM source_suppression x WHERE x.source_item_id=s.item_id)",
            [serde_json::to_string(&source_ids).map_err(|e| e.to_string())?],
            |r| r.get(0),
        ).map_err(|e| e.to_string())?;
        discovery.push(Discovery {
            id,
            slug: slug.clone(),
            name: name.clone(),
            description,
            problem,
            audience,
            builder_usernames: builders,
            categories,
            audience_labels: audiences,
            latest_shared_at: latest,
            comment_count,
            searchable_text,
        });
        details.push(Detail {
            id,
            slug,
            name,
            mentions,
        });
    }
    tx.commit().map_err(|e| e.to_string())?;
    discovery.sort_by(|a, b| {
        b.latest_shared_at
            .cmp(&a.latest_shared_at)
            .then(a.id.cmp(&b.id))
    });
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&discovery).map_err(|e| e.to_string())?);
    for detail in &details {
        digest.update(serde_json::to_vec(detail).map_err(|e| e.to_string())?);
    }
    digest.update(serde_json::to_vec(&category::metadata()).map_err(|e| e.to_string())?);
    digest.update(serde_json::to_vec(&audience::metadata()).map_err(|e| e.to_string())?);
    let version = format!("{:x}", digest.finalize());
    let version = &version[..16];
    let snapshot = destination.join("snapshots").join(version);
    if !snapshot.exists() {
        let staging = destination
            .join("snapshots")
            .join(format!(".staging-{}-{version}", std::process::id()));
        fs::create_dir_all(staging.join("projects")).map_err(|e| e.to_string())?;
        write_json(&staging.join("discovery.json"), &discovery)?;
        for detail in &details {
            write_json(
                &staging
                    .join("projects")
                    .join(format!("{}.json", detail.slug)),
                detail,
            )?;
        }
        write_json(
            &staging.join("manifest.json"),
            &json!({"schema_version":4,"version":version,"discovery":"discovery.json","project_details":"projects/<slug>.json","project_count":details.len(),"categories":category::metadata(),"audiences":audience::metadata()}),
        )?;
        fs::rename(staging, &snapshot).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    let pointer = destination.join("current.json.tmp");
    write_json(
        &pointer,
        &json!({"schema_version":4,"version":version,"snapshot":format!("snapshots/{version}/manifest.json")}),
    )?;
    fs::rename(pointer, destination.join("current.json")).map_err(|e| e.to_string())?;
    Ok(version.to_string())
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    fs::write(path, bytes).map_err(|e| e.to_string())
}
fn source_url(id: i64) -> String {
    format!("https://news.ycombinator.com/item?id={id}")
}
