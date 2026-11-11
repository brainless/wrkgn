//! Local source records. API failures never imply that a source item was deleted.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use sha2::{Digest, Sha256};

const LATEST_SCHEMA_VERSION: i64 = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct SourceItem {
    pub item_id: i64,
    pub thread_id: i64,
    pub parent_id: Option<i64>,
    pub sibling_position: i64,
    pub author: Option<String>,
    pub created_at: Option<i64>,
    pub body: Option<String>,
    pub raw_payload: Value,
    pub deleted: bool,
    pub dead: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredSourceItem {
    pub source: SourceItem,
    pub content_fingerprint: String,
    pub first_collected_at: i64,
    pub last_fetched_at: i64,
    pub revision: i64,
    pub last_run_id: i64,
    pub present_in_tree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchFailure {
    pub item_id: i64,
    pub kind: String,
    pub detail: String,
    pub occurred_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionRun {
    pub id: i64,
    pub thread_id: i64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: String,
    pub items_fetched: i64,
    pub failure_count: i64,
    pub interrupted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceRevision {
    pub source: SourceItem,
    pub content_fingerprint: String,
    pub revision: i64,
    pub run_id: i64,
    pub changed_at: i64,
    pub present_in_tree: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationNode {
    pub item: StoredSourceItem,
    pub replies: Vec<ConversationNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationTree {
    pub thread: StoredSourceItem,
    pub groups: Vec<ConversationNode>,
}

pub struct Store {
    connection: Connection,
}

impl Store {
    /// Uses `WRKGN_DB_PATH`, or `local-data/wrkgn.sqlite` relative to the
    /// current directory. The default path is excluded from version control.
    pub fn open_configured() -> rusqlite::Result<Self> {
        let path = env::var_os("WRKGN_DB_PATH")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("local-data/wrkgn.sqlite"));
        Self::open(path)
    }

    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> rusqlite::Result<()> {
        let mut current: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if current > LATEST_SCHEMA_VERSION {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "database schema version {current} is newer than supported version {LATEST_SCHEMA_VERSION}"
            )));
        }
        if current < 1 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(include_str!("../migrations/001_source_storage.sql"))?;
            transaction.pragma_update(None, "user_version", 1)?;
            transaction.commit()?;
            current = 1;
        }
        if current < 2 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(include_str!("../migrations/002_refresh_state.sql"))?;
            transaction.pragma_update(None, "user_version", 2)?;
            transaction.commit()?;
        }
        if current < 3 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(include_str!("../migrations/003_processing.sql"))?;
            transaction.pragma_update(None, "user_version", 3)?;
            transaction.commit()?;
        }
        if current < 4 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(include_str!("../migrations/004_processing_failures.sql"))?;
            transaction.pragma_update(None, "user_version", 4)?;
            transaction.commit()?;
        }
        if current < 5 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(include_str!("../migrations/005_project_extraction.sql"))?;
            transaction.pragma_update(None, "user_version", 5)?;
            transaction.commit()?;
        }
        Ok(())
    }

    pub(crate) fn connection(&mut self) -> &mut Connection {
        &mut self.connection
    }

    pub fn begin_run(&self, thread_id: i64, started_at: i64) -> rusqlite::Result<i64> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "UPDATE collection_runs SET status = 'incomplete', interrupted = 1,
                    finished_at = ?2
             WHERE thread_id = ?1 AND status = 'running'",
            params![thread_id, started_at],
        )?;
        transaction.execute(
            "INSERT INTO collection_runs (thread_id, started_at, status) VALUES (?1, ?2, 'running')",
            params![thread_id, started_at],
        )?;
        let run_id = transaction.last_insert_rowid();
        transaction.commit()?;
        Ok(run_id)
    }

    pub fn get_run(&self, run_id: i64) -> rusqlite::Result<Option<CollectionRun>> {
        self.connection
            .query_row(
                "SELECT id, thread_id, started_at, finished_at, status, items_fetched,
                    failure_count, interrupted FROM collection_runs WHERE id = ?1",
                [run_id],
                |row| {
                    Ok(CollectionRun {
                        id: row.get(0)?,
                        thread_id: row.get(1)?,
                        started_at: row.get(2)?,
                        finished_at: row.get(3)?,
                        status: row.get(4)?,
                        items_fetched: row.get(5)?,
                        failure_count: row.get(6)?,
                        interrupted: row.get(7)?,
                    })
                },
            )
            .optional()
    }

    pub fn latest_run_for_thread(&self, thread_id: i64) -> rusqlite::Result<Option<CollectionRun>> {
        let run_id = self
            .connection
            .query_row(
                "SELECT id FROM collection_runs WHERE thread_id = ?1 ORDER BY id DESC LIMIT 1",
                [thread_id],
                |row| row.get(0),
            )
            .optional()?;
        run_id
            .map(|id| self.get_run(id))
            .transpose()
            .map(Option::flatten)
    }

    /// Completion is allowed only when no failures were recorded for this run.
    pub fn finish_run(&self, run_id: i64, finished_at: i64) -> rusqlite::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE collection_runs SET finished_at = ?2,
                status = CASE WHEN failure_count = 0 THEN 'complete' ELSE 'incomplete' END
             WHERE id = ?1 AND status = 'running'",
            params![run_id, finished_at],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        let (thread_id, complete): (i64, bool) = transaction.query_row(
            "SELECT thread_id, status = 'complete' FROM collection_runs WHERE id = ?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if complete {
            let absent_ids: Vec<i64> = {
                let mut statement = transaction.prepare(
                    "SELECT item_id FROM source_items
                     WHERE thread_id = ?1 AND last_run_id != ?2 AND present_in_tree = 1
                     ORDER BY item_id",
                )?;
                statement
                    .query_map(params![thread_id, run_id], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for item_id in absent_ids {
                transaction.execute(
                    "UPDATE source_items SET present_in_tree = 0, revision = revision + 1
                     WHERE item_id = ?1",
                    [item_id],
                )?;
                transaction.execute(
                    "INSERT INTO source_item_revisions
                     (item_id, revision, run_id, changed_at, thread_id, parent_id,
                      sibling_position, author, created_at, body, raw_payload,
                      content_fingerprint, deleted, dead, present_in_tree)
                     SELECT item_id, revision, ?2, ?3, thread_id, parent_id,
                            sibling_position, author, created_at, body, raw_payload,
                            content_fingerprint, deleted, dead, present_in_tree
                     FROM source_items WHERE item_id = ?1",
                    params![item_id, run_id, finished_at],
                )?;
            }
        }
        transaction.commit()
    }

    pub fn record_fetch_failure(
        &self,
        run_id: i64,
        failure: &FetchFailure,
    ) -> rusqlite::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO fetch_failures (run_id, item_id, kind, detail, occurred_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                run_id,
                failure.item_id,
                failure.kind,
                failure.detail,
                failure.occurred_at
            ],
        )?;
        transaction.execute(
            "UPDATE collection_runs SET failure_count = failure_count + 1 WHERE id = ?1",
            [run_id],
        )?;
        transaction.commit()
    }

    pub fn failures_for_run(&self, run_id: i64) -> rusqlite::Result<Vec<FetchFailure>> {
        let mut statement = self.connection.prepare(
            "SELECT item_id, kind, detail, occurred_at FROM fetch_failures
             WHERE run_id = ?1 ORDER BY id",
        )?;
        statement
            .query_map([run_id], |row| {
                Ok(FetchFailure {
                    item_id: row.get(0)?,
                    kind: row.get(1)?,
                    detail: row.get(2)?,
                    occurred_at: row.get(3)?,
                })
            })?
            .collect()
    }

    /// Returns true when source data changed. Refreshes that only update the
    /// fetch time do not create a new revision.
    pub fn upsert_item(
        &mut self,
        run_id: i64,
        fetched_at: i64,
        item: &SourceItem,
    ) -> rusqlite::Result<bool> {
        let raw_payload = item.raw_payload.to_string();
        let fingerprint = content_fingerprint(item);
        let transaction = self.connection.transaction()?;
        let previous = transaction
            .query_row(
                "SELECT thread_id, parent_id, sibling_position, author, created_at, body,
                    raw_payload, deleted, dead, revision, present_in_tree
             FROM source_items WHERE item_id = ?1",
                [item.item_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, bool>(7)?,
                        row.get::<_, bool>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, bool>(10)?,
                    ))
                },
            )
            .optional()?;
        let changed = previous.as_ref().is_none_or(|old| {
            old.0 != item.thread_id
                || old.1 != item.parent_id
                || old.2 != item.sibling_position
                || old.3 != item.author
                || old.4 != item.created_at
                || old.5 != item.body
                || old.6 != raw_payload
                || old.7 != item.deleted
                || old.8 != item.dead
                || !old.10
        });
        let revision = previous.map_or(1, |old| old.9 + i64::from(changed));

        transaction.execute(
            "INSERT INTO source_items
             (item_id, thread_id, parent_id, sibling_position, author, created_at, body,
              raw_payload, content_fingerprint, deleted, dead, first_collected_at,
              last_fetched_at, revision, last_run_id, present_in_tree)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, ?13, ?14, 1)
             ON CONFLICT(item_id) DO UPDATE SET
              thread_id = excluded.thread_id, parent_id = excluded.parent_id,
              sibling_position = excluded.sibling_position, author = excluded.author,
              created_at = excluded.created_at, body = excluded.body,
              raw_payload = excluded.raw_payload,
              content_fingerprint = excluded.content_fingerprint,
              deleted = excluded.deleted, dead = excluded.dead,
              last_fetched_at = excluded.last_fetched_at, revision = excluded.revision,
              last_run_id = excluded.last_run_id,
              present_in_tree = 1",
            params![
                item.item_id,
                item.thread_id,
                item.parent_id,
                item.sibling_position,
                item.author,
                item.created_at,
                item.body,
                raw_payload,
                fingerprint,
                item.deleted,
                item.dead,
                fetched_at,
                revision,
                run_id
            ],
        )?;
        if changed {
            transaction.execute(
                "INSERT INTO source_item_revisions
                 (item_id, revision, run_id, changed_at, thread_id, parent_id,
                  sibling_position, author, created_at, body, raw_payload,
                  content_fingerprint, deleted, dead, present_in_tree)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 1)",
                params![
                    item.item_id,
                    revision,
                    run_id,
                    fetched_at,
                    item.thread_id,
                    item.parent_id,
                    item.sibling_position,
                    item.author,
                    item.created_at,
                    item.body,
                    raw_payload,
                    fingerprint,
                    item.deleted,
                    item.dead
                ],
            )?;
        }
        transaction.execute(
            "UPDATE collection_runs SET items_fetched = items_fetched + 1 WHERE id = ?1",
            [run_id],
        )?;
        transaction.commit()?;
        Ok(changed)
    }

    pub fn get_item(&self, item_id: i64) -> rusqlite::Result<Option<StoredSourceItem>> {
        self.connection
            .query_row(
                "SELECT item_id, thread_id, parent_id, sibling_position, author, created_at,
                    body, raw_payload, deleted, dead, content_fingerprint,
                    first_collected_at, last_fetched_at, revision, last_run_id, present_in_tree
             FROM source_items WHERE item_id = ?1",
                [item_id],
                read_item,
            )
            .optional()
    }

    pub fn children_of(&self, parent_id: i64) -> rusqlite::Result<Vec<StoredSourceItem>> {
        let mut statement = self.connection.prepare(
            "SELECT item_id, thread_id, parent_id, sibling_position, author, created_at,
                    body, raw_payload, deleted, dead, content_fingerprint,
                    first_collected_at, last_fetched_at, revision, last_run_id, present_in_tree
             FROM source_items WHERE parent_id = ?1
             ORDER BY sibling_position, item_id",
        )?;
        statement.query_map([parent_id], read_item)?.collect()
    }

    pub fn changed_item_ids_for_run(&self, run_id: i64) -> rusqlite::Result<Vec<i64>> {
        let mut statement = self.connection.prepare(
            "SELECT DISTINCT item_id FROM source_item_revisions WHERE run_id = ?1 ORDER BY item_id",
        )?;
        statement.query_map([run_id], |row| row.get(0))?.collect()
    }

    pub fn revisions_for_item(&self, item_id: i64) -> rusqlite::Result<Vec<SourceRevision>> {
        let mut statement = self.connection.prepare(
            "SELECT item_id, thread_id, parent_id, sibling_position, author, created_at,
                    body, raw_payload, deleted, dead, content_fingerprint, revision,
                    run_id, changed_at, present_in_tree FROM source_item_revisions
             WHERE item_id = ?1 ORDER BY revision",
        )?;
        statement
            .query_map([item_id], |row| {
                let raw: String = row.get(7)?;
                let raw_payload = serde_json::from_str(&raw).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        7,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(SourceRevision {
                    source: SourceItem {
                        item_id: row.get(0)?,
                        thread_id: row.get(1)?,
                        parent_id: row.get(2)?,
                        sibling_position: row.get(3)?,
                        author: row.get(4)?,
                        created_at: row.get(5)?,
                        body: row.get(6)?,
                        raw_payload,
                        deleted: row.get(8)?,
                        dead: row.get(9)?,
                    },
                    content_fingerprint: row.get(10)?,
                    revision: row.get(11)?,
                    run_id: row.get(12)?,
                    changed_at: row.get(13)?,
                    present_in_tree: row.get(14)?,
                })
            })?
            .collect()
    }

    /// Ordered top-level comment groups with their full reply subtrees.
    /// Callers should check that the latest collection run is complete before
    /// using this tree for extraction or publication.
    pub fn conversation_groups(&self, thread_id: i64) -> rusqlite::Result<ConversationTree> {
        let thread = self
            .get_item(thread_id)?
            .filter(|item| item.source.thread_id == thread_id && item.present_in_tree)
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let mut groups = Vec::new();
        for child in self.children_of(thread_id)? {
            if child.present_in_tree && child.source.thread_id == thread_id {
                groups.push(self.conversation_node(child)?);
            }
        }
        Ok(ConversationTree { thread, groups })
    }

    fn conversation_node(&self, item: StoredSourceItem) -> rusqlite::Result<ConversationNode> {
        let mut replies = Vec::new();
        for child in self.children_of(item.source.item_id)? {
            if child.present_in_tree && child.source.thread_id == item.source.thread_id {
                replies.push(self.conversation_node(child)?);
            }
        }
        Ok(ConversationNode { item, replies })
    }

    pub fn schema_version(&self) -> rusqlite::Result<i64> {
        self.connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
    }
}

