//! \[EXTENSION\] The constraints of rows inserted together checked as each
//! row goes in (RULES.md D530), not from a note of each row read back at
//! the statement's end [ISO/IEC 9075:1992 §10.6 GR3].
//!
//! A row inserted is not changed again by an INSERT that activates no
//! trigger and makes no referential action, so its NOT NULL constraints
//! and CHECKs without a subquery hold of it at the statement's end if they
//! hold of it as it goes in; and its key, of a UNIQUE constraint whose
//! unique index of exactly its columns found its key's only entry as it
//! went in (`Index::insert_alone`), is no duplicate. Such a row needs no note; any
//! other is noted, and checked at the end as every row was. A statement,
//! or a table, for which that does not hold has every row noted: one whose
//! changes a trigger or a delta table reads, one with a constraint
//! deferred, a foreign key, an edge's endpoints or a UNIQUE of a period,
//! one of a family of typed tables, and one an assertion or a CHECK with a
//! subquery may read. Rows given by column are checked before they go in,
//! a foreign key's distinct keys each looked up once (RULES.md D1235).

use std::collections::{HashMap, HashSet};

use super::Effects;
use crate::catalog::{ConstraintDesc, ConstraintKind, ForeignKey, QualifiedName, TableDesc};
use crate::db::State;
use crate::error::{Code, Result, Subject};
use crate::exec::{Heard, eval_cond};
use crate::logical::{Cond, Truth, cond_columns};
use crate::storage::Row;
use crate::storage::lanes::Records;
use crate::txn::Txn;
use crate::value::Value;

/// The checks of the rows of a table inserted together, made as each goes
/// in.
pub(in crate::db) struct RowsChecked<'s> {
    desc: &'s TableDesc,
    /// Its NOT NULL constraints and CHECKs, in the order of its constraints.
    rows: Vec<&'s ConstraintDesc>,
    /// Whether it has UNIQUE constraints, each with a unique index of
    /// exactly its columns.
    unique: bool,
    /// Its foreign keys, of rows by column alone (RULES.md D1235).
    foreign: Vec<&'s ConstraintDesc>,
    /// Each CHECK's condition, bound once.
    bound: HashMap<QualifiedName, Cond>,
}

impl State {
    /// The checks of `table`'s rows inserted by a statement that did
    /// `effects`, made as each goes in; none if each is to be noted.
    pub(in crate::db) fn rows_checked<'s>(&'s self, table: &QualifiedName, effects: &Effects<'_>) -> Option<RowsChecked<'s>> {
        self.rows_checked_as(table, effects, false)
    }

    /// \[DESIGN\] RULES.md D1235: as `rows_checked`, of rows given by
    /// column, checked before they go in (`RowsChecked::hold_by_column`):
    /// a foreign key too, to another table, its referenced columns those
    /// of an index, with no period.
    pub(in crate::db) fn rows_checked_by_column<'s>(&'s self, table: &QualifiedName, effects: &Effects<'_>) -> Option<RowsChecked<'s>> {
        self.rows_checked_as(table, effects, true)
    }

