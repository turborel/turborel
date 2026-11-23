//! \[DESIGN\] RULES.md D901: the end of the statement of a compiled
//! program's INSERT of a query's rows written straight on its table.
//!
//! Its rows are checked as `end_straight` checks a point change's one
//! row (D880), each as its statement holds it. Every row's own
//! constraints come first, row by row, then each UNIQUE constraint's rows
//! one after another, the order `State::checks` gathers them in, so the
//! constraint a statement breaking several is refused for is the general
//! path's. A row whose key was its key's only entry in every unique index
//! as it went in is passed over by each UNIQUE constraint such an entry
//! settles. A row is read back where the row held is not the row stored,
//! of a table with a system-time period or of a value its table keeps
//! apart.

use std::sync::Arc;

use super::super::{Check, Checking, broken};
use super::{Straight, settled};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::exec::HeldRows;
use crate::storage::{Row, RowId};
use crate::txn::Txn;
use crate::value::Value;

impl State {
    /// [ISO/IEC 9075:1992 §10.6 GR3] The end of the statement of `tx`
    /// that inserted `rows` into `table` straight, its constraints
    /// `straight`'s: its reads checked as `end_statement` checks them,
    /// and between, each row's constraints. `gone` is each row's id, in
    /// the rows' order, and whether its key was its key's only entry in
    /// every unique index as it went in.
    ///
    /// # Errors
    ///
    /// As `end_statement`.
    pub(in crate::db) fn end_straight_rows(
        self: &Arc<Self>,
        tx: &Txn,
        (table, straight): (&QualifiedName, &Straight),
        gone: &[(RowId, bool)],
        rows: &mut HeldRows<Row>,
    ) -> Result<()> {
        let in_hand = straight.in_hand && !kept_apart(straight, rows)?;
        let unsettled = |&(id, alone): &(RowId, bool)| straight.unique.iter().any(|(_, by)| !settles(tx, by, (id, alone)));
        let reads_back = !in_hand && !straight.row.is_empty();
        if !gone.is_empty() && (straight.reads || reads_back || gone.iter().any(unsettled)) {
            // [ISO/IEC 9075:1992 §4.28] serializable at every level.
            tx.check_reads(true)?;
        }
        let checked = self.straight_rows_checked(tx, (table, straight), (gone, in_hand), rows);
        let read = tx.check_reads(false);
        checked.and(read)
    }

    /// `end_straight_rows`' checks: of each row held, if `in_hand`, else
    /// read back by its id.
    fn straight_rows_checked(
        self: &Arc<Self>,
        tx: &Txn,
        (table, straight): (&QualifiedName, &Straight),
        (gone, in_hand): (&[(RowId, bool)], bool),
        rows: &mut HeldRows<Row>,
    ) -> Result<()> {
        let Some(desc) = self.catalog.table(table) else { return Ok(()) };
        let mut checking = Checking::default();
        if in_hand && !straight.row.is_empty() {
            rows.visit(&mut |row| {
                for c in &straight.row {
                    self.check_row_values(tx, (desc, c), row, &mut checking.bound).map_err(|e| broken(e, (desc, c)))?;
                }
                Ok(())
            })?;
        } else {
            for &(id, _) in gone {
                straight.row.iter().try_for_each(|c| self.row_now(tx, (desc, c), id, &mut checking))?;
            }
        }
        for (constraint, by) in &straight.unique {
            for &(id, _) in gone.iter().filter(|&&row| !settles(tx, by, row)) {
                let check = Check::Unique { table: table.clone(), constraint: constraint.clone(), id };
                self.check(tx, &check, &mut checking)?;
            }
        }
        self.end_checking(tx, &mut checking)
    }
}

/// Whether the entry of row `id` settles the check of a UNIQUE constraint
/// whose unique indexes of exactly its key are `by`: it was `alone` in
/// every unique index as a batch put it in, or its transaction noted it
/// alone in one of `by` (`settled`); never of a constraint no entry
/// settles.
fn settles(tx: &Txn, by: &[String], (id, alone): (RowId, bool)) -> bool {
    (alone && !by.is_empty()) || settled(tx, by, id)
}

/// Whether a row constraint is to be checked and one of `rows` holds a
/// value its table keeps apart, so the rows held are not the rows stored.
fn kept_apart(straight: &Straight, rows: &mut HeldRows<Row>) -> Result<bool> {
    if straight.row.is_empty() {
        return Ok(false);
    }
    let mut apart = false;
    rows.visit(&mut |row| {
        apart = apart || row.iter().any(Value::needs_keeping);
        Ok(())
    })?;
    Ok(apart)
}
