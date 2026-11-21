//! \[DESIGN\] Row waits on the writer's own signal, in the order they began
//! (RULES.md D28, D442). A transaction that would change a row another
//! open transaction changed waits for that transaction to end. Each open
//! transaction has an end of its own (`Ending`), which the waiters for its
//! rows queue on, each with a ticket. When it ends, the waiters go on one at
//! a time in ticket order, each once the one before it has tried its write
//! again (`Turn`). So the first to wait writes first, and the others queue
//! behind it as its writer, rather than every waiter of a table waking at
//! every release of the table's lock. Each waiter sleeps on a signal of its
//! own, so a turn passed wakes the one waiter whose turn it is, not the
//! whole line (RULES.md D449).

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
    /// The waiters asleep, each by its ticket, with its own signal.
    asleep: Vec<(u64, Arc<Condvar>)>,
}

impl Queue {
    /// The turn passes to the next ticket.
    fn pass(&mut self) {
        self.serving = self.serving.saturating_add(1);
        self.settle();
    }

    /// The turn passes over the tickets given up, then wakes the waiter
    /// whose turn it is, if the transaction has ended and it is asleep.
    fn settle(&mut self) {
        while let Some(at) = self.left.iter().position(|&t| t == self.serving) {
            self.left.swap_remove(at);
            self.serving = self.serving.saturating_add(1);
        }
        if !self.ended {
            return;
        }
        if let Some((_, signal)) = self.asleep.iter().find(|&&(t, _)| t == self.serving) {
            signal.notify_one();
        }
    }

    /// `ticket`'s waiter, awake, no longer signalled.
    fn awake(&mut self, ticket: u64) {
        self.asleep.retain(|&(t, _)| t != ticket);
    }
}

/// The end of one open transaction, which the waiters for its rows queue
/// on.
#[derive(Default)]
pub(crate) struct Ending {
    queue: Mutex<Queue>,
}

impl Ending {
    /// The transaction has ended: its first waiter goes on.
    fn end(&self) {
        let mut queue = lock_builtin(&self.queue);
        queue.ended = true;
        queue.settle();
    }

    /// A ticket, the next in line.
    fn ticket(&self) -> u64 {
        let mut queue = lock_builtin(&self.queue);
        let ticket = queue.next;
        queue.next = queue.next.saturating_add(1);
        ticket
    }

    /// `ticket` given up before its turn, the turn passing over it.
    fn leave(&self, ticket: u64) {
        let mut queue = lock_builtin(&self.queue);
        queue.left.push(ticket);
        queue.settle();
    }
}

/// A waiter's turn to try its write again, the transaction it waited for
/// ended: until it is dropped the next waiter waits.
pub(crate) struct Turn(Arc<Ending>);

impl Drop for Turn {
    fn drop(&mut self) {
        lock_builtin(&self.0.queue).pass();
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
        // The ticket first, so a transaction counted as waiting
        // (`Transactions::waiting`) already holds its place in line.
        let ticket = ending.ticket();
        {
            let mut waits_for = lock(&self.shared.waits_for).inspect_err(|_| ending.leave(ticket))?;
            if super::locks::would_deadlock(&waits_for, self.id, &[writer]) {
                drop(waits_for);
                ending.leave(ticket);
                return Err(refused(format!(
                    "waiting for {what} would deadlock with another transaction (roll back to release this transaction's locks)"
                )));
            }
            waits_for.insert(self.id, vec![writer]);
        }
        let began = Instant::now();
        // [DESIGN] RULES.md D520: shown waiting, and for whom.
        self.stop.activity().waits_for_lock(what.to_string(), writer);
        let waited = self.queue_on((&ending, ticket), began).map_err(|e| if e.code == Code::Conflict { refused(e.msg.clone()) } else { e });
        self.stop.activity().woke();
        lock(&self.shared.waits_for)?.remove(&self.id);
        // [DESIGN] RULES.md D240: the time waited, for the client.
        self.stop.served().waited(began.elapsed());
        waited.map(|()| Some(Turn(ending)))
    }

    /// Waits on `ending` for the turn of `ticket`, looking now and then
    /// for a cancel or a timeout, which give the ticket up.
    fn queue_on(&self, (ending, ticket): (&Ending, u64), began: Instant) -> Result<()> {
        let signal = Arc::new(Condvar::new());
        let mut queue = lock_builtin(&ending.queue);
        queue.asleep.push((ticket, Arc::clone(&signal)));
        loop {
            if queue.ended && queue.serving == ticket {
                queue.awake(ticket);
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
                queue.awake(ticket);
                queue.left.push(ticket);
                queue.settle();
                return Err(e);
            }
            queue = signal.wait_timeout(queue, super::locks::STOP_POLL).unwrap_or_else(std::sync::PoisonError::into_inner).0;
        }
    }
}
