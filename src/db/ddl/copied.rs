//! The rows of a table an ALTER TABLE gives a new generation of its
//! storage, copied there by the transaction's own writes, each changed as
//! the action changes the table [ISO/IEC 9075:1992 §11.11 GR2, §11.15
//! GR4; ISO/IEC 9075-2:2023 §11.19 GR1]; and the rows of a table checked
//! of a constraint added [ISO/IEC 9075:1992 §11.16 GR2]. Each is read a
//! row at a time from a scan of the storage, so neither holds the table's
//! rows.

use std::sync::Arc;

use super::super::alter::{Reshape, renamed_to};
use super::super::defaults::default_value;
use super::super::generated::Generating;
use super::super::integrity::Effects;
use super::super::{State, api};
use crate::ast::AlterAction;
use crate::catalog::{ColumnDesc, QualifiedName};
use crate::error::{Code, Result, fail};
use crate::exec::NoSubqueries;
use crate::txn::Txn;
use crate::value::{DataType, Value, assign};

/// [ISO/IEC 9075-2:2023 §11.19 GR1] The rule a value of a column given a
/// new type is assigned to it by, as its row is copied.
const RETYPED: &str = "ISO/IEC 9075-2:2023 §11.19 GR1";

/// [ISO/IEC 9075:1992 §11.11 GR2] The column `action` adds to `table`, as
/// `next` describes it, if it adds one.
fn added_column(next: &State, table: &QualifiedName, action: &AlterAction) -> Option<ColumnDesc> {
    let AlterAction::AddColumn(_) = action else { return None };
    next.catalog.table(table).and_then(|t| t.columns.last()).cloned()
}

/// [ISO/IEC 9075-2:2023 §11.19 GR1] The position of the column `reshape`
/// gives a new type, and the type, as `next` describes `table`.
fn retyped(next: &State, table: &QualifiedName, reshape: Reshape) -> Option<(usize, DataType)> {
    let Reshape::Retype(at) = reshape else { return None };
    next.catalog.table(table).and_then(|t| t.columns.get(at)).map(|c| (at, c.data_type.clone()))
}

/// Copies the rows of `table` that transaction `tx` sees in `st` into its
/// new generation in `next`, each reshaped as the action says, by `tx`'s
/// own writes: under its new name, of a table renamed.
///
/// # Errors
///
/// SQLCODE -201 if a column added NOT NULL would be null in a row
/// [ISO/IEC 9075:1992 §11.11 GR2]; a value's assignment to a column's new
/// type [§9.2]; the reads' and writes'.
pub(in crate::db) fn copy_rows(
    st: &Arc<State>,
    next: &Arc<State>,
    tx: &Txn,
    table: &QualifiedName,
    (action, reshape, mut effects): (&AlterAction, Reshape, Effects<'_>),
) -> Result<()> {
    st.read_lock(tx, table)?;
    // The copy is no change of SQL-data a trigger's event is [ISO/IEC
    // 9075-2:2023 §4.46.1], so it fires none.
    effects.triggered = None;
    let target = renamed_to(table, action);
    let width = st.catalog.table(table).map_or(0, |t| t.columns.len());
    let columns: Vec<usize> = (0..width).collect();
    // [§11.11 GR2] every value of a column added is its default.
    let added = added_column(next, &target, action);
    let fill = added.as_ref().map(|c| default_value(c, effects.user().of(next.catalog.roles()), effects.now)).transpose()?;
    effects.rewritten(&target);
    // [ISO/IEC 9075-2:2023 §11.11 GR2a, GR2b] or, of an identity column,
    // its generator's next value, of a generated column, its expression's.
    let generating = match next.catalog.table(&target).filter(|_| added.is_some()) {
        Some(desc) => Generating::of_column(next, desc, desc.columns.len().saturating_sub(1))?,
        None => Generating::default(),
    };
    // [ISO/IEC 9075-2:2023 §11.27 GR1c] of a system-time period added,
    // its start the transaction timestamp, its end the highest value
    // (IV181).
    let stamp = if matches!(reshape, Reshape::Two(None)) { next.stamp_of(tx, &target)? } else { None };
    let retyped = retyped(next, &target, reshape);
    for read in Arc::clone(st.storage_of(table)?).scan(tx.id, &columns) {
        let (_, row) = read?;
        let mut new = reshape.row(row);
        if let Some(stamp) = &stamp {
            stamp.put(&mut new);
        }
        if let Some((at, t)) = &retyped
            && let Some(slot) = new.get_mut(*at)
        {
            *slot = assign(std::mem::replace(slot, Value::Null), t, RETYPED)?;
        }
        if let (Some(c), Some(value)) = (&added, &fill) {
            if let Some(slot) = new.last_mut() {
                *slot = value.clone();
            }
            generating.give(next, &mut new, &NoSubqueries)?;
            if c.not_null && new.last().is_none_or(Value::is_null) {
                return fail(
                    Code::Constraint,
                    format!("[ISO/IEC 9075:1992 §11.11 GR2] {table} has rows, in which NOT NULL column {} would be null", c.name),
                );
            }
        }
        effects.inserted(&target, next.insert_row(tx, &target, &new)?, &new)?;
    }
    // The table's constraints, those of a column added among them, hold of
    // the rows copied.
    next.end_statement(tx, effects)
}

/// [ISO/IEC 9075:1992 §11.16 GR2] The rows of `table` transaction `tx`
/// sees, in `next`, its definitions with a constraint added, satisfy its
/// constraints: NOT NULL too, a PRIMARY KEY's columns not null [§11.7
/// SR3a], and one SET NOT NULL adds [ISO/IEC 9075-2:2023 §11.15 GR1d].
pub(in crate::db) fn validate_rows(next: &Arc<State>, tx: &Txn, table: &QualifiedName, mut effects: Effects<'_>) -> Result<()> {
    // [EXTENSION] a partitioned table's rows are its partitions' (RULES.md D153).
    if next.catalog.partitioning().is_partitioned(table) {
        return super::super::partitions::validate_attached(next, tx, table, effects);
    }
    next.read_lock(tx, table)?;
    let desc = next.catalog.table(table).cloned().ok_or_else(|| api(format!("[DESIGN] no table {table}")))?;
    let columns: Vec<usize> = (0..desc.columns.len()).collect();
    effects.rewritten(table);
    for read in Arc::clone(next.storage_of(table)?).scan(tx.id, &columns) {
        let (id, row) = read?;
        effects.inserted(table, id, &row)?;
    }
    next.end_statement(tx, effects)
}

/// [ISO/IEC 9075-2:2023 §14.10 GR5] The end of TRUNCATE TABLE of `table`,
/// its new generation in `next` of no row, in transaction `tx`: each CHECK
/// with a subquery, and each assertion, that reads it checked, whole.
///
/// # Errors
///
/// SQLCODE -201 for a constraint it breaks.
pub(super) fn emptied_rows(next: &Arc<State>, tx: &Txn, table: &QualifiedName, mut effects: Effects<'_>) -> Result<()> {
    effects.rewritten(table);
    next.end_statement(tx, effects)
}
