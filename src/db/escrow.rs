//! \[DESIGN\] Escrow of increments, the statement's side (RULES.md D452):
//! which searched UPDATE may reserve its increments instead of writing its
//! rows (`eligible`), the reservation of a row's (`State::reserve_row`),
//! and their application as the transaction commits (`State::fold`). The
//! reservations themselves are kept by each table (`txn/escrow.rs`).

use std::sync::Arc;

use crate::catalog::{ConstraintDesc, ConstraintKind, QualifiedName, TableDesc};
use crate::db::State;
use crate::db::integrity::Effects;
use crate::db::write::Written;
use crate::error::Result;
use crate::exec::{Subqueries, eval};
use crate::logical::{Cond, Expr, LogicalPlan, cond_columns, expr_columns};
use crate::settings::ON;
use crate::settings::escrow::TRANSACTION_ESCROW;
use crate::storage::{Row, RowId};
use crate::txn::escrow::{Reservation, bit, changed};
use crate::txn::{Again, Concurrency, LockWait, Timestamp, Txn, UndoEntry};
use crate::value::{ArithOp, DataType, Decimal, Value, arith, assign};

/// The rule a reservation's bound is told by when a sum leaves its
/// column's type: never told, a reservation that would not fit is not
/// taken, and its row is written as any other.
const BOUND: &str = "RULES.md D452";
/// The times a reservation is tried as its row changes under it, before
/// the row is written as any other.
const TRIES: usize = 8;
/// The last column a mask tells apart: it stands for every later one too.
const LAST: usize = 63;

/// A row as a reservation finds it.
enum Settled {
    /// No change of the guarded columns pending: the row, and the commit
    /// timestamp of its version.
    Row(Row, Timestamp),
    /// Absent or deleted.
    Gone,
    /// A change pending, or waited for: to be read again.
    Again,
}

impl Settled {
    /// Whether it is `row`, in the columns of `guarded`, with no change of
    /// them pending.
    fn is(&self, row: &Row, guarded: u64) -> bool {
        match self {
            Settled::Row(now, _) => changed(row, now) & guarded == 0,
            Settled::Gone | Settled::Again => false,
        }
    }
}

impl State {
    /// Whether `table` is partitioned or a partition.
    fn split(&self, table: &QualifiedName) -> bool {
        let partitioning = self.catalog.partitioning();
        partitioning.is_partitioned(table) || partitioning.of(table).is_some()
    }

    /// Whether column `c` of `table`, described by `desc`, is exact numeric
    /// and nothing but its values depends on it: no key, index, CHECK,
    /// domain or assertion; NOT NULL alone may constrain it.
    fn free(&self, table: &QualifiedName, desc: &TableDesc, c: usize) -> bool {
        let Some(column) = desc.columns.get(c) else { return false };
        let exact = matches!(column.data_type, DataType::Exact { .. }) && column.domain.is_none();
        let indexed = self.catalog.indexes(table).iter().any(|ix| ix.columns.contains(&c) || !ix.expressions.is_empty());
        let asserted = self.catalog.constrained().reading_checks || self.catalog.assertions().iter().any(|a| reads(a, table));
        exact && !indexed && !asserted && desc.constraints.iter().all(|k| unconstrained(&k.kind, c))
    }

