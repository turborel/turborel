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
//! whole line (RULES.md D449). A waiter whose turn's write is made hands
//! the waiters behind it for the same row to its own end, unwoken, so a hot
//! row's line moves from writer to writer without each waiter waking to
//! find the next (RULES.md D743). A row waited for is claimed for the
//! first in its line, so a writer that comes later queues behind the line
//! rather than writing the row between its writer's end and the first
//! waiter's write (`claims.rs`, RULES.md D456).

mod claims;

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Instant;

use self::claims::{Claim, Claims};
use super::{LockWait, TxId, Txn};
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError};
use crate::storage::RowId;
use crate::sync::{lock, lock_builtin};

/// Shards of the registry of ends.
const SHARDS: u64 = 64;

/// Where a waiter stands in line: the end it queues on, its ticket there,
/// and whether its turn has come.
struct Place {
    ending: Arc<Ending>,
    ticket: u64,
    turn: bool,
}

/// A waiter's seat: the row it would change, its place, which a hand-over
/// moves, and the signal it sleeps on.
struct Seat {
    waiter: TxId,
    row: (QualifiedName, RowId),
    place: Mutex<Place>,
    signal: Condvar,
}

impl Seat {
    /// The seat gives up its place, the turn passing over it, wherever a
    /// hand-over has moved it meanwhile, and the claim of its row, if it
    /// holds it, passing to the next in line.
    fn leave(&self, claims: &Claims) {
        loop {
            let ending = Arc::clone(&lock_builtin(&self.place).ending);
            let mut queue = lock_builtin(&ending.queue);
            let place = lock_builtin(&self.place);
            if !Arc::ptr_eq(&place.ending, &ending) {
                continue;
            }
            let ticket = place.ticket;
            drop(place);
            queue.seated.retain(|&(t, _)| t != ticket);
            claims.pass_on(self, (&ending, &queue));
            queue.left.push(ticket);
            queue.settle();
            return;
        }
    }
}

/// Where an open transaction's waiters stand.
#[derive(Default)]
struct Queue {
    /// Whether it has ended.
    ended: bool,
    /// The next ticket to give.
    next: u64,
    /// The ticket whose turn it is, once it has ended.
    serving: u64,
    /// Tickets whose waiters stopped waiting or moved before their turn.
    left: Vec<u64>,
    /// The waiters seated, each by its ticket.
    seated: Vec<(u64, Arc<Seat>)>,
}

impl Queue {
    /// The waiter seated first for `row`, if any is.
    fn first_for(&self, row: &(QualifiedName, RowId)) -> Option<TxId> {
        self.seated.iter().filter(|(_, seat)| seat.row == *row).min_by_key(|&&(t, _)| t).map(|(_, seat)| seat.waiter)
    }

    /// A ticket, the next in line.
    fn ticket(&mut self) -> u64 {
        let ticket = self.next;
        self.next = self.next.saturating_add(1);
        ticket
    }

    /// The turn passes to the next ticket.
    fn pass(&mut self) {
        self.serving = self.serving.saturating_add(1);
        self.settle();
    }

    /// The turn passes over the tickets given up, then goes to the waiter
    /// whose turn it is, waking it, if the transaction has ended and it is
    /// seated.
    fn settle(&mut self) {
        while let Some(at) = self.left.iter().position(|&t| t == self.serving) {
            self.left.swap_remove(at);
            self.serving = self.serving.saturating_add(1);
        }
        if !self.ended {
            return;
        }
        if let Some(at) = self.seated.iter().position(|&(t, _)| t == self.serving) {
            let (_, seat) = self.seated.swap_remove(at);
            lock_builtin(&seat.place).turn = true;
            seat.signal.notify_one();
        }
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

    /// `ticket` given up before its turn, the turn passing over it.
    fn leave(&self, ticket: u64) {
        let mut queue = lock_builtin(&self.queue);
        queue.left.push(ticket);
        queue.settle();
    }
}

/// A waiter's turn to try its write again, the transaction it waited for
/// ended: until it is dropped the next waiter waits. Its seat, whose row's
/// claim passes on as it is dropped, unless its write moved it.
pub(crate) struct Turn {
    ending: Arc<Ending>,
    seat: Arc<Seat>,
    claims: Arc<Claims>,
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut queue = lock_builtin(&self.ending.queue);
        self.claims.pass_on(&self.seat, (&self.ending, &queue));
        queue.pass();
    }
}