    /// `rows_checked`, of rows by column if `by_column`.
    fn rows_checked_as<'s>(&'s self, table: &QualifiedName, effects: &Effects<'_>, by_column: bool) -> Option<RowsChecked<'s>> {
        let noted = effects.triggered.is_some() || effects.delta.is_some() || effects.restricted.is_some();
        let read = self.catalog.constrained().reading_checks || self.catalog.assertions().iter().any(|a| reads(a, table));
        if noted || read || self.family(table).iter().any(|t| t != table) {
            return None;
        }
        let desc = self.catalog.table(table)?;
        let mut checked = RowsChecked { desc, rows: Vec::new(), unique: false, foreign: Vec::new(), bound: HashMap::new() };
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
                    if !self.catalog.indexes(table).into_iter().any(|ix| ix.unique && ix.columns == key) {
                        return None;
                    }
                    checked.unique = true;
                }
                ConstraintKind::ForeignKey(fk) if by_column && self.looked_up(table, fk) => checked.foreign.push(c),
                ConstraintKind::Unique { .. } | ConstraintKind::ForeignKey(_) | ConstraintKind::Connection(_) => return None,
            }
        }
        Some(checked)
    }

    /// Whether foreign key `fk` of `table` references another table, of
    /// no subtable, through an index of exactly its referenced columns,
    /// with no period: so its keys are each looked up in it.
    fn looked_up(&self, table: &QualifiedName, fk: &ForeignKey) -> bool {
        let other = fk.table != *table && self.family(&fk.table).iter().all(|t| t == &fk.table);
        let exact = |ix: &&crate::catalog::IndexDesc| {
            ix.columns.len() == fk.referenced.len() && fk.referenced.iter().all(|c| ix.columns.contains(c))
        };
        other && fk.period.is_none() && self.catalog.indexes(&fk.table).iter().any(exact)
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

    /// \[DESIGN\] RULES.md D1148: whether every row of `columns`, the
    /// table's columns each by its records, satisfies its NOT NULL
    /// constraints and CHECKs, each by the columns it reads alone: a NOT
    /// NULL by its column's nulls, a CHECK of each row by the values of the
    /// columns it reads, the others null; and its foreign keys, each
    /// distinct key of the rows looked up once (`foreign_hold`). False if
    /// one is broken, so the rows go in as rows, which tells how. A
    /// UNIQUE constraint is checked as the rows' entries go in, as of rows
    /// inserted together (RULES.md D1235).
    ///
    /// # Errors
    ///
    /// As a CHECK's condition, bound or evaluated.
    pub(in crate::db) fn hold_by_column(&mut self, (st, tx): (&State, &Txn), columns: &[Records]) -> Result<bool> {
        for &c in &self.rows {
            let holds = match &c.kind {
                ConstraintKind::NotNull(at) => columns.get(*at).is_some_and(|r| r.nulls.iter().all(|b| *b == 0)),
                ConstraintKind::Check { text, names, horizon, .. } => {
                    let cond = st.bound(&self.desc.name, (c, (text, names, horizon)), &mut self.bound)?;
                    checked_by_column(self.desc, (&cond, &Heard(tx.stop.warnings())), columns)?
                }
                ConstraintKind::Unique { .. } | ConstraintKind::ForeignKey(_) | ConstraintKind::Connection(_) => false,
            };
            if !holds {
                return Ok(false);
            }
        }
        self.foreign_hold((st, tx), columns)
    }

    /// Whether every row of `columns` matches a row its foreign keys
    /// reference, as `State::check` finds it, each distinct key of the
    /// rows looked up once, in a row of its columns alone, the others
    /// null; false if one matches none.
    ///
    /// # Errors
    ///
    /// As the lookup, but a constraint broken.
    fn foreign_hold(&mut self, (st, tx): (&State, &Txn), columns: &[Records]) -> Result<bool> {
        for &c in &self.foreign {
            let ConstraintKind::ForeignKey(fk) = &c.kind else { continue };
            let mut seen: HashSet<Vec<Option<&[u8]>>> = HashSet::new();
            let mut row: Row = vec![Value::Null; self.desc.columns.len()];
            for n in 0..columns.first().map_or(0, Records::len) {
                let key: Vec<Option<&[u8]>> = fk.columns.iter().map(|&at| columns.get(at).and_then(|r| r.record(n))).collect();
                if seen.contains(&key) {
                    continue;
                }
                if !decoded_into(self.desc, (&fk.columns, &key), &mut row) {
                    return Ok(false);
                }
                match st.check_row_values(tx, (self.desc, c), &row, &mut self.bound) {
                    Ok(()) => seen.insert(key),
                    Err(e) if e.code == Code::Constraint => return Ok(false),
                    Err(e) => return Err(e),
                };
            }
        }
        Ok(true)
    }

    /// Whether a row gone in needs a note for the statement's end: the
    /// table has UNIQUE constraints, and its key was not `alone`, the only
    /// entry of its unique indexes'.
    pub(in crate::db) fn needs_note(&self, alone: bool) -> bool {
        self.unique && !alone
    }
}

/// Whether the `fields` of `columns` of `desc`, each a record or none
/// for a null, decoded, are written into `row` at their columns: not if
/// one does not decode.
fn decoded_into(desc: &TableDesc, (columns, fields): (&[usize], &[Option<&[u8]>]), row: &mut Row) -> bool {
    for (&at, field) in columns.iter().zip(fields) {
        let (Some(column), Some(slot)) = (desc.columns.get(at), row.get_mut(at)) else { return false };
        *slot = match field.map(|f| column.data_type.decode_record(f)) {
            None => Value::Null,
            Some(Some(v)) => v,
            Some(None) => return false,
        };
    }
    true
}

/// Whether `cond`, a CHECK of `desc`, is not false of any row of
/// `columns`, each row made of the columns it reads alone, decoded, the
/// others null.
fn checked_by_column(desc: &TableDesc, (cond, heard): (&Cond, &Heard<'_>), columns: &[Records]) -> Result<bool> {
    let mut read = Vec::new();
    cond_columns(cond, &mut read);
    read.sort_unstable();
    read.dedup();
    let mut row: Row = vec![Value::Null; desc.columns.len()];
    for n in 0..columns.first().map_or(0, Records::len) {
        for &at in &read {
            let (Some(records), Some(column), Some(slot)) = (columns.get(at), desc.columns.get(at), row.get_mut(at)) else {
                return Ok(false);
            };
            *slot = match records.record(n) {
                None => Value::Null,
                Some(field) => match column.data_type.decode_record(field) {
                    Some(v) => v,
                    None => return Ok(false),
                },
            };
        }
        if eval_cond(cond, &row, heard)? == Truth::False {
            return Ok(false);
        }
    }
    Ok(true)
}