    /// \[DESIGN\] Reserves, for `tx`, the increments `deltas` to row `id` of
    /// `table`, which its statement read as `seen`, each guarding the
    /// columns of `guarded`: whether all were (RULES.md D452). Not when the
    /// row is gone, when a guarded column but those added to changed since
    /// it was read, or when the row's value plus every increment held, the
    /// positive ones and the negative ones apart, would leave the column's
    /// type: its row is then written as any other. Another transaction's
    /// change of a guarded column pending, it waits for that one to end.
    ///
    /// # Errors
    ///
    /// The storage's, reading the row; the undo log's; a wait's.
    pub(super) fn reserve_row(
        &self,
        tx: &Txn,
        (table, id): (&QualifiedName, RowId),
        (seen, deltas, guarded): (&Row, &[(usize, Value)], u64),
    ) -> Result<bool> {
        let added = deltas.iter().fold(0, |mask, (c, _)| mask | bit(*c));
        let read = if added & bit(LAST) == 0 { guarded & !added } else { guarded };
        for _ in 0..TRIES {
            let (now, stamp) = match self.escrowed_row(tx, (table, id), (guarded, true))? {
                Settled::Row(now, stamp) => (now, stamp),
                Settled::Gone => return Ok(false),
                Settled::Again => continue,
            };
            if changed(seen, &now) & read != 0 {
                return Ok(false);
            }
            // Taken, and the row looked at again, while no write of it is
            // between its look and its change; taken back at once if short.
            let escrow = Arc::clone(&self.lock_of(table)?.escrow);
            let (whole, still) = escrow.exclusive(id, || -> Result<(bool, bool)> {
                let taken = self.take(tx, (table, id), &now, (deltas, guarded))?;
                let still = taken == deltas.len() && self.escrowed_row(tx, (table, id), (guarded, false))?.is(&now, guarded);
                if !still {
                    self.give_back(tx, (table, id), deltas.get(..taken).unwrap_or_default())?;
                }
                Ok((taken == deltas.len(), still))
            })?;
            if !whole {
                return Ok(false);
            }
            if still {
                for (column, _) in deltas {
                    tx.log(UndoEntry::Escrowed { table: table.clone(), escrow: Arc::clone(&escrow), id, column: *column })?;
                }
                // [DESIGN] RULES.md D441: a sum bounded by a commit past its
                // snapshot is told once that commit is durable.
                if stamp > tx.snapshot() {
                    tx.depend(stamp);
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Row `id` as it is now, if no other transaction's change of a column
    /// of `guarded`, or deletion, is pending; if one is, and `wait`, once
    /// that transaction ends.
    fn escrowed_row(&self, tx: &Txn, (table, id): (&QualifiedName, RowId), (guarded, wait): (u64, bool)) -> Result<Settled> {
        let newest = self.storage_of(table)?.newest(tx.id, id)?;
        let Some(row) = newest.row else { return Ok(Settled::Gone) };
        let touched = match &newest.pending {
            None => false,
            Some(None) => true,
            Some(Some(pending)) => changed(&row, pending) & guarded != 0,
        };
        if !touched {
            return Ok(Settled::Row(row, newest.stamp));
        }
        if let Some(writer) = newest.writer.filter(|_| wait) {
            drop(tx.wait_for_row((table, id), writer)?);
        }
        Ok(Settled::Again)
    }

    /// Reserves `deltas` on row `id`, now `now`, in order, as long as each
    /// fits: how many were.
    fn take(
        &self,
        tx: &Txn,
        (table, id): (&QualifiedName, RowId),
        now: &Row,
        (deltas, guarded): (&[(usize, Value)], u64),
    ) -> Result<usize> {
        let lock = self.lock_of(table)?;
        let Some(desc) = self.catalog.table(table) else { return Ok(0) };
        tx.escrowing(&lock.escrow);
        for (n, (c, delta)) in deltas.iter().enumerate() {
            let (Some(from @ Value::Exact(_)), Some(column)) = (now.get(*c), desc.columns.get(*c)) else { return Ok(n) };
            let reservation = Reservation { tx: tx.id, column: *c, delta: delta.clone(), guarded };
            if !lock.escrow.reserve((table, id), reservation, |held| Ok(fits(from, delta, (*c, &column.data_type), held)))? {
                return Ok(n);
            }
        }
        Ok(deltas.len())
    }

    /// Takes back the reservations of `taken` on row `id`.
    fn give_back(&self, tx: &Txn, (table, id): (&QualifiedName, RowId), taken: &[(usize, Value)]) -> Result<()> {
        let lock = self.lock_of(table)?;
        for (column, _) in taken {
            lock.escrow.take_back(tx.id, id, *column);
        }
        Ok(())
    }
}

impl State {
    /// \[DESIGN\] Applies the increments `tx` reserved, as it commits
    /// (RULES.md D452): in a statement of its own, at a snapshot of its own,
    /// each row's as one change of the row, written over its newest version
    /// as a READ COMMITTED statement's write is, table by table, row by row
    /// in order. Its reservations, held until it ends, keep any other
    /// transaction from changing what they add to meanwhile.
    ///
    /// # Errors
    ///
    /// A write's, as an UPDATE's: a deadlock, a timeout, a cancel.
    pub(crate) fn fold(&self, tx: &Txn) -> Result<()> {
        let tables = tx.reserved();
        if tables.is_empty() {
            return Ok(());
        }
        tx.begin_statement(|| true)?;
        for (table, own) in tables {
            self.fold_rows(tx, &table, &own)?;
        }
        Ok(())
    }

    /// \[DESIGN\] Applies now the increments `tx` reserved on `table`, as
    /// its statement is to read what they add to: so it reads its own sums
    /// (RULES.md D452). Each is taken back as applied, and put back if the
    /// statement is undone.
    ///
    /// # Errors
    ///
    /// The undo log's; a write's, as an UPDATE's.
    pub(crate) fn materialize(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        let escrow = Arc::clone(&self.lock_of(table)?.escrow);
        let own = escrow.take_own(tx.id);
        let mut logged = Ok(());
        for (id, r) in &own {
            let (column, delta, guarded) = (r.column, r.delta.clone(), r.guarded);
            let entry = UndoEntry::Folded { table: table.clone(), escrow: Arc::clone(&escrow), id: *id, column, delta, guarded };
            logged = logged.and_then(|()| tx.log(entry));
            if logged.is_err() {
                escrow.put_back(*id, r.clone());
            }
        }
        logged?;
        self.fold_rows(tx, table, &own)
    }

    /// Applies `own`, increments reserved on rows of `table`, in the order
    /// of their rows, each row's as one change of it.
    fn fold_rows(&self, tx: &Txn, table: &QualifiedName, own: &[(RowId, Reservation)]) -> Result<()> {
        let Some(desc) = self.catalog.table(table) else { return Ok(()) };
        let types: Vec<DataType> = desc.columns.iter().map(|c| c.data_type.clone()).collect();
        let mut at = 0;
        while let Some((id, _)) = own.get(at) {
            let id = *id;
            let row: Vec<&Reservation> = own.get(at..).unwrap_or_default().iter().take_while(|(of, _)| *of == id).map(|(_, r)| r).collect();
            at = at.saturating_add(row.len());
            self.fold_one(tx, (table, id), (&row, &types))?;
        }
        Ok(())
    }

    /// Row `id`'s increments `deltas` applied over its newest version,
    /// recomputed over a newer one if another transaction commits one
    /// first; none if the row is gone, deleted by `tx` itself.
    fn fold_one(&self, tx: &Txn, (table, id): (&QualifiedName, RowId), (deltas, types): (&[&Reservation], &[DataType])) -> Result<()> {
        let Some(row) = self.storage_of(table)?.newest(tx.id, id)?.row else { return Ok(()) };
        let mut recheck = |newer: &Row| Ok(Again::Update(added(newer, deltas, types)?));
        self.fold_row(tx, (table, id), &added(&row, deltas, types)?, &mut recheck).map(|_| ())
    }
}

/// `row` with `deltas` added, each sum assigned to its column's type.
fn added(row: &Row, deltas: &[&Reservation], types: &[DataType]) -> Result<Row> {
    let mut out = row.clone();
    for r in deltas {
        let (Some(value), Some(ty)) = (out.get(r.column), types.get(r.column)) else { continue };
        let sum = assign(arith(ArithOp::Add, value, &r.delta)?, ty, BOUND)?;
        if let Some(slot) = out.get_mut(r.column) {
            *slot = sum;
        }
    }
    Ok(out)
}

/// \[DESIGN\] Of the searched UPDATE `w`, whose statement does
/// `effects`, the columns its reservations guard, a bit a column, if it may reserve its increments rather than write its rows
/// (RULES.md D452): at READ COMMITTED, waiting for locks, with
/// TRANSACTION.ESCROW on, of a table whose engine keeps versions; each
/// SET item `c = c + e` or `c = c - e` of an exact numeric `c` and an
/// `e` that reads neither the row nor a table; its condition reading no
/// table and none of those columns; nothing that may see a sum before
/// it is applied: no trigger, delta table, view check, portion, typed
/// table or partition, generated column, CHECK, assertion or domain,
/// and none of the columns in a key or an index.
pub(super) fn escrow_of(st: &Arc<State>, tx: &Txn, w: &Written, effects: &Effects<'_>) -> Option<u64> {
    let b = &w.bound;
    let quiet = effects.triggered.is_none() && effects.delta.is_none() && effects.restricted.is_none();
    let plain = b.checks.is_empty() && b.portion.is_none() && b.generated.is_empty() && b.desc.typed.is_none();
    let on = if let Value::Char(word) = w.db(st).settings.get(TRANSACTION_ESCROW) { word == ON } else { false };
    let engine = st.capabilities(&b.table).is_ok_and(|caps| matches!(caps.concurrency, Concurrency::Engine { .. }));
    // Its application waits for a row as it commits, as a statement
    // waiting for locks would, never refused for want of waiting.
    let waits = !tx.one_snapshot() && tx.lock_wait == LockWait::Wait && engine && quiet;
    if !waits || !plain || !on || st.split(&b.table) {
        return None;
    }
    let mut columns = Vec::new();
    for (c, e) in &b.set {
        columns.push(increment(*c, e.as_ref()?)?);
    }
    let read = if let LogicalPlan::Filter { cond, .. } = &b.rows { condition_columns(cond)? } else { Vec::new() };
    if columns.iter().any(|c| read.contains(c) || !st.free(&b.table, &b.desc, *c)) {
        return None;
    }
    let guarded = columns.iter().chain(&read).fold(0, |mask, &c| mask | bit(c));
    Some(guarded)
}

/// The increments `w`'s SET items make to row `seen`, which they made
/// `new`, each of its column; none if one is not an exact number its
/// column holds as it is, or not what made `new`: its rows are then written
/// as any other's.
///
/// # Errors
///
/// An increment's evaluation's, as its SET item's.
pub(super) fn deltas(w: &Written, (seen, new): (&Row, &Row), subqueries: &dyn Subqueries) -> Result<Option<Vec<(usize, Value)>>> {
    let mut out = Vec::new();
    for (c, e) in &w.bound.set {
        let (Some(Expr::Arith(_, op, d)), Some(column), Some(from), Some(to)) =
            (e, w.bound.desc.columns.get(*c), seen.get(*c), new.get(*c))
        else {
            return Ok(None);
        };
        let added = eval(d, seen, subqueries)?;
        let delta = if matches!(op, ArithOp::Sub) { arith(ArithOp::Sub, &Value::Exact(Decimal::new(0, 0)), &added)? } else { added };
        let held = assign(delta.clone(), &column.data_type, BOUND).ok();
        let sum = arith(ArithOp::Add, from, &delta).ok().and_then(|sum| assign(sum, &column.data_type, BOUND).ok());
        if !(equal(held.as_ref(), &delta) && equal(sum.as_ref(), to)) {
            return Ok(None);
        }
        out.push((*c, delta));
    }
    Ok(Some(out))
}

/// Whether `a` is an exact number equal to `b`.
fn equal(a: Option<&Value>, b: &Value) -> bool {
    let difference = a.and_then(|a| arith(ArithOp::Sub, a, b).ok());
    if let Some(Value::Exact(d)) = difference { d.units == 0 } else { false }
}

/// The column a SET item `(c, e)` adds to, if `e` is `c + d` or `c - d`
/// of a `d` reading neither the row nor a table, invoking no routine and
/// drawing no sequence's value.
fn increment(c: usize, e: &Expr) -> Option<usize> {
    let Expr::Arith(a, ArithOp::Add | ArithOp::Sub, d) = e else { return None };
    let mut read = Vec::new();
    expr_columns(d, &mut read);
    let Expr::Column(of) = **a else { return None };
    (of == c && read.is_empty() && d.reads_nothing()).then_some(c)
}

/// The columns of the row `cond` reads, if it reads no table.
fn condition_columns(cond: &Cond) -> Option<Vec<usize>> {
    let mut read = Vec::new();
    cond_columns(cond, &mut read);
    cond.tables().is_empty().then_some(read)
}

/// Whether a constraint of kind `kind` leaves column `c` alone, but for
/// NOT NULL: a sum of exact numbers is not null.
fn unconstrained(kind: &ConstraintKind, c: usize) -> bool {
    match kind {
        ConstraintKind::NotNull(_) => true,
        ConstraintKind::Unique { columns, .. } => !columns.contains(&c),
        ConstraintKind::ForeignKey(fk) => !fk.columns.contains(&c),
        ConstraintKind::Check { .. } | ConstraintKind::Connection(_) => false,
    }
}

/// Whether assertion `a` reads `table`.
fn reads(a: &ConstraintDesc, table: &QualifiedName) -> bool {
    if let ConstraintKind::Check { reads, .. } = &a.kind { reads.contains(table) } else { false }
}

/// Whether `delta` added to column `column`, of type `ty`, whose value is
/// `value`, keeps it in its type whichever of the increments `held` apply
/// first: with every positive one, and with every negative one.
fn fits(value: &Value, delta: &Value, (column, ty): (usize, &DataType), held: &[Reservation]) -> bool {
    let positive = |v: &Value| if let Value::Exact(d) = v { d.units > 0 } else { false };
    let deltas = held.iter().filter(|r| r.column == column).map(|r| &r.delta).chain([delta]);
    let (mut up, mut down) = (Some(value.clone()), Some(value.clone()));
    for d in deltas {
        let side = if positive(d) { &mut up } else { &mut down };
        *side = side.take().and_then(|sum| arith(ArithOp::Add, &sum, d).ok());
    }
    let within = |sum: Option<Value>| sum.is_some_and(|sum| assign(sum, ty, BOUND).is_ok());
    within(up) && within(down)
}