impl Turn {
    /// \[DESIGN\] RULES.md D743: `row`, written by `tx` in `turn`, if it
    /// had one, has `tx` its writer, so the waiters behind for the same row
    /// queue on `tx`'s end, in their order and unwoken, as each would on
    /// waking to find it. One whose wait on `tx` would deadlock stays, to
    /// wake and find that itself.
    pub(crate) fn hand_over(turn: Option<&Turn>, tx: &Txn, (table, id): (&QualifiedName, RowId)) {
        let Some(turn) = turn else { return };
        let Some(next) = tx.shared.ends.of(tx.id) else { return };
        let Ok(mut waits_for) = lock(&tx.shared.waits_for) else { return };
        let mut from = lock_builtin(&turn.ending.queue);
        let mut to = lock_builtin(&next.queue);
        if to.ended {
            return;
        }
        let moves =
            |seat: &Seat| seat.row.1 == id && seat.row.0 == *table && !super::locks::would_deadlock(&waits_for, seat.waiter, &[tx.id]);
        let (mut moving, staying): (Vec<_>, Vec<_>) = std::mem::take(&mut from.seated).into_iter().partition(|(_, seat)| moves(seat));
        from.seated = staying;
        moving.sort_unstable_by_key(|&(t, _)| t);
        for (old, seat) in moving {
            from.left.push(old);
            let ticket = to.ticket();
            {
                let mut place = lock_builtin(&seat.place);
                place.ending = Arc::clone(&next);
                place.ticket = ticket;
            }
            waits_for.insert(seat.waiter, vec![tx.id]);
            to.seated.push((ticket, seat));
        }
        // [DESIGN] RULES.md D456: the row's claim, to the first moved.
        let claim = to.first_for(&(table.clone(), id)).map(|first| Claim { owner: tx.id, line: Arc::clone(&next), first });
        tx.shared.ends.claims.set((table, id), claim);
    }
}

/// The ends of open transactions, by id, in shards.
pub(super) struct Ends {
    shards: Vec<Mutex<HashMap<TxId, Arc<Ending>>>>,
    /// The claims of rows waited for (RULES.md D456).
    claims: Arc<Claims>,
}

