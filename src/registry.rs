// SPDX-License-Identifier: MIT OR Apache-2.0

//! A bounded record of outstanding continuations, exposed through exfiltrate.
//!
//! A continuation is a future waiting for something outside Rust to signal it.
//! When that signal never comes the program hangs with no stack to look at:
//! the waiting task is parked and the sender is somewhere a debugger cannot
//! follow. This is the only way to see it from outside.
//!
//! **Age is the point.** A continuation outstanding for 30 seconds in a program
//! whose operations take milliseconds *is* the bug, and nothing else in the
//! process can tell you it exists.
//!
//! # Identity
//!
//! Two ids, deliberately.
//!
//! `context` is the continuation's `logwise::ContextToken`, which this crate
//! already mints per continuation and which is a *durable, causal* identity
//! shared with the rest of the ecosystem — `some_executor` mints one per task
//! the same way. That makes a stuck continuation and the task awaiting it
//! joinable, through `snapshot --subsystem context --id <token>` and through
//! the logwise records carrying the same token.
//!
//! But the facade mints `NONE` until a runtime is installed, so with no
//! runtime **every** continuation would share the token `0` and none could be
//! told apart. `seq` is therefore a local monotonic id that is always distinct,
//! and it is what `--id` selects. `context` is reported alongside for joining,
//! and is `0` exactly when no runtime was installed to mint one.
//!
//! # Bounded, and honest about it
//!
//! Keeps the most recent `N`; `N` comes from
//! `CONTINUE_REGISTRY_CAPACITY` and defaults to [`DEFAULT_CAPACITY`]. Overflow
//! forgets *settled* continuations before outstanding ones — an outstanding one
//! is the whole reason to look — and every drop is counted and reported, so a
//! caller can never mistake "nothing is stuck" for "I lost it".
//!
//! Behind the `exfiltrate` feature. With it off none of this is compiled and
//! creating a continuation does not touch a lock.

use std::collections::VecDeque;
use std::panic::Location;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use wasm_lite_std::time::Instant;

pub const DEFAULT_CAPACITY: usize = 1024;

/// What happened to a continuation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// Still waiting. This is the interesting one.
    Outstanding,
    /// A value was sent.
    Sent,
    /// The future was dropped before a value arrived; the sender will find it
    /// hung up.
    FutureHungUp,
    /// The sender was dropped without sending. This crate defines that as a
    /// programmer error, and it is the error that otherwise shows up only as a
    /// hang somewhere else entirely.
    SenderDroppedUnsent,
}

