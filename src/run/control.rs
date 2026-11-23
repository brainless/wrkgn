//! Shared stop state: the consecutive-failure circuit breaker and the
//! interrupt flag. Only atomics, so it is shared freely across worker threads.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

/// Consecutive exhausted calls on one API type that stop the run.
pub const BREAKER_THRESHOLD: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Api {
    Llm,
    HnFirebase,
    Algolia,
}

impl Api {
    const ALL: [Api; 3] = [Api::Llm, Api::HnFirebase, Api::Algolia];

    fn index(self) -> usize {
        match self {
            Api::Llm => 0,
            Api::HnFirebase => 1,
            Api::Algolia => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Api::Llm => "LLM provider",
            Api::HnFirebase => "HN Firebase API",
            Api::Algolia => "Algolia search",
        }
    }
}

/// Separate consecutive-failure counters per API type. A failure is a call
/// that exhausted its retries; any success resets that API's counter.
#[derive(Debug)]
pub struct Breaker {
    counters: [AtomicUsize; 3],
    threshold: usize,
    tripped: AtomicBool,
    /// 0 = none, otherwise index + 1.
    tripped_api: AtomicU8,
}

impl Default for Breaker {
    fn default() -> Self {
        Self::new(BREAKER_THRESHOLD)
    }
}

impl Breaker {
    pub fn new(threshold: usize) -> Self {
        Self {
            counters: [const { AtomicUsize::new(0) }; 3],
            threshold,
            tripped: AtomicBool::new(false),
            tripped_api: AtomicU8::new(0),
        }
    }

    /// Counts one exhausted call; returns whether the breaker is now tripped.
    pub fn record_failure(&self, api: Api) -> bool {
        let count = self.counters[api.index()].fetch_add(1, Ordering::SeqCst) + 1;
        if count >= self.threshold
            && self
                .tripped
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            self.tripped_api
                .store(api.index() as u8 + 1, Ordering::SeqCst);
        }
        self.is_tripped()
    }

    pub fn record_success(&self, api: Api) {
        self.counters[api.index()].store(0, Ordering::SeqCst);
    }

    pub fn is_tripped(&self) -> bool {
        self.tripped.load(Ordering::SeqCst)
    }

    /// The API whose counter tripped the breaker first.
    pub fn tripped_api(&self) -> Option<Api> {
        match self.tripped_api.load(Ordering::SeqCst) {
            0 => None,
            n => Api::ALL.get(usize::from(n) - 1).copied(),
        }
    }

    pub fn consecutive_failures(&self, api: Api) -> usize {
        self.counters[api.index()].load(Ordering::SeqCst)
    }
}

#[derive(Debug, Default)]
pub struct RunControl {
    pub breaker: Breaker,
    interrupted: AtomicBool,
}

impl RunControl {
    pub fn new(breaker: Breaker) -> Self {
        Self {
            breaker,
            interrupted: AtomicBool::new(false),
        }
    }

    pub fn with_threshold(threshold: usize) -> Self {
        Self::new(Breaker::new(threshold))
    }

    /// A control that never trips; interrupts still work.
    pub fn disabled() -> Self {
        Self::with_threshold(usize::MAX)
    }

    /// Requests a stop; returns whether one was already requested.
    pub fn interrupt(&self) -> bool {
        self.interrupted.swap(true, Ordering::SeqCst)
    }

    pub fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    pub fn should_stop(&self) -> bool {
        self.interrupted() || self.breaker.is_tripped()
    }
}
