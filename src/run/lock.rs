//! One live run per command, enforced by a PID file next to the database.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A lock file with unparsable content is stale only after this long, so a
/// file another process is just writing is never taken over.
pub const GARBAGE_GRACE: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub struct RunLock {
    path: PathBuf,
    pid: u32,
}

#[derive(Debug)]
pub struct Acquired {
    pub lock: RunLock,
    /// PID recorded by a stale lock that was taken over, when it parsed.
    pub took_over_pid: Option<u32>,
    /// A stale lock file existed (even with unreadable content).
    pub took_over: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LockError {
    Held {
        pid: u32,
        path: PathBuf,
    },
    /// A lock file with unreadable content that is too recent to take over.
    HeldUnknown {
        path: PathBuf,
    },
    Io(String),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held { pid, path } => write!(
                f,
                "another run is active (PID {pid}, lock {}); if that PID is not a wrkgn run, delete the lock file and retry",
                path.display()
            ),
            Self::HeldUnknown { path } => write!(
                f,
                "another run may be starting (lock {} has unreadable content); retry shortly, or delete the file if it persists",
                path.display()
            ),
            Self::Io(message) => write!(f, "cannot take the run lock: {message}"),
        }
    }
}

impl std::error::Error for LockError {}

impl RunLock {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        let own = fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            == Some(self.pid);
        if own {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn lock_path(db: &Path, command: &str) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(format!(".{command}.lock"));
    PathBuf::from(name)
}

fn create(path: &Path, pid: u32) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(pid.to_string().as_bytes())?;
    file.sync_all()
}

pub fn acquire(db: &Path, command: &str) -> Result<Acquired, LockError> {
    let path = lock_path(db, command);
    let pid = std::process::id();
    let io_error = |error: io::Error| LockError::Io(format!("{}: {error}", path.display()));
    match create(&path, pid) {
        Ok(()) => {
            return Ok(Acquired {
                lock: RunLock { path, pid },
                took_over_pid: None,
                took_over: false,
            });
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error(error)),
    }
    let content = fs::read_to_string(&path).map_err(io_error)?;
    let old_pid = content.trim().parse::<u32>().ok().filter(|pid| *pid > 0);
    match old_pid {
        // Any live PID, including this process, means the lock is held.
        Some(old) if crate::snapshots::process_alive(old) => {
            return Err(LockError::Held { pid: old, path });
        }
        Some(_) => {}
        None => {
            let age = fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .map_err(io_error)
                .map(|modified| {
                    SystemTime::now()
                        .duration_since(modified)
                        .unwrap_or_default()
                })?;
            if age < GARBAGE_GRACE {
                return Err(LockError::HeldUnknown { path });
            }
        }
    }
    fs::remove_file(&path).map_err(io_error)?;
    match create(&path, pid) {
        Ok(()) => Ok(Acquired {
            lock: RunLock { path, pid },
            took_over_pid: old_pid,
            took_over: true,
        }),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let other = fs::read_to_string(&path)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok());
            Err(match other {
                Some(pid) => LockError::Held { pid, path },
                None => LockError::HeldUnknown { path },
            })
        }
        Err(error) => Err(io_error(error)),
    }
}
