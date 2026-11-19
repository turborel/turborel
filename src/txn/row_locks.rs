//! \[EXTENSION\] Row locks, of `FOR UPDATE` and `FOR SHARE` (RULES.md
//! D185). A query that locks its rows takes each lock as it reads the row,
//! after its sort and before its row limit, so it locks only the rows it
//! gives. A lock FOR UPDATE excludes every other lock of the row and every
//! other transaction's change to it; a lock FOR SHARE excludes locks FOR
//! UPDATE and changes. Each is held to its transaction's end.
//!
//! The locks of a table's rows are kept in `ROW_STRIPES` stripes, by row
//! id, so lockers and writers of different rows seldom meet. A writer of a
//! row holds its stripe while it writes, and a locker holds it while it
//! looks for a change pending to the row. So either the locker sees the
//! writer's change, and waits for it, or the writer sees the lock, and
//! waits for its transaction to end.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::ThreadId;

use super::waiting::{Waiter, Waits, not_available, runs_here, wait_for};
use super::{TableLock, Timestamp, TxId, Txn};
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError};
use crate::storage::{RowId, TableStorage};
use crate::sync::{Signal, lock, lock_builtin};

/// \[DESIGN\] The stripes a table's row locks are kept in.
const ROW_STRIPES: u64 = 64;

/// The holders of each locked row of a stripe.
pub(crate) type Held = HashMap<RowId, Vec<RowHolder>>;

/// One transaction holding a row's lock.
pub(crate) struct RowHolder {
    tx: TxId,
    /// FOR UPDATE, else FOR SHARE.
    exclusive: bool,
    running: Arc<Mutex<Option<ThreadId>>>,
}

/// Some of a table's row locks, and the signal that one was released.
#[derive(Default)]
struct Stripe {
    held: Mutex<Held>,
    released: Signal,
}

/// \[EXTENSION\] The locks of a table's rows (RULES.md D185).
pub(crate) struct RowLocks {
    stripes: Vec<Stripe>,
    /// How many holders there are, so that a transaction's end looks at no
    /// stripe of a table whose rows none locks.
    count: AtomicUsize,
}

impl Default for RowLocks {
    fn default() -> Self {
        RowLocks { stripes: (0..ROW_STRIPES).map(|_| Stripe::default()).collect(), count: AtomicUsize::new(0) }
    }
}

/// A row's stripe, held as its writer writes it.
pub(crate) type Writing<'a> = MutexGuard<'a, Held>;

/// A row of a table, as a refusal names it.
pub(crate) struct RowOf<'a>(pub(crate) &'a QualifiedName);

impl Display for RowOf<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a row of {}", self.0)
    }
}

/// What a locker found of a row as it took its stripe.
enum Step {
    /// Locked.
    Got,
    /// Held by another transaction, and SKIP LOCKED written.
    Skipped,
    /// Changed by a transaction still open.
    Writer(TxId),
    /// Changed or deleted by one that committed after the locker's snapshot.
    Changed,
}

impl RowLocks {
    /// The stripe of row `id`.
    fn stripe(&self, id: RowId) -> Result<&Stripe> {
        let at = usize::try_from(id % ROW_STRIPES).unwrap_or_default();
        self.stripes.get(at).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] row {id} has no lock stripe")))
    }

    /// Releases `tx`'s locks, waking those who wait for them.
    pub(super) fn release(&self, tx: TxId) {
        if self.count.load(Ordering::SeqCst) == 0 {
            return;
        }
        for stripe in &self.stripes {
            let mut held = lock_builtin(&stripe.held);
            let mut gone = 0_usize;
            held.retain(|_, holders| {
                let before = holders.len();
                holders.retain(|h| h.tx != tx);
                gone = gone.saturating_add(before.saturating_sub(holders.len()));
                !holders.is_empty()
            });
            drop(held);
            if gone > 0 {
                self.count.fetch_sub(gone, Ordering::SeqCst);
                stripe.released.notify_all();
            }
        }
    }

    /// Row `id`'s stripe, held for `tx` to write the row once no other
    /// transaction holds a lock of it, the row named `what` in a refusal.
    ///
    /// # Errors
    ///
    /// As a table lock's wait (`waiting::wait_for`).
    pub(crate) fn writing<'a>(&'a self, tx: &Txn, what: &dyn Display, id: RowId) -> Result<Writing<'a>> {
        let stripe = self.stripe(id)?;
        let held = lock(&stripe.held)?;
        if held.get(&id).is_none_or(|holders| holders.iter().all(|h| h.tx == tx.id)) {
            return Ok(held);
        }
        drop(held);
        let who = tx.waiter();
        let holding = |state: &Held, me: ThreadId| others(state, id, who.id, true, me);
        let (outcome, guard) = wait_for((&who, Waits::Setting), what, (&stripe.held, &stripe.released), &holding)?;
        outcome?;
        Ok(guard)
    }

    /// Takes row `id`'s lock for `who`, exclusive or shared, once no lock
    /// against it is held, as `waits` says, unless `newest` says the row
    /// has a change pending or was changed since the locker's snapshot.
    fn take(
        &self,
        (who, waits): (&Waiter, Waits),
        (what, id): (&dyn Display, RowId),
        exclusive: bool,
        newest: &dyn Fn() -> Result<Step>,
    ) -> Result<Step> {
        let stripe = self.stripe(id)?;
        let holding = |held: &Held, me: ThreadId| others(held, id, who.id, exclusive, me);
        let (outcome, mut held) = wait_for((who, waits), what, (&stripe.held, &stripe.released), &holding)?;
        if !outcome? {
            return Ok(Step::Skipped);
        }
        let found = newest()?;
        let Step::Got = found else { return Ok(found) };
        let holders = held.entry(id).or_default();
        if let Some(mine) = holders.iter_mut().find(|h| h.tx == who.id) {
            mine.exclusive |= exclusive;
        } else {
            holders.push(RowHolder { tx: who.id, exclusive, running: Arc::clone(&who.running) });
            self.count.fetch_add(1, Ordering::SeqCst);
        }
        Ok(Step::Got)
    }
}

