//! What an INSERT inserts into, and the rules it cites: its table, its
//! `<insert column list>` and the values it gives.

use super::super::rule;
use super::Inserted;
use crate::ast::{ColumnName, Overriding, TableName};
use crate::binder::no_column;
use crate::catalog::{ColumnDesc, Horizon, QualifiedName};
use crate::error::Result;

/// What an INSERT, or a MERGE's WHEN NOT MATCHED clause, inserts into: its
/// table, its `<insert column list>`, if written, the values it gives and
/// its `<override clause>`, and the rules it cites.
pub(in crate::db) struct InsertInto<'a> {
    pub(in crate::db) table: &'a TableName,
    pub(in crate::db) columns: Option<&'a [ColumnName]>,
    pub(in crate::db) given: (Inserted<'a>, Option<Overriding>),
    pub(in crate::db) rules: &'a InsertRules,
    /// The columns seen of the table without a column list: a triggered
    /// action's, as they were when its trigger was defined [ISO/IEC
    /// 9075-2:2023 §11.21 GR2, Note 561].
    pub(in crate::db) horizon: &'a Horizon,
}

/// The rules an insert cites, each in full: of a read-only table, of a
/// column of its column list its table lacks, of one listed twice, and of
/// the privileges it needs.
pub(in crate::db) struct InsertRules {
    pub(in crate::db) read_only: &'static str,
    pub(in crate::db) missing: &'static str,
    pub(in crate::db) twice: &'static str,
    pub(in crate::db) privilege: &'static str,
}

/// [X3.135-1986 §8.7 SR1 to SR3] The rules of an INSERT.
pub(in crate::db) const INSERT_RULES: InsertRules = InsertRules {
    read_only: "X3.135-1986 §8.7 SR2",
    missing: "X3.135-1986 §8.7 SR3",
    twice: "X3.135-1986 §8.7 SR3",
    privilege: "X3.135-1986 §8.7 SR1",
};

/// The columns `columns` of the table or view `name` that `list` names,
/// by their positions, in order: each a column of it, none named twice.
pub(in crate::db) fn object_columns(
    name: &QualifiedName,
    columns: &[ColumnDesc],
    list: &[ColumnName],
    rules: &InsertRules,
) -> Result<Vec<usize>> {
    let mut cols = Vec::new();
    for ColumnName { name: c, at } in list {
        let missing = || no_column(name, format!("[{}] {name} has no column {c}", rules.missing), c).or_at(*at);
        let i = columns.iter().position(|d| d.name == *c).ok_or_else(missing)?;
        if cols.contains(&i) {
            return Err(rule(format!("[{}] column {c} is listed twice", rules.twice)).or_at(*at));
        }
        cols.push(i);
    }
    Ok(cols)
}
