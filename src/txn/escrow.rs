//! \[DESIGN\] Escrow of increments (RULES.md D452): a searched UPDATE that
//! only adds to exact numeric columns of a row, at READ COMMITTED, holds
//! no lock on the row. It reserves each increment here, and its
//! transaction applies them as it commits, as an ordinary change of the
//! row then (`db/escrow.rs`). A reservation is taken only while the
//! column's value plus every increment reserved on it, the positive ones
//! together and the negative ones together, stays within the column's
//! type, so a commit's application never overflows. Until its transaction
//! ends, a reservation also keeps another transaction from changing the
//! columns it guards (the column and those its statement's condition
//! read) or deleting the row: such a write waits for it, as for a lock
//! (`txn/write.rs`).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::hash::xxhash::Xxh3Map;
use crate::storage::{Row, RowId, column_bit, same};
use crate::sync::lock_builtin;
use crate::txn::{TxId, Txn};
use crate::value::Value;

/// An increment reserved on a column of a row.
#[derive(Clone, Debug)]
pub(crate) struct Reservation {
    /// The transaction that reserved it.
    pub(crate) tx: TxId,
    /// The column it adds to, by its place in the row.
    pub(crate) column: usize,
    /// What it adds: an exact numeric value, not null.
    pub(crate) delta: Value,
    /// The columns another transaction may not change while it is held, a
    /// bit a column, the 64th standing for every later one.
    pub(crate) guarded: u64,
}

/// \[DESIGN\] A table's reservations, by row.
#[derive(Default)]
pub(crate) struct Escrow {
    /// How many reservations are held: an ordinary write looks no further
    /// while none is.
    held: AtomicUsize,
    /// Moved before each reservation is taken. A write reads it before it
    /// looks for reservations and again once written, and looks again if
    /// it moved: a reservation taken meanwhile either finds the write
    /// pending and is taken back, or is found by the write.
    epoch: AtomicU64,
    rows: Mutex<Xxh3Map<RowId, Vec<Reservation>>>,
    /// The table's name, as its first reservation named it.
    table: Mutex<Option<QualifiedName>>,
}

impl Escrow {
    /// Whether any reservation is held.
    pub(crate) fn any(&self) -> bool {
        self.held.load(Ordering::Acquire) > 0
    }

    /// Its epoch now.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// Another transaction than `tx` holding a reservation of row `id` that
    /// a write by `tx` changing the columns of `changed` (none: deleting
    /// the row) must wait for.
    pub(crate) fn blocking(&self, tx: TxId, id: RowId, changed: Option<u64>) -> Option<TxId> {
        if !self.any() {
            return None;
        }
        let rows = lock_builtin(&self.rows);
        let held = rows.get(&id)?;
        held.iter().find(|r| r.tx != tx && changed.is_none_or(|mask| mask & r.guarded != 0)).map(|r| r.tx)
    }

    /// Reserves `reservation` on row `id` if `fits` says it may be, given
    /// the reservations of the row held now: whether it was.
    ///
    /// # Errors
    ///
    /// `fits`'s.
    pub(crate) fn reserve(
        &self,
        (table, id): (&QualifiedName, RowId),
        reservation: Reservation,
        fits: impl FnOnce(&[Reservation]) -> Result<bool>,
    ) -> Result<bool> {
        lock_builtin(&self.table).get_or_insert_with(|| table.clone());
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let mut rows = lock_builtin(&self.rows);
        let held = rows.entry(id).or_default();
        if !fits(held)? {
            if held.is_empty() {
                rows.remove(&id);
            }
            return Ok(false);
        }
        held.push(reservation);
        self.held.fetch_add(1, Ordering::AcqRel);
        Ok(true)
    }

    /// Takes back the latest reservation of `tx` on `column` of row `id`:
    /// its statement, or the transaction, undone.
    pub(crate) fn take_back(&self, tx: TxId, id: RowId, column: usize) {
        let mut rows = lock_builtin(&self.rows);
        let Some(held) = rows.get_mut(&id) else { return };
        if let Some(at) = held.iter().rposition(|r| r.tx == tx && r.column == column) {
            held.remove(at);
            self.held.fetch_sub(1, Ordering::AcqRel);
        }
        if held.is_empty() {
            rows.remove(&id);
        }
    }