/// The transactions but `me` whose locks of row `id` in `held` stand
/// against a lock, exclusive or not, each with whether its call runs on
/// thread `here`.
fn others(held: &Held, id: RowId, me: TxId, exclusive: bool, here: ThreadId) -> Vec<(TxId, bool)> {
    let Some(holders) = held.get(&id) else { return Vec::new() };
    holders.iter().filter(|h| h.tx != me && (exclusive || h.exclusive)).map(|h| (h.tx, runs_here(&h.running, here))).collect()
}

/// What a locking query got of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Locked {
    /// Its lock: the row is given.
    Got,
    /// Nothing, of SKIP LOCKED: the row is passed over.
    Skipped,
}

/// \[EXTENSION\] How a locking query locks the rows of one table it reads
/// (RULES.md D185): owned, to be called as its plan runs.
pub(crate) struct RowLocking {
    pub(crate) who: Waiter,
    pub(crate) waits: Waits,
    /// FOR UPDATE, else FOR SHARE.
    pub(crate) exclusive: bool,
    pub(crate) table: QualifiedName,
    pub(crate) lock: Arc<TableLock>,
    pub(crate) storage: Arc<dyn TableStorage>,
    /// The snapshot the query reads at.
    pub(crate) snapshot: Timestamp,
    /// Set when a row it would lock had changed since `snapshot`, so a
    /// query at a snapshot of its own may run again at a new one.
    pub(crate) changed: AtomicBool,
}

impl RowLocking {
    /// Locks row `id`, waiting as `waits` says for a lock or a change
    /// another transaction holds.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if the lock is not granted, the row changed since the
    /// query's snapshot, or of NOWAIT (SQLSTATE TBL01); as a wait's.
    pub(crate) fn lock(&self, id: RowId) -> Result<Locked> {
        let what = format!("a row of {}", self.table);
        let newest = || {
            let n = self.storage.newest(self.who.id, id)?;
            let changed = n.row.is_none() || n.stamp > self.snapshot;
            Ok(match (n.writer, changed) {
                (Some(by), _) => Step::Writer(by),
                (None, true) => Step::Changed,
                (None, false) => Step::Got,
            })
        };
        loop {
            let writer = match self.lock.rows.take((&self.who, self.waits), (&what, id), self.exclusive, &newest)? {
                Step::Got => return Ok(Locked::Got),
                Step::Skipped => return Ok(Locked::Skipped),
                Step::Changed => return self.changed(&what),
                Step::Writer(by) => by,
            };
            match self.waits {
                Waits::Skip => return Ok(Locked::Skipped),
                Waits::NoWait => return Err(not_available(&what)),
                Waits::Setting => self.lock.wait_for_writer(&self.who, &what, writer)?,
            }
        }
    }

    /// Of a row `what` changed since the snapshot: passed over of SKIP
    /// LOCKED, else the statement refused, to run again.
    fn changed(&self, what: &str) -> Result<Locked> {
        if self.waits == Waits::Skip {
            return Ok(Locked::Skipped);
        }
        self.changed.store(true, Ordering::SeqCst);
        Err(SqlError::new(
            Code::Conflict,
            format!(
                "[ISO/IEC 9075:1992 §4.28] {what} was changed by a transaction that committed after the statement's snapshot, so {}",
                super::waiting::UNDONE
            ),
        ))
    }
}
