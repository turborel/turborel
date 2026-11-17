//! [ISO/IEC 9075-2:2023 §14.12 SR9, SR14 to SR18, GR6] The WHEN clauses of
//! a MERGE, bound: each search condition and SET clause over the target's
//! columns and the source's, each merge insert value list over the
//! source's alone.

use super::super::datetime::stored;
use super::super::statements::{AssignRule, SetRules, bind_set_list};
use super::super::{Binder, ObjectColumn, Scope, rule};
use super::target::CLAUSE;
use super::{BoundAction, BoundWhen};
use crate::ast::{MatchedAction, MergeInsert, MergeWhen, SearchCondition, SetClause};
use crate::catalog::TableDesc;
use crate::error::{At, Code, Result, SqlError};
use crate::logical::{Cond, Expr};

/// The rule a set function in a MERGE's search condition breaks: it has
/// no argument source, as a searched UPDATE's has none.
pub(super) const NO_SET_FUNCTION: &str = "[X3.135-1986 §5.8 SR1] a set function needs an argument source (a <table expression>), which a search condition of a MERGE does not have";

/// [ISO/IEC 9075-2:2023 §14.15 SR6, SR11b; §14.12 SR16] The rules a
/// `<set clause>` of a MERGE cites.
const MERGE_SET: SetRules = SetRules {
    set_function: "[ISO/IEC 9075-2:2023 §14.15 SR6] a value of a SET clause contains no set function",
    column: "ISO/IEC 9075-2:2023 §14.12 SR16",
    assign: AssignRule { rule: "ISO/IEC 9075-2:2023 §14.15 SR11b" },
};

/// [ISO/IEC 9075-2:2023 §14.12 SR18g] Store assignment of each merge
/// insert value to its insert object column.
const MERGE_ASSIGN: AssignRule = AssignRule { rule: "ISO/IEC 9075-2:2023 §14.12 SR18g" };

/// What the WHEN clauses are bound over: the rows of the target's columns
/// and the source's, and those of the source's alone, where the merge
/// insert value lists are [SR9; §7.6 SR20b].
pub(super) struct Scopes<'b, 'a> {
    /// The target first, then the source.
    pub(super) both: &'b Binder<'a>,
    /// The source alone, at the same places in the row.
    pub(super) source: &'b Binder<'a>,
    /// The base table changed.
    pub(super) base: &'b TableDesc,
    /// Where the first flag is in a row: past the target's and source's
    /// columns.
    pub(super) flags: usize,
}

/// A search condition of a MERGE, bound over the target's and source's
/// columns [SR9].
pub(super) fn condition(both: &Binder<'_>, c: &SearchCondition) -> Result<Cond> {
    both.search_condition(c, Scope::Rows(NO_SET_FUNCTION))
}

/// [GR6a] The `<merge when clause>`s `clauses`, in order; `inserts` the
/// object columns of each WHEN NOT MATCHED clause, in order. Each WHEN NOT
/// MATCHED clause of a search condition is given the next flag, in order,
/// and its condition is returned in that order, of which the flag is made
/// (`rows::flag`).
pub(super) fn bind_clauses(
    scopes: &Scopes<'_, '_>,
    clauses: &[MergeWhen],
    inserts: &[Vec<ObjectColumn>],
) -> Result<(Vec<BoundWhen>, Vec<Cond>)> {
    let (mut bound, mut flagged, mut inserted) = (Vec::new(), Vec::new(), 0_usize);
    for clause in clauses {
        match clause {
            MergeWhen::Matched { condition: written, action, at } => {
                let condition = written.as_ref().map(|c| condition(scopes.both, c)).transpose()?;
                let action = match action {
                    MatchedAction::Update(set) => BoundAction::Update(set_clauses(scopes.both, set).or_at(*at)?),
                    MatchedAction::Delete => BoundAction::Delete,
                };
                bound.push(BoundWhen::Matched { condition, action });
            }
            MergeWhen::NotMatched { condition: written, insert, .. } => {
                let objects = inserts.get(inserted).ok_or_else(|| {
                    SqlError::new(Code::Api, "[DESIGN] a MERGE is given the object columns of each WHEN NOT MATCHED clause")
                })?;
                inserted = inserted.saturating_add(1);
                let flag = match written {
                    Some(c) => {
                        flagged.push(condition(scopes.both, c)?);
                        Some(scopes.flags.saturating_add(flagged.len()).saturating_sub(1))
                    }
                    None => None,
                };
                let (object, values) = insert_values(scopes, insert, objects)?;
                bound.push(BoundWhen::NotMatched { flag, object, values });
            }
        }
    }
    Ok((bound, flagged))
}

/// [SR14, SR16; §14.15] The `<set clause list>` `set` of a WHEN MATCHED
/// clause: each object column's position in the base table and its new
/// value, in order, no column twice [§14.15 SR8]. Its values may draw
/// sequence values [§6.14 SR1e].
fn set_clauses(both: &Binder<'_>, set: &[SetClause]) -> Result<Vec<(usize, Option<Expr>)>> {
    both.drawing(true, || bind_set_list(both, set, &MERGE_SET))
}

/// [SR18] The merge insert value list of `insert` for its insert object
/// columns `objects`: each column's position in the base table, and its
/// value over a row's source columns, of NI values, one for each column
/// [SR18d], each assignable to its column [SR18g], NULL a null and
/// DEFAULT its default [SR18b], a `?` of its column's type [SR18e]. Its
/// values may draw sequence values [§6.14 SR1d].
fn insert_values(scopes: &Scopes<'_, '_>, insert: &MergeInsert, objects: &[ObjectColumn]) -> Result<(Vec<usize>, Vec<Expr>)> {
    let row = &insert.values;
    let wrong = |n: usize| {
        let msg = format!("[{CLAUSE} SR18d] the insert column list has {} columns and the insert value list {n} values", objects.len());
        rule(msg).or_at(row.at)
    };
    if row.elements.len() != objects.len() {
        return Err(wrong(row.elements.len()));
    }
    let binder = scopes.source;
    let taken = binder.dynamic_types(std::slice::from_ref(row), Some(objects))?;
    let bind_row = || binder.values_row(row, (Some(objects), &taken), MERGE_ASSIGN);
    let elements = binder.drawing(true, || binder.hosting(true, bind_row)).or_at(row.at)?;
    // A row subquery gives as many values as it has columns.
    if elements.len() != objects.len() {
        return Err(wrong(elements.len()));
    }
    let mut object = Vec::new();
    let mut values = Vec::new();
    for ((e, t), o) in elements.into_iter().zip(objects) {
        object.push(base_position(scopes.base, o)?);
        // A time or timestamp without time zone, of the session's
        // displacement, as a column WITH TIME ZONE takes it (`stored`).
        values.push(match t {
            Some(t) => stored(e, t, &o.desc.data_type, binder.now),
            None => e,
        });
    }
    Ok((object, values))
}

/// The position in the base table `base` of the insert object column
/// `o`, a column of it, as the db layer gives each (`db/write.rs`
/// `Target::objects`).
fn base_position(base: &TableDesc, o: &ObjectColumn) -> Result<usize> {
    base.column(&o.desc.name).ok_or_else(|| {
        SqlError::new(Code::Api, format!("[DESIGN] an insert object column of a MERGE, {}, is no column of {}", o.desc.name, base.name))
    })
}