    /// Whether `tx` holds a reservation on a column of `needed`, a bit a
    /// column.
    pub(crate) fn unfolded(&self, tx: TxId, needed: u64) -> bool {
        let rows = lock_builtin(&self.rows);
        rows.values().flatten().any(|r| r.tx == tx && bit(r.column) & needed != 0)
    }

    /// Takes every reservation of `tx` away, in the order of `of`: to be
    /// applied before it commits.
    pub(crate) fn take_own(&self, tx: TxId) -> Vec<(RowId, Reservation)> {
        let own = self.of(tx);
        self.end(tx);
        own
    }

    /// Puts `reservation` of row `id` back, its application undone.
    pub(crate) fn put_back(&self, id: RowId, reservation: Reservation) {
        lock_builtin(&self.rows).entry(id).or_default().push(reservation);
        self.held.fetch_add(1, Ordering::AcqRel);
    }

    /// Ends every reservation of `tx`, its transaction ended.
    pub(crate) fn end(&self, tx: TxId) {
        let mut rows = lock_builtin(&self.rows);
        let mut ended = 0_usize;
        rows.retain(|_, held| {
            let before = held.len();
            held.retain(|r| r.tx != tx);
            ended = ended.saturating_add(before.saturating_sub(held.len()));
            !held.is_empty()
        });
        self.held.fetch_sub(ended, Ordering::AcqRel);
    }

    /// The reservations of `tx`, in the order of their rows, then of their
    /// columns, then as made.
    pub(crate) fn of(&self, tx: TxId) -> Vec<(RowId, Reservation)> {
        let rows = lock_builtin(&self.rows);
        let mut own: Vec<(RowId, Reservation)> =
            rows.iter().flat_map(|(&id, held)| held.iter().filter(|r| r.tx == tx).map(move |r| (id, r.clone()))).collect();
        own.sort_by_key(|(id, r)| (*id, r.column));
        own
    }
}

impl Txn {
    /// \[DESIGN\] Notes that it reserves increments in `escrow`'s table, so
    /// it applies them as it commits and they end with it (RULES.md D452).
    pub(crate) fn escrowing(&self, escrow: &Arc<Escrow>) {
        let mut state = lock_builtin(&self.state);
        if !state.escrowed.iter().any(|e| Arc::ptr_eq(e, escrow)) {
            state.escrowed.push(Arc::clone(escrow));
        }
    }

    /// \[DESIGN\] The increments it reserved, by table in the order of their
    /// names, each table's in the order of its rows (RULES.md D452).
    pub(crate) fn reserved(&self) -> Vec<(QualifiedName, Vec<(RowId, Reservation)>)> {
        let escrowed = lock_builtin(&self.state).escrowed.clone();
        let mut tables: Vec<(QualifiedName, Vec<(RowId, Reservation)>)> = escrowed
            .iter()
            .filter_map(|escrow| Some((lock_builtin(&escrow.table).clone()?, escrow.of(self.id))))
            .filter(|(_, own)| !own.is_empty())
            .collect();
        tables.sort_by(|a, b| a.0.cmp(&b.0));
        tables
    }
}

/// The columns whose values `new` changes from `old`, a bit a column, the
/// 64th standing for every later one; a row of another width changes all.
pub(crate) fn changed(old: &Row, new: &Row) -> u64 {
    if old.len() != new.len() {
        return u64::MAX;
    }
    old.iter().zip(new).enumerate().filter(|(_, (a, b))| !same(a, b)).fold(0, |mask, (c, _)| mask | bit(c))
}

/// The bit of column `c` in a mask of `changed`, or of `Reservation::guarded`.
pub(crate) fn bit(c: usize) -> u64 {
    column_bit(c).unwrap_or(1 << 63)
}
