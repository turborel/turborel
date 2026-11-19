//! \[EXTENSION\] A plug-in's objects that serve the database as a whole
//! (RULES.md D222): what makes users' verifiers (`Credentials`), which
//! an extension's state puts in place of the host's; and a sink of the
//! database's events (`Diagnostics`), beside the host's.
//!
//! A sink's events go to a queue, at most `QUEUE` of them, never waiting
//! on the program: the database tells events under locks, where a call
//! may not be made (R-ERR-4). A thread of the sink's own hands its
//! program the queue a batch at a time; events past a full queue, or of
//! a call that failed, are counted as dropped and the count told with
//! the next batch.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::db::Credentials;
use crate::diagnostics::{Diagnostics, Event, Level};
use crate::error::Result;
use crate::plugin::payload::process::{Told, put_events, put_verifier};
use crate::plugin::payload::{CREDENTIALS_VERIFIER, DIAGNOSTICS_EVENTS};
use crate::sync::lock_builtin;

use super::{Link, answer, leaked};

/// \[DESIGN\] The events a sink holds unsent at most; past them, dropped.
pub const QUEUE: usize = 4096;
/// The events a call carries when the manifest says no BATCH.
pub const DEFAULT_BATCH: usize = 256;
/// \[DESIGN\] How long, in milliseconds, a sink's thread waits for an event
/// before it looks whether its sink was dropped.
const WAIT_MS: u64 = 100;

/// A plug-in's maker of verifiers.
#[derive(Debug)]
pub struct PluginCredentials {
    link: Link,
    name: &'static str,
}

impl PluginCredentials {
    /// The credentials `link` names.
    pub fn new(link: Link) -> PluginCredentials {
        let name = leaked(&link.object);
        PluginCredentials { link, name }
    }
}

impl Credentials for PluginCredentials {
    fn name(&self) -> &'static str {
        self.name
    }
    fn verifier(&self, user: &str, password: &str) -> Result<Vec<u8>> {
        let mut payload = Vec::new();
        put_verifier(&mut payload, user, password);
        answer(&self.link.plain(CREDENTIALS_VERIFIER, payload)?, |r| r.bytes().map(<[u8]>::to_vec))
    }
}

/// What a sink and its thread share.
#[derive(Default)]
struct Queue {
    held: Mutex<VecDeque<Told>>,
    told: Condvar,
    dropped: AtomicU64,
    sent: AtomicU64,
    gone: AtomicBool,
}

/// A plug-in's sink of events.
pub struct PluginDiagnostics {
    name: &'static str,
    queue: Arc<Queue>,
}

impl std::fmt::Debug for PluginDiagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginDiagnostics").field("name", &self.name).finish_non_exhaustive()
    }
}

/// The word of `level`.
fn word(level: Level) -> &'static str {
    match level {
        Level::Error => Level::WORDS[0],
        Level::Warning => Level::WORDS[1],
        Level::Info => Level::WORDS[2],
    }
}

impl PluginDiagnostics {
    /// The sink `link` names, its thread started, `batch` events a call.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if its thread cannot be started.
    pub fn new(link: Link, batch: Option<u32>) -> Result<PluginDiagnostics> {
        let name = leaked(&link.object);
        let queue = Arc::new(Queue::default());
        let batch = batch.and_then(|b| usize::try_from(b).ok()).unwrap_or(DEFAULT_BATCH).max(1);
        let shared = Arc::clone(&queue);
        crate::threads::plugin_sender(&link.named(), move || send(&shared, &link, batch))?;
        Ok(PluginDiagnostics { name, queue })
    }

    /// The events sent and dropped so far.
    pub fn counts(&self) -> (u64, u64) {
        (self.queue.sent.load(Ordering::Relaxed), self.queue.dropped.load(Ordering::Relaxed))
    }
}

/// The thread's work: each batch held handed to the program, until the
/// sink is dropped.
fn send(queue: &Queue, link: &Link, batch: usize) {
    while !queue.gone.load(Ordering::Relaxed) {
        let taken: Vec<Told> = {
            let held = lock_builtin(&queue.held);
            let (mut held, _) = queue
                .told
                .wait_timeout_while(held, Duration::from_millis(WAIT_MS), |h| h.is_empty())
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let n = held.len().min(batch);
            held.drain(..n).collect()
        };
        if taken.is_empty() {
            continue;
        }
        let dropped = queue.dropped.swap(0, Ordering::Relaxed);
        let mut payload = Vec::new();
        put_events(&mut payload, &taken, dropped);
        let n = u64::try_from(taken.len()).unwrap_or(u64::MAX);
        match link.plain(DIAGNOSTICS_EVENTS, payload) {
            Ok(_) => drop(queue.sent.fetch_add(n, Ordering::Relaxed)),
            Err(_) => drop(queue.dropped.fetch_add(n.saturating_add(dropped), Ordering::Relaxed)),
        }
    }
}

impl Drop for PluginDiagnostics {
    fn drop(&mut self) {
        self.queue.gone.store(true, Ordering::Relaxed);
        self.queue.told.notify_all();
    }
}

impl Diagnostics for PluginDiagnostics {
    fn name(&self) -> &'static str {
        self.name
    }
    fn event(&self, event: &Event) {
        let text = format!("{event:?}");
        let kind = text.split(['(', ' ', '{']).next().unwrap_or_default().to_owned();
        let told = Told { level: word(event.level()).to_owned(), kind, text };
        let mut held = lock_builtin(&self.queue.held);
        if held.len() >= QUEUE {
            self.queue.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        held.push_back(told);
        drop(held);
        self.queue.told.notify_one();
    }
}
