//! \[DESIGN\] Group commit (RULES.md D25, D441, D740): the commits whose
//! records wait for a sync gather in one queue, each with a ticket of its
//! own. The first to come while no sync runs leads: it syncs the log
//! through the last record queued, and the sync's reach covers every
//! record appended before it began. It then marks each commit covered
//! durable in one pass over the clock, and wakes those commits one ticket
//! at a time. The commits that came while it synced, and were not covered,
//! are the next group: the first of them is woken to lead, and syncs for
//! all of them at once. No other thread wakes, so a group's waiters never
//! crowd onto one lock as a shared condition variable's would.
//!
//! The queue's lock is held only to add a commit or take a group, never
//! during the sync, so appends, commits and their waits go on while the
//! log syncs. A failed sync stops the database's changes (`logging.rs`):
//! every commit queued is woken and told, none marked durable.

use std::mem;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, Thread};
use std::time::Duration;

use super::{Timestamp, Transactions};
use crate::error::{Code, Result, SqlError};
use crate::log::{Log, Lsn};
use crate::sync::lock_builtin;

/// A ticket's state: waiting for its group's sync.
const WAITING: u8 = 0;
/// Woken to lead the next group.
const LEAD: u8 = 1;
/// Its commit record is durable.
const SYNCED: u8 = 2;
/// The sync failed.
const FAILED: u8 = 3;

/// \[DESIGN\] How long a waiting commit sleeps before it looks at its
/// ticket again, should a wake-up be missed: none is, as a ticket is set
/// before its thread is woken, so this only bounds a defect's cost.
const PARKED: Duration = Duration::from_millis(10);

/// One commit's ticket: its state, and the thread to wake as it changes.
struct Ticket {
    state: AtomicU8,
    thread: Thread,
}

impl Ticket {
    /// A ticket of the calling thread, waiting.
    fn mine() -> Arc<Ticket> {
        Arc::new(Ticket { state: AtomicU8::new(WAITING), thread: thread::current() })
    }

    /// Sets the ticket's state, then wakes its thread: a thread woken
    /// before it looks finds the state set, and one woken after has a wake
    /// waiting, so none sleeps past its change.
    fn set(&self, state: u8) {
        self.state.store(state, Ordering::Release);
        self.thread.unpark();
    }

    /// Waits until the ticket leaves `WAITING`: its state then.
    fn wait(&self) -> u8 {
        loop {
            let state = self.state.load(Ordering::Acquire);
            if state != WAITING {
                return state;
            }
            thread::park_timeout(PARKED);
        }
    }
}

/// A commit queued: its log, its commit record's position, its commit
/// timestamp, and its ticket.
struct Queued {
    log: Arc<dyn Log>,
    lsn: Lsn,
    at: Timestamp,
    ticket: Arc<Ticket>,
}

/// The commits waiting for a sync, and whether a leader syncs for them.
#[derive(Default)]
pub(super) struct Group {
    queue: Mutex<Queue>,
}

#[derive(Default)]
struct Queue {
    queued: Vec<Queued>,
    leading: bool,
}

/// Whether `a` and `b` are one log.
fn same(a: &Arc<dyn Log>, b: &Arc<dyn Log>) -> bool {
    Arc::ptr_eq(a, b)
}

impl Transactions {
    /// Syncs the commit record at `lsn` of `log`, of the commit at `at`,
    /// in a group (`group.rs`): returns once it is durable, the commit
    /// then marked so on the clock.
    ///
    /// # Errors
    ///
    /// As `taking_changes`; the log's, if the group's sync failed.
    pub(super) fn sync_in_group(&self, log: &Arc<dyn Log>, lsn: Lsn, at: Timestamp) -> Result<()> {
        self.taking_changes()?;
        let ticket = Ticket::mine();
        let lead = {
            let mut queue = lock_builtin(&self.group.queue);
            queue.queued.push(Queued { log: Arc::clone(log), lsn, at, ticket: Arc::clone(&ticket) });
            !mem::replace(&mut queue.leading, true)
        };
        let mut state = if lead { LEAD } else { ticket.wait() };
        while state == LEAD {
            self.lead();
            state = ticket.wait();
        }
        if state == SYNCED {
            return Ok(());
        }
        Err(self.stopped_by().unwrap_or_else(|| SqlError::new(Code::Io, "[DESIGN] the group's sync of the log failed")))
    }

    /// Leads a group: syncs the log of the first commit queued through the
    /// last record of that log queued, marks the commits covered durable
    /// and wakes them, and wakes the first of those left to lead the next.
    fn lead(&self) {
        let Some((log, through)) = self.group_through() else {
            // None queued: none to lead, and the next to come leads.
            lock_builtin(&self.group.queue).leading = false;
            return;
        };
        let reached = self.sync_reaching(&*log, through);
        let (covered, next) = {
            let mut queue = lock_builtin(&self.group.queue);
            let reach = reached.as_ref().map_or(Lsn(u64::MAX), |r| *r);
            let (covered, left): (Vec<Queued>, Vec<Queued>) =
                mem::take(&mut queue.queued).into_iter().partition(|q| same(&q.log, &log) && q.lsn < reach);
            queue.queued = left;
            let next = queue.queued.first().map(|q| Arc::clone(&q.ticket));
            queue.leading = next.is_some();
            (covered, next)
        };
        let state = if reached.is_ok() {
            self.clock.end_syncs(covered.iter().map(|q| q.at));
            SYNCED
        } else {
            FAILED
        };
        for queued in covered {
            queued.ticket.set(state);
        }
        if let Some(next) = next {
            next.set(LEAD);
        }
    }