impl Default for Ends {
    fn default() -> Ends {
        Ends { shards: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(), claims: Arc::default() }
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
    /// changed row `id` of `table` this one would change, to end, and for
    /// the waiters before it to have tried again (RULES.md D28, D29): its
    /// turn, which the next waiter waits on until it is dropped once this
    /// one has tried its write, or none if `writer` had already ended. A
    /// hand-over may move it meanwhile to the row's next writer (D743).
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if locking is no-wait, waiting would deadlock, or lasts
    /// past `LOCKS.TIMEOUT_MS`; SQLCODE -920 if its statement is cancelled.
    pub(crate) fn wait_for_row(&self, row: (&QualifiedName, RowId), writer: TxId) -> Result<Option<Turn>> {
        let Some(ending) = self.shared.ends.of(writer) else { return Ok(None) };
        self.wait_in_line(row, (ending, writer)).map(Some)
    }

    /// \[DESIGN\] RULES.md D456: the claim of row `id` of `table` by
    /// another transaction's line, which this one queues behind before it
    /// writes the row, if there is one.
    pub(crate) fn claim_of(&self, row: (&QualifiedName, RowId)) -> Option<Claim> {
        let ends = &self.shared.ends;
        ends.claims.of(row, self.id, |tx| ends.of(tx).is_some())
    }

    /// \[DESIGN\] RULES.md D456: waits behind the waiters of `claim`'s
    /// line for row `id` of `table`, for its turn as `wait_for_row` does.
    ///
    /// # Errors
    ///
    /// As `wait_for_row`'s.
    pub(crate) fn wait_behind(&self, row: (&QualifiedName, RowId), claim: &Claim) -> Result<Turn> {
        self.wait_in_line(row, (Arc::clone(&claim.line), claim.owner))
    }

    /// Waits in `ending`'s line, that of transaction `writer`, for row `id`
    /// of `table`: the row claimed for this one if no line claims it
    /// (RULES.md D456). It waits for `writer` alone in the graph, as every
    /// waiter in a line does, the turns before its own not counted: one
    /// counting the first waiter would keep that edge once the claim
    /// passed on, and find a cycle that is not there.
    fn wait_in_line(&self, (table, id): (&QualifiedName, RowId), (ending, writer): (Arc<Ending>, TxId)) -> Result<Turn> {
        let refused = |why: String| {
            SqlError::new(Code::Conflict, format!("[DESIGN] {why}, so the statement is rejected and undone [X3.135-1986 §7.3 GR3b]"))
        };
        if self.lock_wait == LockWait::NoWait {
            return Err(refused(format!("a row of {table} is locked by another transaction; locking is no-wait")));
        }
        // The ticket first, so a transaction counted as waiting
        // (`Transactions::waiting`) already holds its place in line.
        let ticket = lock_builtin(&ending.queue).ticket();
        {
            let mut waits_for = lock(&self.shared.waits_for).inspect_err(|_| ending.leave(ticket))?;
            if super::locks::would_deadlock(&waits_for, self.id, &[writer]) {
                drop(waits_for);
                ending.leave(ticket);
                return Err(refused(format!(
                    "waiting for a row of {table} would deadlock with another transaction (roll back to release this transaction's locks)"
                )));
            }
            waits_for.insert(self.id, vec![writer]);
        }
        let claim = Claim { owner: writer, line: Arc::clone(&ending), first: self.id };
        self.shared.ends.claims.claim((table, id), claim);
        let began = Instant::now();
        // [DESIGN] RULES.md D520: shown waiting, and for whom.
        self.stop.activity().waits_for_lock(format!("a row of {table}"), writer);
        let seat = Arc::new(Seat {
            waiter: self.id,
            row: (table.clone(), id),
            place: Mutex::new(Place { ending, ticket, turn: false }),
            signal: Condvar::new(),
        });
        let waited = self.queue_on(&seat, began).map_err(|e| if e.code == Code::Conflict { refused(e.msg.clone()) } else { e });
        self.stop.activity().woke();
        lock(&self.shared.waits_for)?.remove(&self.id);
        // [DESIGN] RULES.md D240: the time waited, for the client.
        self.stop.served().waited(began.elapsed());
        waited
    }

    /// Waits in `seat` for its turn, looking now and then for a cancel or
    /// a timeout, which give the seat up.
    fn queue_on(&self, seat: &Arc<Seat>, began: Instant) -> Result<Turn> {
        // The seat's lock before its queue's only here, as no one else
        // knows the seat until it is seated.
        let mut place = lock_builtin(&seat.place);
        {
            let mut queue = lock_builtin(&place.ending.queue);
            if queue.ended && queue.serving == place.ticket {
                return Ok(self.turn(&place.ending, seat));
            }
            queue.seated.push((place.ticket, Arc::clone(seat)));
        }
        loop {
            if place.turn {
                return Ok(self.turn(&place.ending, seat));
            }
            if let Some(e) = self.stopped_waiting(began) {
                drop(place);
                seat.leave(&self.shared.ends.claims);
                return Err(e);
            }
            place = seat.signal.wait_timeout(place, super::locks::STOP_POLL).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    /// The turn of `seat` in `ending`'s line.
    fn turn(&self, ending: &Arc<Ending>, seat: &Arc<Seat>) -> Turn {
        Turn { ending: Arc::clone(ending), seat: Arc::clone(seat), claims: Arc::clone(&self.shared.ends.claims) }
    }

    /// Why a row wait begun at `began` stops before its turn: a cancel,
    /// or `LOCKS.TIMEOUT_MS` passed.
    fn stopped_waiting(&self, began: Instant) -> Option<SqlError> {
        self.stop.check_now().err().or_else(|| {
            self.stop.lock_timeout().filter(|&most| began.elapsed() >= most).map(|most| {
                SqlError::new(
                    Code::Conflict,
                    format!("a row stayed locked by another transaction past LOCKS.TIMEOUT_MS, {} ms", most.as_millis()),
                )
            })
        })
    }
}

#[cfg(test)]
mod tests;