fn read_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredSourceItem> {
    let raw: String = row.get(7)?;
    let raw_payload = serde_json::from_str(&raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(StoredSourceItem {
        source: SourceItem {
            item_id: row.get(0)?,
            thread_id: row.get(1)?,
            parent_id: row.get(2)?,
            sibling_position: row.get(3)?,
            author: row.get(4)?,
            created_at: row.get(5)?,
            body: row.get(6)?,
            raw_payload,
            deleted: row.get(8)?,
            dead: row.get(9)?,
        },
        content_fingerprint: row.get(10)?,
        first_collected_at: row.get(11)?,
        last_fetched_at: row.get(12)?,
        revision: row.get(13)?,
        last_run_id: row.get(14)?,
        present_in_tree: row.get(15)?,
    })
}

/// Hashes source content and attribution, excluding child-list changes and
/// other volatile HN metadata such as score.
pub fn content_fingerprint(item: &SourceItem) -> String {
    let material = serde_json::to_vec(&(
        &item.author,
        item.created_at,
        &item.body,
        item.raw_payload.get("title"),
        item.raw_payload.get("url"),
        item.raw_payload.get("type"),
        item.deleted,
        item.dead,
    ))
    .expect("source fingerprint material is serializable");
    format!("{:x}", Sha256::digest(material))
}
