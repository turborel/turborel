//! \[DESIGN\] A row's claim by its line of waiters (RULES.md D456). Once a
//! transaction waits for a row, the row is claimed for the first waiter in
//! its line, until that waiter has written it or given up its place. A
//! writer that comes later finds the claim before it writes, and queues in
//! the same line behind the waiters already there, though the row's writer
//! has ended and the first waiter not yet woken to write. So no newcomer
//! writes a row in demand ahead of the transactions waiting for it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::{Ending, Queue, SHARDS, Seat};
use crate::catalog::QualifiedName;
use crate::hash::xxhash::{Xxh3Map, xxh3_64};
use crate::storage::RowId;
use crate::sync::lock_builtin;
use crate::txn::TxId;

/// A row's claim: the line its waiters queue in, and its first waiter.
#[derive(Clone)]
pub(crate) struct Claim {
    /// The transaction whose end the line is.
    pub(crate) owner: TxId,
    /// That end.
    pub(super) line: Arc<Ending>,
    /// The first waiter in it, whose write of the row comes next.
    pub(crate) first: TxId,
}

/// The claims of rows in demand, by row id, in as many shards as the ends
/// of open transactions have, as a row is claimed only while one of them
/// is waited for; how many there are, so a write finds none without
/// looking while no row is waited for.
pub(super) struct Claims {
    count: AtomicUsize,
    shards: Vec<Mutex<Xxh3Map<RowId, Vec<(QualifiedName, Claim)>>>>,
}

impl Default for Claims {
    fn default() -> Claims {
        Claims { count: AtomicUsize::new(0), shards: (0..SHARDS).map(|_| Mutex::new(Xxh3Map::default())).collect() }
    }
}

impl Claims {
    /// Runs `f` on the claim of row `id` of `table`, which it may set,
    /// replace or remove.
    fn with<T>(&self, (table, id): (&QualifiedName, RowId), f: impl FnOnce(&mut Option<Claim>) -> T) -> Option<T> {
        let at = usize::try_from(xxh3_64(&id.to_le_bytes()) % SHARDS).ok()?;
        let mut shard = lock_builtin(self.shards.get(at)?);
        let of_row = shard.entry(id).or_default();
        let held = of_row.iter().position(|(t, _)| t == table);
        let mut claim = held.map(|at| of_row.swap_remove(at).1);
        let had = claim.is_some();
        let done = f(&mut claim);
        match (had, claim) {
            (true, None) => {
                self.count.fetch_sub(1, Ordering::SeqCst);
            }
            (false, Some(claim)) => {
                self.count.fetch_add(1, Ordering::SeqCst);
                of_row.push((table.clone(), claim));
            }
            (true, Some(claim)) => of_row.push((table.clone(), claim)),
            (false, None) => {}
        }
        if of_row.is_empty() {
            shard.remove(&id);
        }
        Some(done)
    }

    /// The claim of row `id` of `table` that `tx` must queue behind: none
    /// if it has none, or `tx` is its first waiter or writes the row now.
    /// One whose first waiter has ended, as `open` tells, is dropped,
    /// unless it changed meanwhile; `open` is asked with no shard held.
    pub(super) fn of(&self, row: (&QualifiedName, RowId), tx: TxId, open: impl Fn(TxId) -> bool) -> Option<Claim> {
        if self.count.load(Ordering::SeqCst) == 0 {
            return None;
        }
        let claim = self.with(row, |held| held.clone()).flatten()?;
        if !open(claim.first) {
            self.with(row, |held| {
                if held.as_ref().is_some_and(|h| h.first == claim.first && Arc::ptr_eq(&h.line, &claim.line)) {
                    *held = None;
                }
            });
            return None;
        }
        Some(claim).filter(|c| c.owner != tx && c.first != tx)
    }

    /// Row `id` of `table` claimed by `claim`, unless it is claimed
    /// already.
    pub(super) fn claim(&self, row: (&QualifiedName, RowId), claim: Claim) {
        self.with(row, |held| {
            held.get_or_insert(claim);
        });
    }

    /// Row `id` of `table` claimed by `claim`, or by none.
    pub(super) fn set(&self, row: (&QualifiedName, RowId), claim: Option<Claim>) {
        self.with(row, |held| *held = claim);
    }

    /// `seat`, the first waiter of its row's claim in `line`, gives up its
    /// place there: the claim passes to the next waiter seated for the row
    /// in `queue`, the line's, or ends if there is none.
    pub(super) fn pass_on(&self, seat: &Seat, (line, queue): (&Arc<Ending>, &Queue)) {
        let (table, id) = &seat.row;
        self.with((table, *id), |held| {
            let Some(claim) = held.as_ref().filter(|c| c.first == seat.waiter && Arc::ptr_eq(&c.line, line)) else { return };
            let next = queue.first_for(&seat.row);
            *held = next.map(|first| Claim { owner: claim.owner, line: Arc::clone(line), first });
        });
    }
}
