//! \[EXTENSION\] The constraints of rows inserted together checked as each
//! row goes in (RULES.md D530), not from a note of each row read back at
//! the statement's end [ISO/IEC 9075:1992 §10.6 GR3].
//!
//! A row inserted is not changed again by an INSERT that activates no
//! trigger and makes no referential action, so its NOT NULL constraints
//! and CHECKs without a subquery hold of it at the statement's end if they
//! hold of it as it goes in; and its key, of a UNIQUE constraint whose
//! unique index of exactly its columns found its key's only entry as it
//! went in (`Txn::alone`), is no duplicate. Such a row needs no note; any
//! other is noted, and checked at the end as every row was. A statement,
//! or a table, for which that does not hold has every row noted: one whose
//! changes a trigger or a delta table reads, one with a constraint
//! deferred, a foreign key, an edge's endpoints or a UNIQUE of a period,
//! one of a family of typed tables, and one an assertion or a CHECK with a
//! subquery may read.

use std::collections::HashMap;

use super::Effects;
use crate::catalog::{ConstraintDesc, ConstraintKind, QualifiedName, TableDesc};
use crate::db::State;
use crate::error::{Code, Result, Subject};
use crate::logical::Cond;
use crate::storage::{Row, RowId};
use crate::txn::Txn;

/// The checks of the rows of a table inserted together, made as each goes
/// in.
pub(in crate::db) struct RowsChecked<'s> {
    desc: &'s TableDesc,
    /// Its NOT NULL constraints and CHECKs, in the order of its constraints.
    rows: Vec<&'s ConstraintDesc>,
    /// The unique index of each of its UNIQUE constraints.
    unique: Vec<&'s str>,
    /// Each CHECK's condition, bound once.
    bound: HashMap<QualifiedName, Cond>,
}

impl State {
    /// The checks of `table`'s rows inserted by a statement that did
    /// `effects`, made as each goes in; none if each is to be noted.
    pub(in crate::db) fn rows_checked<'s>(&'s self, table: &QualifiedName, effects: &Effects<'_>) -> Option<RowsChecked<'s>> {
        let noted = effects.triggered.is_some() || effects.delta.is_some() || effects.restricted.is_some();
        let read = self.catalog.constrained().reading_checks || self.catalog.assertions().iter().any(|a| reads(a, table));
        if noted || read || self.family(table).iter().any(|t| t != table) {
            return None;
        }
        let desc = self.catalog.table(table)?;
        let mut checked = RowsChecked { desc, rows: Vec::new(), unique: Vec::new(), bound: HashMap::new() };
        let deferral = effects.deferral.borrow();
        for c in self.constraints_of(table) {
            if deferral.deferred(c) {
                return None;
            }
            match &c.kind {
                ConstraintKind::NotNull(_) => checked.rows.push(c),
                ConstraintKind::Check { reads, .. } => {
                    if !reads.is_empty() {
                        return None;
                    }
                    checked.rows.push(c);
                }
                ConstraintKind::Unique { columns, period: None, .. } => {
                    let key = desc.key_columns(columns, None);
                    let mut indexes = self.catalog.indexes(table).into_iter();
                    checked.unique.push(&indexes.find(|ix| ix.unique && ix.columns == key)?.name);
                }
                ConstraintKind::Unique { .. } | ConstraintKind::ForeignKey(_) | ConstraintKind::Connection(_) => return None,
            }
        }
        Some(checked)
    }
}

/// Whether assertion `a` reads `table`.
fn reads(a: &ConstraintDesc, table: &QualifiedName) -> bool {
    if let ConstraintKind::Check { reads, .. } = &a.kind { reads.contains(table) } else { false }
}

impl RowsChecked<'_> {
    /// That `row`, about to go in, satisfies the table's NOT NULL
    /// constraints and CHECKs, as `State::check` finds them.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, about the constraint and its table,
    /// for one it breaks [ISO/IEC 9075:1992 §18.1 GR3f].
    pub(in crate::db) fn check(&mut self, (st, tx): (&State, &Txn), row: &Row) -> Result<()> {
        for &c in &self.rows {
            st.check_row_values(tx, (self.desc, c), row, &mut self.bound).map_err(|e| {
                if e.code == Code::Constraint && e.about_nothing() {
                    e.about(Subject::constraint(c.name.parts(), self.desc.name.parts()))
                } else {
                    e
                }
            })?;
        }
        Ok(())
    }

    /// Whether row `id`, gone in, needs a note for the statement's end:
    /// its key was not its unique index's only entry.
    pub(in crate::db) fn needs_note(&self, tx: &Txn, id: RowId) -> bool {
        self.unique.iter().any(|ix| !tx.alone(ix, id))
    }
}
