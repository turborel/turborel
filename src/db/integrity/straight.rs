//! \[DESIGN\] The constraints of a compiled program's point change, a
//! searched UPDATE or DELETE of one row by its key or an INSERT of one row
//! of values, written straight on its table (RULES.md D841), and their
//! checks at the end of its statement.
//!
//! What its statement's end checks is known from its table and the columns
//! it gives values to [ISO/IEC 9075:1992 §10.6 GR3], as `State::checks`
//! gathers it: of a row inserted, every constraint of its table; of a row
//! updated, the constraints on a column given a value, UNIQUE ones too,
//! and each CHECK without a subquery; of a row deleted, the foreign keys
//! referencing it. Such a change runs straight only where these are all
//! its end checks, each of the row it wrote, in the order `State::checks`
//! makes them, the row's own first and each at once as one that cannot be
//! deferred is, then its UNIQUE ones. A constraint that may be deferred, a
//! foreign key referencing a column given a value or a row deleted, a
//! CHECK reading tables and an assertion each leave the change to the
//! general path.

use std::sync::Arc;

use super::gather::{assigned, of_row};
use super::{Check, Checking};
use crate::binder::BoundWrite;
use crate::catalog::{ConstraintDesc, ConstraintKind, QualifiedName};
use crate::db::State;
use crate::db::assignment::set_columns_of;
use crate::error::Result;
use crate::storage::RowId;
use crate::txn::Txn;

/// The constraints a point change checks of the row it writes, each
/// immediate and not deferrable: its row constraints, then its UNIQUE
/// ones.
#[derive(Debug)]
pub(in crate::db) struct Straight {
    row: Vec<ConstraintDesc>,
    unique: Vec<QualifiedName>,
}

impl State {
    /// Of the searched UPDATE, or DELETE if `delete`, bound as `bound`, the
    /// constraints its statement checks of the row it writes, if they are
    /// all it checks: none if it is to run the general path.
    pub(in crate::db) fn straight(&self, bound: &BoundWrite, delete: bool) -> Option<Straight> {
        let table = &bound.table;
        if !bound.checks.is_empty() || bound.portion.is_some() || bound.named != *table {
            return None;
        }
        let columns = (!delete).then(|| set_columns_of(&bound.set, &bound.generated));
        if self.referenced_by(table, columns.as_deref()) {
            return None;
        }
        self.straight_of(table, (columns.as_deref(), delete))
    }

    /// Of an INSERT of one row into `table`, the constraints its statement
    /// checks of it, as `straight` finds them.
    pub(in crate::db) fn straight_insert(&self, table: &QualifiedName) -> Option<Straight> {
        self.straight_of(table, (None, false))
    }

    /// `straight`'s constraints of `table`, of a row given values in
    /// `columns`, or in every column if none, or deleted if `delete`.
    fn straight_of(&self, table: &QualifiedName, (columns, delete): (Option<&[usize]>, bool)) -> Option<Straight> {
        let desc = self.catalog.table(table)?;
        let whole = desc.typed.is_none() && !self.catalog.partitioning().is_partitioned(table);
        if !whole || self.catalog.constrained().reading_checks || !self.catalog.assertions().is_empty() {
            return None;
        }
        let mut straight = Straight { row: Vec::new(), unique: Vec::new() };
        if delete {
            return Some(straight);
        }
        for c in self.constraints_of(table) {
            let unique = if let ConstraintKind::Unique { columns: key, period, .. } = &c.kind {
                Some(assigned(&desc.key_columns(key, period.as_deref()), columns))
            } else {
                None
            };
            if !unique.unwrap_or_else(|| of_row(Some(desc), c, columns)) {
                continue;
            }
            if c.attributes.deferrable {
                return None;
            }
            if unique.is_some() {
                straight.unique.push(c.name.clone());
            } else {
                straight.row.push(c.clone());
            }
        }
        Some(straight)
    }

    /// Whether a foreign key references `table`, a row of which is deleted,
    /// or updated giving values to `columns`, by one of them: a check or a
    /// referential action of its statement's end [ISO/IEC 9075:1992 §11.8
    /// GR2].
    fn referenced_by(&self, table: &QualifiedName, columns: Option<&[usize]>) -> bool {
        if !self.catalog.constrained().foreign_keys {
            return false;
        }
        self.catalog.tables().iter().any(|child| {
            self.foreign_keys_of(&child.name).into_iter().filter(|(_, fk)| self.reaches(table, &fk.table)).any(|(_, fk)| {
                let parent = self.catalog.table(&fk.table);
                let key = parent.map_or_else(|| fk.referenced.clone(), |p| fk.referenced_key(p));
                assigned(&key, columns)
            })
        })
    }

    /// [ISO/IEC 9075:1992 §10.6 GR3] The end of the statement of `tx` that
    /// wrote the rows `ids` of `table` by a point change whose constraints
    /// are `straight`'s: its reads checked as `end_statement` checks them,
    /// and between, each of its rows' constraints.
    ///
    /// # Errors
    ///
    /// As `end_statement`.
    pub(in crate::db) fn end_straight(
        self: &Arc<Self>,
        tx: &Txn,
        (table, straight): (&QualifiedName, &Straight),
        ids: &[RowId],
    ) -> Result<()> {
        // [ISO/IEC 9075:1992 §4.28] serializable at every level.
        tx.check_reads(true)?;
        let checked = self.straight_checked(tx, (table, straight), ids);
        let read = tx.check_reads(false);
        checked.and(read)
    }

    /// `end_straight`'s checks of the rows `ids`.
    fn straight_checked(self: &Arc<Self>, tx: &Txn, (table, straight): (&QualifiedName, &Straight), ids: &[RowId]) -> Result<()> {
        let Some(desc) = self.catalog.table(table) else { return Ok(()) };
        let mut checking = Checking::default();
        for &id in ids {
            for c in &straight.row {
                self.row_now(tx, (desc, c), id, &mut checking)?;
            }
        }
        for constraint in &straight.unique {
            for &id in ids {
                let check = Check::Unique { table: table.clone(), constraint: constraint.clone(), id };
                self.check(tx, &check, &mut checking)?;
            }
        }
        self.end_checking(tx, &mut checking)
    }
}
