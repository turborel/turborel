//! The leaves of a program evaluated over its frame, and what assigns their
//! values: the assignment statement [ISO/IEC 9075-4:2006 draft §14.5] and
//! SELECT INTO [ISO/IEC 9075-2:2023 §14.7].
//!
//! A leaf's outer expressions, the slots of the frame it reads, are filled
//! with the values the frame holds, and the leaf runs as a correlated
//! subquery does of them; one that is a VALUES of one row is evaluated
//! where it is, as a function's `<return statement>` is
//! (`exec/invoked.rs`).

use std::sync::Arc;

use super::Host;
use super::framed::Framed;
use crate::db::{Completion, assign_targets};
use crate::error::{Code, Result, SqlError, SqlState, fail, fail_with};
use crate::exec::{Subqueries, eval};
use crate::logical::{Assign, Expr, Into, Leaf, LogicalPlan, Modified, Target};
use crate::params::Params;
use crate::storage::Row;
use crate::value::types::Structured;
use crate::value::{Value, assign};

/// [ISO/IEC 9075-2:2023 §9.2] Store assignment, as an assignment statement
/// assigns.
const STORE: &str = "ISO/IEC 9075-2:2023 §9.2";

/// The rows `leaf` gives over `frame`: at most `limit`, if given.
///
/// # Errors
///
/// The leaf's.
pub(crate) fn rows(subqueries: &dyn Subqueries, leaf: &Leaf, frame: &Params, limit: Option<usize>) -> Result<Vec<Row>> {
    evaluated(subqueries, leaf, frame, limit).map_err(|e| e.or_at(leaf.at))
}

/// `rows`' work.
fn evaluated(subqueries: &dyn Subqueries, leaf: &Leaf, frame: &Params, limit: Option<usize>) -> Result<Vec<Row>> {
    let framed = Framed { inner: subqueries, frame: frame.dynamic_values(), outer: &[] };
    let outer = leaf.query.outer.iter().map(|e| eval(e, &Vec::new(), &framed)).collect::<Result<Vec<Value>>>()?;
    if let Some(row) = values_row(&leaf.query.plan) {
        // A CALL's arguments, bound with the program, are slots of the frame
        // themselves (`binder/psm/inline.rs`); the rest read the leaf's
        // outer values where they are (RULES.md D610).
        let framed = Framed { outer: &outer, ..framed };
        let row = row.iter().map(|e| eval(e, &Vec::new(), &framed)).collect::<Result<Row>>()?;
        return Ok(vec![row]);
    }
    let found = subqueries.rows(&leaf.query, &outer, limit)?;
    match limit {
        Some(n) => found.first(n),
        None => found.rows()?.collect(),
    }
}

/// The one expression of each column of a plan that is a VALUES of one
/// row, evaluated where it is.
fn values_row(plan: &LogicalPlan) -> Option<&[Expr]> {
    let LogicalPlan::Values { rows, .. } = plan else { return None };
    let [row] = rows.as_slice() else { return None };
    Some(row)
}

/// The value of `leaf`, a value of the program: of the first column of its
/// row, the null value if it has none.
///
/// # Errors
///
/// The leaf's.
pub(super) fn value<H: Host>(host: &H, leaf: &Leaf, frame: &Params) -> Result<Value> {
    Ok(host.rows(leaf, frame, Some(1))?.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or(Value::Null))
}

/// Whether `leaf`, a condition of the program, is true: neither false nor
/// unknown.
///
/// # Errors
///
/// The leaf's.
pub(super) fn truth<H: Host>(host: &H, leaf: &Leaf, frame: &Params) -> Result<bool> {
    Ok(matches!(value(host, leaf, frame)?, Value::Bool(true)))
}

