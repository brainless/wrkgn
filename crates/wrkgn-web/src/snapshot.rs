use crate::{discovery::DiscoveryIndex, similar::SimilarIndex};
use rusqlite::Connection;
use std::{
    path::Path,
    sync::{Arc, Condvar, Mutex},
};
use wrkgn_public_snapshot::{self as public, Metadata};

/// All waiting and SQLite work occurs inside spawn_blocking, never on Tokio workers.
pub struct ReadPool {
    available: Mutex<Vec<Connection>>,
    ready: Condvar,
}
impl ReadPool {
    fn new(connections: Vec<Connection>) -> Self {
        Self {
            available: Mutex::new(connections),
            ready: Condvar::new(),
        }
    }
    fn run<T>(&self, work: impl FnOnce(&Connection) -> Result<T, String>) -> Result<T, String> {
        let mut available = self
            .available
            .lock()
            .map_err(|_| "snapshot pool poisoned")?;
        while available.is_empty() {
            available = self
                .ready
                .wait(available)
                .map_err(|_| "snapshot pool poisoned")?;
        }
        let conn = available.pop().unwrap();
        drop(available);
        // Return the connection even if a reader panics.
        struct Lease<'a> {
            pool: &'a ReadPool,
            conn: Option<Connection>,
        }
        impl Drop for Lease<'_> {
            fn drop(&mut self) {
                if let Some(conn) = self.conn.take()
                    && let Ok(mut available) = self.pool.available.lock()
                {
                    available.push(conn);
                    self.pool.ready.notify_one();
                }
            }
        }
        let lease = Lease {
            pool: self,
            conn: Some(conn),
        };
        work(lease.conn.as_ref().unwrap())
    }
}
pub struct Snapshot {
    pub metadata: Metadata,
    pub discovery: DiscoveryIndex,
    pub similar_projects: SimilarIndex,
    pool: ReadPool,
}
impl Snapshot {
    pub fn load(path: &Path, connections: usize) -> Result<Self, String> {
        if connections == 0 {
            return Err("snapshot pool needs at least one connection".into());
        }
        let selected = public::select(path)?;
        let conn = public::open_read_only(&selected.database)?;
        let metadata = public::validate_connection(&conn, selected.version.as_deref())?;
        let discovery = DiscoveryIndex::new(public::discovery(&conn)?);
        let similar_projects = SimilarIndex::new(&discovery.projects);
        let mut pool = vec![conn];
        for _ in 1..connections {
            pool.push(public::open_read_only(&selected.database)?);
        }
        Ok(Self {
            metadata,
            discovery,
            similar_projects,
            pool: ReadPool::new(pool),
        })
    }
    pub async fn read<T: Send + 'static>(
        self: &Arc<Self>,
        work: impl FnOnce(&Connection) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let snapshot = Arc::clone(self);
        tokio::task::spawn_blocking(move || snapshot.pool.run(work))
            .await
            .map_err(|_| "snapshot reader failed".to_string())?
    }
}
