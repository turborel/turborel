//! \[DESIGN\] The constraints of a compiled program's point change, a
//! searched UPDATE or DELETE of one row by its key, or of its INSERT of a
//! query's rows (`straight/rows.rs`), written straight on its table
//! (RULES.md D841, D901), and their checks at the end of its statement.
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

mod rows;

use super::gather::{assigned, of_row};
use super::{Check, Checking, broken};
use crate::binder::BoundWrite;
use crate::catalog::{ConstraintDesc, ConstraintKind, QualifiedName, TableDesc};
use crate::db::State;
use crate::db::assignment::set_columns_of;
use crate::db::write::system_time::system_time;
use crate::error::Result;
use crate::storage::{Row, RowId, column_bit};
use crate::txn::Txn;
use crate::value::Value;

/// The constraints a point change checks of the row it writes, each
/// immediate and not deferrable: its row constraints, then its UNIQUE
/// ones; and of an UPDATE, the tables its SET values read, found once.
#[derive(Debug)]
pub(in crate::db) struct Straight {
    row: Vec<ConstraintDesc>,
    /// Each UNIQUE constraint, with the unique indexes of exactly its key
    /// whose note of a row's entry inserted alone settles its check of
    /// that row (`Txn::alone`, RULES.md D880): none where its check looks
    /// further.
    unique: Vec<(QualifiedName, Vec<String>)>,
    set_tables: Vec<QualifiedName>,
    /// Whether a row constraint is checked of the row its statement holds
    /// (RULES.md D880): not of a table with a system-time period, whose
    /// rows are stamped as they are written.
    in_hand: bool,
    /// Whether a row constraint may read a table: any but NOT NULL.
    reads: bool,
    /// \[DESIGN\] (RULES.md D875) Of an UPDATE whose row before nothing
    /// of its statement reads, the columns it sets, a bit a column: its
    /// write may then be told them, and read no other of the row it
    /// replaces. None of a DELETE or an INSERT, of a table with a
    /// system-time period, whose history keeps the row before, or of a
    /// column past the last bit. A statement written straight has no
    /// trigger, no foreign key referencing a column it sets and no OLD
    /// TABLE of an UPDATE, and checks its constraints of the row after.
    set: Option<u64>,
}

impl Straight {
    /// The tables the SET values of its UPDATE read (`BoundWrite::set_tables`).
    pub(in crate::db) fn set_tables(&self) -> &[QualifiedName] {
        &self.set_tables
    }

    /// The columns its UPDATE sets, if its write may be told them (`set`).
    pub(in crate::db) fn set(&self) -> Option<u64> {
        self.set
    }
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
        let straight = self.straight_of(table, (columns.as_deref(), delete))?;
        let mask = |set: &Vec<usize>| set.iter().try_fold(0_u64, |mask, &c| column_bit(c).map(|bit| mask | bit));
        let set = columns.as_ref().filter(|_| straight.in_hand).and_then(mask);
        Some(Straight { set_tables: bound.set_tables(), set, ..straight })
    }

    /// Of an INSERT into `table`, the constraints its statement checks of
    /// each row it inserts, as `straight` finds them.
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
        let in_hand = system_time(desc).is_none();
        let mut straight = Straight { row: Vec::new(), unique: Vec::new(), set_tables: Vec::new(), in_hand, reads: false, set: None };
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
                straight.unique.push((c.name.clone(), self.settling(desc, &c.kind)));
            } else {
                straight.reads |= !matches!(c.kind, ConstraintKind::NotNull(_));
                straight.row.push(c.clone());
            }
        }
        Some(straight)
    }

    /// \[DESIGN\] RULES.md D880: the unique indexes of `desc` on exactly
    /// the key of its UNIQUE constraint of `kind`, where `KeyCheck::row`
    /// passes over a row whose entry in one was inserted alone: of a
    /// constraint with no period, of a table that is no other's subtable
    /// or partition, so the constraint's rows are its own alone. None
    /// otherwise, and the check is made whole.
    fn settling(&self, desc: &TableDesc, kind: &ConstraintKind) -> Vec<String> {
        let ConstraintKind::Unique { columns, period: None, .. } = kind else { return Vec::new() };
        if !self.above(&desc.name).is_empty() || self.family(&desc.name).len() != 1 {
            return Vec::new();
        }
        let key = desc.key_columns(columns, None);
        self.catalog.indexes(&desc.name).into_iter().filter(|ix| ix.unique && ix.columns == key).map(|ix| ix.name.clone()).collect()
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
    /// and between, each of its rows' constraints. `held` is the one row
    /// written, as its statement holds it, if it wrote one and holds it.
    ///
    /// \[DESIGN\] RULES.md D880: a row constraint is checked of the row
    /// held, where that is the row stored, with no read of it back; a
    /// UNIQUE constraint of a row whose entry was inserted alone is
    /// settled by that note; and where no check then reads a table, the
    /// participants are not told to check reads and told again not to.
    ///
    /// # Errors
    ///
    /// As `end_statement`.
    pub(in crate::db) fn end_straight(
        self: &Arc<Self>,
        tx: &Txn,
        (table, straight): (&QualifiedName, &Straight),
        ids: &[RowId],
        held: Option<&Row>,
    ) -> Result<()> {
        // A row of large objects is stored as its table keeps them.
        let held = held.filter(|row| straight.in_hand && ids.len() == 1 && !row.iter().any(Value::needs_keeping));
        let unsettled = |&id: &RowId| straight.unique.iter().any(|(_, alone)| !settled(tx, alone, id));
        let reads_back = held.is_none() && !straight.row.is_empty();
        if !ids.is_empty() && (straight.reads || reads_back || ids.iter().any(unsettled)) {
            // [ISO/IEC 9075:1992 §4.28] serializable at every level.
            tx.check_reads(true)?;
        }
        let checked = self.straight_checked(tx, (table, straight), ids, held);
        let read = tx.check_reads(false);
        checked.and(read)
    }

    /// `end_straight`'s checks of the rows `ids`, of `held` if given.
    fn straight_checked(
        self: &Arc<Self>,
        tx: &Txn,
        (table, straight): (&QualifiedName, &Straight),
        ids: &[RowId],
        held: Option<&Row>,
    ) -> Result<()> {
        let Some(desc) = self.catalog.table(table) else { return Ok(()) };
        let mut checking = Checking::default();
        for &id in ids {
            for c in &straight.row {
                match held {
                    Some(row) => self.check_row_values(tx, (desc, c), row, &mut checking.bound).map_err(|e| broken(e, (desc, c)))?,
                    None => self.row_now(tx, (desc, c), id, &mut checking)?,
                }
            }
        }
        for (constraint, alone) in &straight.unique {
            for &id in ids.iter().filter(|&&id| !settled(tx, alone, id)) {
                let check = Check::Unique { table: table.clone(), constraint: constraint.clone(), id };
                self.check(tx, &check, &mut checking)?;
            }
        }
        self.end_checking(tx, &mut checking)
    }
}

/// Whether row `id`'s entry in one of the unique indexes `alone` was its
/// key's only one as `tx` inserted it, which settles its constraint's
/// check of the row (`KeyCheck::row`).
fn settled(tx: &Txn, alone: &[String], id: RowId) -> bool {
    alone.iter().any(|index| tx.alone(index, id))
}
