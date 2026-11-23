//! Live one-line status on stderr. A terminal rewrites the line in place;
//! otherwise plain lines are written when the item count changes or after
//! `NON_TTY_INTERVAL`. The existing processing `cli::Status` is unchanged;
//! drivers feed its progress into `Snapshot::extract`.

use std::io::Write;
use std::time::{Duration, Instant};

/// Longest gap between plain status lines when output is not a terminal.
pub const NON_TTY_INTERVAL: Duration = Duration::from_secs(30);

pub trait Clock {
    fn now(&self) -> Instant;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub command: String,
    pub unit: Option<String>,
    /// Kind and ID of the item being worked on, such as `("thread", 123)`.
    pub item: Option<(&'static str, i64)>,
    pub collected: usize,
    pub processed: usize,
    pub failed: usize,
    /// Extraction progress as `(done, total)` groups or stories.
    pub extract: Option<(usize, usize)>,
}

impl Snapshot {
    pub fn render(&self) -> String {
        let mut parts = vec![self.command.clone()];
        if let Some(unit) = &self.unit {
            parts.push(unit.clone());
        }
        if let Some((kind, id)) = self.item {
            parts.push(format!("{kind} {id}"));
        }
        parts.push(format!(
            "collected {} processed {} failed {}",
            self.collected, self.processed, self.failed
        ));
        if let Some((done, total)) = self.extract {
            parts.push(format!("Extracting {done}/{total} groups"));
        }
        parts.join(" | ")
    }

    fn item_count(&self) -> usize {
        self.collected + self.processed + self.failed
    }
}

pub struct StatusLine<W: Write, C: Clock> {
    out: W,
    tty: bool,
    clock: C,
    last_emit: Option<Instant>,
    last_items: Option<usize>,
    drawn: bool,
}

impl<W: Write, C: Clock> StatusLine<W, C> {
    pub fn new(out: W, tty: bool, clock: C) -> Self {
        Self {
            out,
            tty,
            clock,
            last_emit: None,
            last_items: None,
            drawn: false,
        }
    }

    pub fn update(&mut self, snapshot: &Snapshot) {
        let now = self.clock.now();
        let line = snapshot.render();
        if self.tty {
            let _ = write!(self.out, "\r\x1b[2K{line}");
            let _ = self.out.flush();
            self.drawn = true;
            return;
        }
        let items = snapshot.item_count();
        let due = match (self.last_emit, self.last_items) {
            (Some(at), Some(last)) => {
                last != items || now.saturating_duration_since(at) >= NON_TTY_INTERVAL
            }
            _ => true,
        };
        if due {
            let _ = writeln!(self.out, "{line}");
            let _ = self.out.flush();
            self.last_emit = Some(now);
            self.last_items = Some(items);
        }
    }

    /// Erases the in-place line so following output starts clean.
    pub fn clear(&mut self) {
        if self.tty && self.drawn {
            let _ = write!(self.out, "\r\x1b[2K");
            let _ = self.out.flush();
            self.drawn = false;
        }
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}
