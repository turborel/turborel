//! \[DESIGN\] Row waits on the writer's own signal, in the order they began
//! (RULES.md D28, D442). A transaction that would change a row another
//! open transaction changed waits for that transaction to end. Each open
//! transaction has an end of its own (`Ending`), which the waiters for its
//! rows queue on, each with a ticket. When it ends, the waiters go on one at
//! a time in ticket order, each once the one before it has tried its write
//! again (`Turn`). So the first to wait writes first, and the others queue
//! behind it as its writer, rather than every waiter of a table waking at
//! every release of the table's lock.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use super::{LockWait, TxId, Txn};
use crate::error::{Code, Result, SqlError};
use crate::sync::{lock, lock_builtin};

/// Shards of the registry of ends.
const SHARDS: u64 = 64;

/// Where an open transaction's waiters stand.
#[derive(Default)]
struct Queue {
    /// Whether it has ended.
    ended: bool,
    /// The next ticket to give.
    next: u64,
    /// The ticket whose turn it is, once it has ended.
    serving: u64,
    /// Tickets whose waiters stopped waiting before their turn.
    left: Vec<u64>,
}

impl Queue {
    /// The turn passes to the next ticket.
    fn pass(&mut self) {
        self.serving = self.serving.saturating_add(1);
        self.settle();
    }

    /// The turn passes over the tickets given up.
    fn settle(&mut self) {
        while let Some(at) = self.left.iter().position(|&t| t == self.serving) {
            self.left.swap_remove(at);
            self.serving = self.serving.saturating_add(1);
        }
    }
}

/// The end of one open transaction, which the waiters for its rows queue
/// on.
#[derive(Default)]
pub(crate) struct Ending {
    queue: Mutex<Queue>,
    turned: Condvar,
}

impl Ending {
    /// The transaction has ended: its first waiter goes on.
    fn end(&self) {
        let mut queue = lock_builtin(&self.queue);
        queue.ended = true;
        queue.settle();
        drop(queue);
        self.turned.notify_all();
    }
}

/// A waiter's turn to try its write again, the transaction it waited for
/// ended: until it is dropped the next waiter waits.
pub(crate) struct Turn(Arc<Ending>);

impl Drop for Turn {
    fn drop(&mut self) {
        lock_builtin(&self.0.queue).pass();
        self.0.turned.notify_all();
    }
}

/// The ends of open transactions, by id, in shards.
pub(super) struct Ends {
    shards: Vec<Mutex<HashMap<TxId, Arc<Ending>>>>,
}

impl Default for Ends {
    fn default() -> Ends {
        Ends { shards: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect() }
    }
}

impl Ends {
    fn shard(&self, tx: TxId) -> Option<&Mutex<HashMap<TxId, Arc<Ending>>>> {
        usize::try_from(tx % SHARDS).ok().and_then(|at| self.shards.get(at))
    }

    /// `tx` began: its end, for its waiters.
    pub(super) fn begin(&self, tx: TxId) {
        if let Some(shard) = self.shard(tx) {
            lock_builtin(shard).insert(tx, Arc::default());
        }
    }

    /// `tx` ended: its waiters go on, in turn.
    pub(super) fn end(&self, tx: TxId) {
        let ending = self.shard(tx).and_then(|shard| lock_builtin(shard).remove(&tx));
        if let Some(ending) = ending {
            ending.end();
        }
    }

    /// The end of `tx`, if it is open.
    fn of(&self, tx: TxId) -> Option<Arc<Ending>> {
        self.shard(tx).and_then(|shard| lock_builtin(shard).get(&tx).cloned())
    }
}

impl Txn {
    /// [ISO/IEC 9075:1992 §4.28] Waits for transaction `writer`, which
    /// changed a row of `what` this one would change, to end, and for the
    /// waiters before it to have tried again (RULES.md D28, D29): its
    /// turn, which the next waiter waits on until it is dropped once this
    /// one has tried its write, or none if `writer` had already ended.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if locking is no-wait, waiting would deadlock, or lasts
    /// past `LOCKS.TIMEOUT_MS`; SQLCODE -920 if its statement is cancelled.
    pub(crate) fn wait_for_row(&self, what: &dyn std::fmt::Display, writer: TxId) -> Result<Option<Turn>> {
        let Some(ending) = self.shared.ends.of(writer) else { return Ok(None) };
        let refused = |why: String| {
            SqlError::new(Code::Conflict, format!("[DESIGN] {why}, so the statement is rejected and undone [X3.135-1986 §7.3 GR3b]"))
        };
        if self.lock_wait == LockWait::NoWait {
            return Err(refused(format!("{what} is locked by another transaction; locking is no-wait")));
        }
        {
            let mut waits_for = lock(&self.shared.waits_for)?;
            if super::locks::would_deadlock(&waits_for, self.id, &[writer]) {
                return Err(refused(format!(
                    "waiting for {what} would deadlock with another transaction (roll back to release this transaction's locks)"
                )));
            }
            waits_for.insert(self.id, vec![writer]);
        }
        let began = Instant::now();
        let waited = self.queue_on(&ending, began).map_err(|e| if e.code == Code::Conflict { refused(e.msg.clone()) } else { e });
        lock(&self.shared.waits_for)?.remove(&self.id);
        // [DESIGN] RULES.md D240: the time waited, for the client.
        self.stop.served().waited(began.elapsed());
        waited.map(|()| Some(Turn(ending)))
    }

    /// Takes a ticket on `ending` and waits for its turn, looking now and
    /// then for a cancel or a timeout, which give the ticket up.
    fn queue_on(&self, ending: &Ending, began: Instant) -> Result<()> {
        let mut queue = lock_builtin(&ending.queue);
        let ticket = queue.next;
        queue.next = queue.next.saturating_add(1);
        loop {
            if queue.ended && queue.serving == ticket {
                return Ok(());
            }
            let stopped = self.stop.check_now().err().or_else(|| {
                self.stop.lock_timeout().filter(|&most| began.elapsed() >= most).map(|most| {
                    SqlError::new(
                        Code::Conflict,
                        format!("a row stayed locked by another transaction past LOCKS.TIMEOUT_MS, {} ms", most.as_millis()),
                    )
                })
            });
            if let Some(e) = stopped {
                queue.left.push(ticket);
                queue.settle();
                drop(queue);
                ending.turned.notify_all();
                return Err(e);
            }
            queue = ending.turned.wait_timeout(queue, super::locks::STOP_POLL).unwrap_or_else(std::sync::PoisonError::into_inner).0;
        }
    }
}