/// [ISO/IEC 9075-4:2006 draft §14.5] The assignment statement `a`: its
/// source's row, of as many values as it has targets [SR1], the first a
/// query gives and of none the null values, each value assigned to its
/// target in turn.
///
/// # Errors
///
/// SQLSTATE 21000 if its subquery gives more than one row; 2202A if a field
/// is assigned of a null row [GR4a]; the store assignment's.
pub(super) fn assignment<H: Host>(host: &H, a: &Assign, frame: &mut Params) -> Result<Completion> {
    let row = match &a.source {
        None => vec![Value::Null; a.targets.len()],
        Some(leaf) => match host.rows(leaf, frame, Some(2))?.as_mut_slice() {
            [] => vec![Value::Null; a.targets.len()],
            [row] => std::mem::take(row),
            [_, _, ..] => {
                return fail(
                    Code::Cardinality,
                    "[ISO/IEC 9075:1992 §7.11 GR1; ISO/IEC 9075-4:2006 draft §14.5 SR1f] the assigned row's subquery gives more than one row",
                );
            }
        },
    };
    frame.set_routine(Arc::clone(&a.scope));
    for (target, value) in a.targets.iter().zip(row) {
        assign_target(target, value, frame).map_err(|e| e.or_at(target.spec.at).or_at(a.at))?;
    }
    Ok(Completion::Success)
}

/// [§14.5 GR1, GR3 to GR5] `value` assigned to `target`: of a field, the
/// row its target holds with that field the value [GR4b].
fn assign_target(target: &Target, value: Value, frame: &mut Params) -> Result<()> {
    let spec = &target.spec;
    let value = if target.fields.is_empty() {
        value
    } else {
        let held = frame
            .routine_value(spec.qualifier.as_deref(), &spec.name)
            .or_else(|| frame.transition_value(&spec.name))
            .unwrap_or(Value::Null);
        with_field(held, &target.fields, assign(value, &target.data_type, STORE)?)?
    };
    assign_targets(&[value], std::slice::from_ref(spec), frame, STORE, STORE).map(|_| ())
}

/// The value `held` with the field or attribute at `path` `value`, each
/// part of the path one of the value of the one before: of a structured
/// value, as its mutator gives it, of its most specific type
/// [§14.5 SR5; ISO/IEC 9075-2:2023 §4.9.3.3].
///
/// # Errors
///
/// SQLSTATE 2202H of a field of a null row [GR4a]; 2202D of an attribute
/// of a null value, as its mutator raises [ISO/IEC 9075-2:2023 §9.18 GR4].
fn with_field(held: Value, path: &[Modified], value: Value) -> Result<Value> {
    let Some((first, rest)) = path.split_first() else { return Ok(value) };
    match (*first, held) {
        (Modified::Field(k), Value::Row(fields)) => {
            let mut fields = fields.to_vec();
            let slot = fields.get_mut(k).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] the row has no such field"))?;
            *slot = with_field(std::mem::replace(slot, Value::Null), rest, value)?;
            Ok(Value::Row(fields.into()))
        }
        (Modified::Attribute(k), Value::Structured(s)) => {
            let mut attributes = s.attributes.to_vec();
            let slot = attributes.get_mut(k).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] the value has no such attribute"))?;
            *slot = with_field(std::mem::replace(slot, Value::Null), rest, value)?;
            Ok(Value::Structured(Arc::new(Structured::new(Arc::clone(&s.of), attributes.into()))))
        }
        (Modified::Attribute(_), _) => {
            let msg = "[ISO/IEC 9075-2:2023 §9.18 GR4; ISO/IEC 9075-4:2006 draft §14.5 SR5] an attribute of a null value is assigned";
            fail_with(Code::Data, SqlState::NULL_MUTATOR_SUBJECT, msg.to_owned())
        }
        (Modified::Field(_), _) => {
            let msg = "[ISO/IEC 9075-4:2006 draft §14.5 GR4a] a field of a null row is assigned";
            fail_with(Code::Data, SqlState::NULL_VALUE_IN_FIELD_REFERENCE, msg.to_owned())
        }
    }
}

/// [ISO/IEC 9075-2:2023 §14.7] SELECT INTO `i`: of no row, no data, its
/// targets not assigned [GR2]; of one, its values assigned to them.
///
/// # Errors
///
/// SQLSTATE 21000 of more than one row; the assignment's.
pub(super) fn select_into<H: Host>(host: &H, i: &Into, frame: &mut Params) -> Result<Completion> {
    let mut found = host.rows(&i.query, frame, Some(2))?;
    match found.as_mut_slice() {
        [] => Ok(Completion::NoData),
        [row] => {
            host.present(row);
            frame.set_routine(Arc::clone(&i.scope));
            assign_targets(row, &i.targets, frame, "X3.135-1986 §8.10 GR8", "X3.135-1986 §8.10 GR9")
        }
        [_, _, ..] => fail(Code::Cardinality, "[X3.135-1986 §8.10 GR2] the result has more than one row"),
    }
}