    /// The log of the first commit queued, and the last position of a
    /// commit record of it queued: what the group's sync goes through.
    fn group_through(&self) -> Option<(Arc<dyn Log>, Lsn)> {
        let queue = lock_builtin(&self.group.queue);
        let log = Arc::clone(&queue.queued.first()?.log);
        let through = queue.queued.iter().filter(|q| same(&q.log, &log)).map(|q| q.lsn).max()?;
        Some((log, through))
    }
}

// Unit tests: grouping, the hand-over to the next leader, and a failed
// sync (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use std::sync::Condvar;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    use super::*;
    use crate::log::{LogCapabilities, MemoryLog, Records};

    /// A log in memory whose syncs are counted, wait while it is held, and
    /// fail while it is failing.
    #[derive(Default)]
    struct Gated {
        inner: MemoryLog,
        held: Mutex<bool>,
        opened: Condvar,
        syncs: AtomicUsize,
        failing: AtomicBool,
    }

    impl Gated {
        fn hold(&self, held: bool) {
            *lock_builtin(&self.held) = held;
            self.opened.notify_all();
        }
    }

    impl Log for Gated {
        fn capabilities(&self) -> LogCapabilities {
            LogCapabilities { durable: true }
        }
        fn append(&self, record: &[u8]) -> Result<Lsn> {
            self.inner.append(record)
        }
        fn sync(&self, through: Lsn) -> Result<()> {
            self.syncs.fetch_add(1, Ordering::SeqCst);
            let mut held = lock_builtin(&self.held);
            while *held {
                held = self.opened.wait(held).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if self.failing.load(Ordering::SeqCst) {
                return Err(SqlError::new(Code::Io, "[DESIGN] failing"));
            }
            self.inner.sync(through)
        }
        fn read_from(&self, from: Lsn) -> Records<'_> {
            self.inner.read_from(from)
        }
        fn truncate_before(&self, before: Lsn) -> Result<()> {
            self.inner.truncate_before(before)
        }
        fn end(&self) -> Lsn {
            self.inner.end()
        }

        fn bytes_between(&self, from: Lsn, to: Lsn) -> u64 {
            self.inner.bytes_between(from, to)
        }
    }

    /// Three commits' records in `log`, and their commit timestamps, ended
    /// and not durable.
    fn three(shared: &Transactions, log: &Arc<dyn Log>) -> Vec<(Lsn, Timestamp)> {
        (0..3)
            .map(|_| {
                let at = shared.clock.take_commit();
                shared.clock.end_commit(at);
                (log.append(b"commit").unwrap(), at)
            })
            .collect()
    }

    /// The three commits synced in a group, the first syncing alone while
    /// the other two queue behind it: what each was told.
    fn grouped(shared: &Transactions, gated: &Arc<Gated>) -> Vec<Result<()>> {
        let log: Arc<dyn Log> = Arc::<Gated>::clone(gated);
        let commits = three(shared, &log);
        gated.hold(true);
        thread::scope(|scope| {
            let (log, commits) = (&log, &commits);
            let sync = move |i: usize| move || commits.get(i).map(|&(lsn, at)| shared.sync_in_group(log, lsn, at)).unwrap();
            let first = scope.spawn(sync(0));
            while gated.syncs.load(Ordering::SeqCst) == 0 {
                thread::yield_now();
            }
            let rest = [scope.spawn(sync(1)), scope.spawn(sync(2))];
            while lock_builtin(&shared.group.queue).queued.len() < 3 {
                thread::yield_now();
            }
            assert_eq!(shared.clock.durable(), 0, "none synced yet");
            gated.hold(false);
            let mut told = vec![first.join().unwrap()];
            told.extend(rest.map(|r| r.join().unwrap()));
            told
        })
    }

    /// Verifies: [DESIGN] RULES.md D441, D740: the first commit leads
    /// and syncs alone; the two that queue while it syncs are woken to a
    /// second sync, one leading for both: three commits, two syncs, all
    /// durable, and none left queued or leading.
    #[test]
    fn commits_that_come_during_a_sync_share_the_next() {
        let shared = Transactions::default();
        let gated = Arc::new(Gated::default());
        let told = grouped(&shared, &gated);
        assert!(told.iter().all(Result::is_ok), "{told:?}");
        assert_eq!(gated.syncs.load(Ordering::SeqCst), 2);
        assert_eq!(shared.clock.durable(), 3);
        let queue = lock_builtin(&shared.group.queue);
        assert!(queue.queued.is_empty() && !queue.leading);
    }

    /// Verifies: [DESIGN] RULES.md D25, D441, D740: a failed sync fails
    /// every commit it was to make durable, none marked so, and the
    /// database takes no change after; the commits queued behind it fail
    /// with it, never claimed durable by a later sync.
    #[test]
    fn a_failed_sync_fails_its_group_and_the_next() {
        let shared = Transactions::default();
        let gated = Arc::new(Gated::default());
        gated.failing.store(true, Ordering::SeqCst);
        let told = grouped(&shared, &gated);
        assert!(told.iter().all(Result::is_err), "{told:?}");
        assert_eq!(shared.clock.durable(), 0);
        assert!(shared.taking_changes().is_err());
        assert!(!lock_builtin(&shared.group.queue).leading);
    }
}