impl Outcome {
    pub const fn name(self) -> &'static str {
        match self {
            Outcome::Outstanding => "outstanding",
            Outcome::Sent => "sent",
            Outcome::FutureHungUp => "future-hung-up",
            Outcome::SenderDroppedUnsent => "sender-dropped-unsent",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// A locally minted id, always distinct. What `--id` selects.
    pub seq: u64,
    /// The continuation's durable logwise context, for joining across crates.
    /// `0` when no logwise runtime was installed to mint one.
    pub context: u64,
    /// Where `continuation()` was called, captured with `#[track_caller]` so no
    /// call site had to change to become debuggable.
    pub created_at_file: &'static str,
    pub created_at_line: u32,
    pub created: Instant,
    pub outcome: Outcome,
    /// Whether the future has been polled at least once. An unpolled
    /// outstanding continuation means nobody is even waiting yet, which is a
    /// different bug from one whose signal never came.
    pub polled: bool,
}

struct Registry {
    capacity: usize,
    entries: VecDeque<Entry>,
}

impl Registry {
    /// Inserts, evicting if full. Returns how many entries it had to forget.
    ///
    /// A method rather than inline in [`record_created`] so the policy can be
    /// tested on a local registry: the global one is shared with every other
    /// test in this crate that happens to create a continuation, so nothing
    /// asserting on eviction order could be deterministic against it.
    fn push(&mut self, entry: Entry) -> u64 {
        let mut forgotten = 0;
        while self.entries.len() >= self.capacity {
            // Forget something already settled before anything still waiting:
            // an outstanding continuation is the whole reason to look.
            let victim = self
                .entries
                .iter()
                .position(|entry| entry.outcome != Outcome::Outstanding)
                .unwrap_or(0);
            self.entries.remove(victim);
            forgotten += 1;
        }
        self.entries.push_back(entry);
        forgotten
    }
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static CREATED: AtomicU64 = AtomicU64::new(0);
static NEXT_SEQ: AtomicU64 = AtomicU64::new(1);

fn configured_capacity() -> usize {
    std::env::var("CONTINUE_REGISTRY_CAPACITY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|capacity| *capacity > 0)
        .unwrap_or(DEFAULT_CAPACITY)
}

fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> Option<R> {
    // `try_lock`: creating a continuation must not block on a query, and a
    // query must not block the program. A missed record is counted.
    let mut guard = REGISTRY.try_lock().ok()?;
    let registry = guard.get_or_insert_with(|| Registry {
        capacity: configured_capacity(),
        entries: VecDeque::new(),
    });
    Some(f(registry))
}

/// Records a new continuation and returns the local id it was given.
pub(crate) fn record_created(context: u64, location: &'static Location<'static>) -> u64 {
    CREATED.fetch_add(1, Ordering::Relaxed);
    let seq = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
    let recorded = with(|registry| {
        let forgotten = registry.push(Entry {
            seq,
            context,
            created_at_file: location.file(),
            created_at_line: location.line(),
            created: Instant::now(),
            outcome: Outcome::Outstanding,
            polled: false,
        });
        DROPPED.fetch_add(forgotten, Ordering::Relaxed);
    });
    if recorded.is_none() {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
    seq
}

pub(crate) fn record_polled(seq: u64) {
    let _ = with(|registry| {
        if let Some(entry) = registry.entries.iter_mut().find(|entry| entry.seq == seq) {
            entry.polled = true;
        }
    });
}

pub(crate) fn record_outcome(seq: u64, outcome: Outcome) {
    let _ = with(|registry| {
        if let Some(entry) = registry.entries.iter_mut().find(|entry| entry.seq == seq) {
            // First terminal outcome wins: a sender dropped after sending is
            // not a second event.
            if entry.outcome == Outcome::Outstanding {
                entry.outcome = outcome;
            }
        }
    });
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RegistryStats {
    pub created: u64,
    pub dropped: u64,
    pub retained: usize,
    pub outstanding: usize,
    pub capacity: usize,
}

pub fn stats() -> RegistryStats {
    let (retained, outstanding, capacity) = with(|registry| {
        (
            registry.entries.len(),
            registry
                .entries
                .iter()
                .filter(|entry| entry.outcome == Outcome::Outstanding)
                .count(),
            registry.capacity,
        )
    })
    .unwrap_or((0, 0, configured_capacity()));
    RegistryStats {
        created: CREATED.load(Ordering::Relaxed),
        dropped: DROPPED.load(Ordering::Relaxed),
        retained,
        outstanding,
        capacity,
    }
}

/// Retained entries, oldest first — the order a reader wants, because the
/// oldest outstanding continuation is usually the bug.
///
/// `None` when the registry was locked; report `busy` rather than an empty
/// list.
pub fn entries() -> Option<Vec<Entry>> {
    with(|registry| registry.entries.iter().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(seq: u64, outcome: Outcome) -> Entry {
        Entry {
            seq,
            context: 0,
            created_at_file: "test",
            created_at_line: 1,
            created: Instant::now(),
            outcome,
            polled: false,
        }
    }

    fn registry(capacity: usize) -> Registry {
        Registry {
            capacity,
            entries: VecDeque::new(),
        }
    }

    /// Overflow forgets a *settled* entry before an outstanding one: an
    /// outstanding continuation is the whole reason to look.
    #[test]
    fn overflow_forgets_settled_before_outstanding() {
        let mut registry = registry(2);
        assert_eq!(registry.push(entry(1, Outcome::Outstanding)), 0);
        assert_eq!(registry.push(entry(2, Outcome::Sent)), 0);
        assert_eq!(registry.push(entry(3, Outcome::Outstanding)), 1);

        let ids: Vec<u64> = registry.entries.iter().map(|entry| entry.seq).collect();
        assert_eq!(ids, vec![1, 3], "the settled entry went first");
    }

    /// When everything is outstanding it still drops the oldest, and says how
    /// many — a drop must be counted, not implied.
    #[test]
    fn overflow_of_all_outstanding_drops_the_oldest_and_counts_it() {
        let mut registry = registry(2);
        registry.push(entry(1, Outcome::Outstanding));
        registry.push(entry(2, Outcome::Outstanding));
        assert_eq!(registry.push(entry(3, Outcome::Outstanding)), 1);

        let ids: Vec<u64> = registry.entries.iter().map(|entry| entry.seq).collect();
        assert_eq!(ids, vec![2, 3]);
    }

    /// A capacity of one still works, and every insert past the first forgets
    /// exactly one.
    #[test]
    fn a_capacity_of_one_keeps_only_the_newest() {
        let mut registry = registry(1);
        registry.push(entry(1, Outcome::Outstanding));
        assert_eq!(registry.push(entry(2, Outcome::Outstanding)), 1);
        let ids: Vec<u64> = registry.entries.iter().map(|entry| entry.seq).collect();
        assert_eq!(ids, vec![2]);
    }
}
