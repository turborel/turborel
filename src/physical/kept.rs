//! \[DESIGN\] Physical plans kept between the executions of a prepared
//! statement (RULES.md D45). A probe that fixes every column of a unique
//! index finds one row at most, so it is chosen without pricing another
//! path (`physical/access.rs`), by constants and by the statement's `?`s
//! alike: a null stands in a probe for each `?` until an execution fills
//! it. A plan of such a probe alone, under a filter or a projection, is
//! the plan every execution would make, whatever its values, so it is made
//! once, with its slots, and kept; each execution fills the slots, as
//! `logical/dynamic.rs` fills a logical plan's. Every other plan is chosen
//! by its values' statistics: a planner given an execution's values may
//! still make it with the slots, each `?` compared with a column standing
//! in its probe for the value (`ProbeSlot`), so another execution can fill
//! it, and the statement keeps such a plan once its executions keep making
//! one of the same shape (`kept/shape.rs`).

use super::Plan;
use super::access::{ix_is_point, whole_key};
use super::probe::{Compared, placed_as_is};
use crate::db::State;
use crate::error::{Code, Result, fail};
use crate::index::Probe;
use crate::logical::{Cond, Expr, Placing, Slot, filled_expr_as};
use crate::value::{CompOp, DataType, Value};

mod filled;
mod shape;

#[cfg(test)]
use filled::filled_plan;
pub(crate) use filled::{Kept, filled_plan_as_is, prepared_plan};

pub(crate) use shape::same_shape;

/// A `?` of a probe: the slot, and the place among the probe's values its
/// value takes; or a sum or difference of it and a constant, which raises
/// nothing (`Expr::can_raise`), evaluated as it is placed (RULES.md D613).
#[derive(Debug, Clone)]
pub struct ProbeSlot {
    /// The place among the probe's values.
    pub place: Place,
    /// The slot, whose type the value is cast to.
    pub slot: Slot,
    /// The expression of the slot whose value is placed, if not the slot
    /// alone.
    pub expr: Option<Box<Expr>>,
}

impl ProbeSlot {
    /// The slot `slot`'s value placed at `place`.
    pub fn of(place: Place, slot: Slot) -> ProbeSlot {
        ProbeSlot { place, slot, expr: None }
    }
}

impl PartialEq for ProbeSlot {
    /// One place and slot, and expressions written alike, as an
    /// expression compares no other way.
    fn eq(&self, other: &ProbeSlot) -> bool {
        let written = |e: &Option<Box<Expr>>| e.as_ref().map(|e| format!("{e:?}"));
        self.place == other.place && self.slot == other.slot && written(&self.expr) == written(&other.expr)
    }
}

/// Where a `?`'s value goes in a probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// The value of this comparison in `Probe::Columns`.
    Column(usize),
    /// The value compared with the index's column of this place, from 0:
    /// a `Probe::Compare`'s, one of the equal values of a `Probe::Prefix`
    /// or a `Probe::Range`, or the comparison after a prefix's.
    Key(usize),
    /// The lower bound of a `Probe::Range`.
    Low,
    /// The upper bound of a `Probe::Range`.
    High,
    /// The constant or parameter argument of `Probe::Predicate` (RULES.md D34).
    PredicateValue,
    /// \[EXTENSION\] A field of the ROW of values of a json index's
    /// `Probe::Predicate` (`Predicate::argument` none).
    PredicateField(usize),
}

/// The probe fixing every column of an index on `columns`, of `types`, by
/// the equalities among `conds`: of a constant, one of `compared`, placed
/// as the index places it; or of a `?`, a null standing in for its value,
/// its slot noted. With the conjuncts it answers. None if a column has
/// neither.
pub(super) fn point_probe(
    conds: &[Cond],
    compared: &[Compared],
    (columns, types): (&[usize], &[DataType]),
) -> Option<(Probe, Vec<ProbeSlot>, Vec<usize>)> {
    let (mut equal, mut slots, mut used) = (Vec::new(), Vec::new(), Vec::new());
    for (at, (&column, data_type)) in columns.iter().zip(types).enumerate() {
        if let Some((value, answered)) = constant_equal(compared, column, data_type) {
            slots.extend(answered.slot_at(Place::Key(at)));
            equal.push(value);
            used.push(answered.at);
        } else if let Some((slot, answered)) = slot_equal(conds, column, data_type) {
            equal.push(Value::Null);
            slots.push(ProbeSlot::of(Place::Key(at), slot));
            used.push(answered);
        } else {
            return None;
        }
    }
    // An index of one column is asked by a comparison, which every method
    // answers that supports equality.
    let probe = match (equal.as_slice(), columns.len()) {
        ([only], 1) => Probe::Compare { op: CompOp::Eq, value: only.clone() },
        _ => Probe::Prefix { equal, then: None },
    };
    Some((probe, slots, used))
}

/// The value a constant among `compared` holds `column`, of `data_type`,
/// equal to, placed as an index on it places it, and its comparison: of a
/// `?`'s value, only as it is (`Compared::placed`).
fn constant_equal<'c>(compared: &'c [Compared], column: usize, data_type: &DataType) -> Option<(Value, &'c Compared)> {
    compared.iter().filter(|c| c.column == column && c.exact && c.op == CompOp::Eq).find_map(|c| {
        let placed = c.placed(data_type)?;
        match placed.as_slice() {
            [(CompOp::Eq, value)] => Some((value.clone(), c)),
            // A value between two of the column's equals none of them.
            _ => None,
        }
    })
}

/// The slot a conjunct of `conds` holds `column`, of `data_type`, equal
/// to, if its type's values are placed in an index on the column as they
/// are, and its conjunct.
fn slot_equal(conds: &[Cond], column: usize, data_type: &DataType) -> Option<(Slot, usize)> {
    conds.iter().enumerate().find_map(|(at, c)| {
        let slot = match c {
            Cond::Compare(Expr::Column(col), CompOp::Eq, Expr::Dynamic(slot))
            | Cond::Compare(Expr::Dynamic(slot), CompOp::Eq, Expr::Column(col))
                if *col == column =>
            {
                slot
            }
            Cond::Compare(..)
            | Cond::Distinct(..)
            | Cond::Const(_)
            | Cond::Or(..)
            | Cond::And(..)
            | Cond::Not(_)
            | Cond::Is(..)
            | Cond::IsNull(..)
            | Cond::In(..)
            | Cond::Like(..)
            | Cond::Exists(_)
            | Cond::Unique(..)
            | Cond::Quantified(..)
            | Cond::Match { .. }
            | Cond::Value(_) => return None,
        };
        placed_as_is(&slot.data_type, data_type).then_some((slot.clone(), at))
    })
}

/// Whether `plan` is the plan every execution of its statement would make,
/// whatever its values: one probe fixing every column of a unique index,
/// under a filter, a projection or a limit, or none. The index is read from
/// `st`, the state it was planned in.
pub(crate) fn keepable(plan: &Plan, st: &State) -> bool {
    match plan {
        Plan::Project { input, .. } | Plan::Filter { input, .. } | Plan::Limit { input, .. } => keepable(input, st),
        Plan::IndexScan { index, probe, .. } => {
            st.catalog.index(index).is_some_and(|ix| ix_is_point(ix) && whole_key(probe, ix.columns.len()))
        }
        Plan::SeqScan { .. }
        | Plan::Nearest(_)
        | Plan::Product { .. }
        | Plan::HashJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::RangeJoin(_)
        | Plan::SemiJoin(_)
        | Plan::MarkJoin(_)
        | Plan::OuterJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::System { .. }
        | Plan::Values { .. }
        | Plan::Append { .. }
        | Plan::MergeAppend { .. }
        | Plan::Sort { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Parallel(..)
        | Plan::Counted(_) => false,
    }
}

/// `probe`, each of `slots` placed with its value of `values`: as it is
/// if it has none, else a copy (RULES.md D610).
///
/// # Errors
///
/// As `place`.
pub(crate) fn placed<'p>(probe: &'p Probe, slots: &[ProbeSlot], values: &[Value]) -> Result<std::borrow::Cow<'p, Probe>> {
    if slots.is_empty() {
        return Ok(std::borrow::Cow::Borrowed(probe));
    }
    let mut copy = probe.clone();
    for s in slots {
        place(&mut copy, s, values, Placing::Cast)?;
    }
    Ok(std::borrow::Cow::Owned(copy))
}

/// `plan`, an index scan, each `?` of its probe filled with its value,
/// as `placing` says, so it has no slot left to place as it opens
/// (RULES.md D610).
fn filled_scan(mut plan: Plan, values: &[Value], placing: Placing) -> Result<Plan> {
    if let Plan::IndexScan { probe, slots, .. } = &mut plan {
        for s in std::mem::take(slots) {
            place(probe, &s, values, placing)?;
        }
    }
    Ok(plan)
}

/// The value of `s`'s slot, cast to its type as a logical plan's slots
/// are, or as it is, as `placing` says; or of its expression of the slot.
///
/// # Errors
///
/// As `logical::filled`; SQLCODE -910 if the slot is not filled by a
/// constant.
fn slot_value(s: &ProbeSlot, values: &[Value], placing: Placing) -> Result<Value> {
    let expr = s.expr.as_deref().map_or_else(|| Expr::Dynamic(s.slot.clone()), Clone::clone);
    evaluated(expr, values, placing)
}

/// The value of `expr`, a slot or a sum or difference of one and a
/// constant, its slot filled from `values` as `placing` says and then
/// evaluated, as a plan made with the values folds it.
///
/// # Errors
///
/// As `logical::filled` and `exec::eval`; SQLCODE -910 if it is no
/// constant once filled and evaluated.
pub(super) fn evaluated(expr: Expr, values: &[Value], placing: Placing) -> Result<Value> {
    Ok(match filled_expr_as(expr, values, placing)? {
        Expr::Const(value) => value,
        e @ Expr::Arith(..) => crate::exec::eval(&e, &Vec::new(), &crate::exec::NoSubqueries)?,
        Expr::Column(_)
        | Expr::Dynamic(_)
        | Expr::Neg(_)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Cast(..)
        | Expr::Case(..)
        | Expr::Subquery(..)
        | Expr::SortKey(..)
        | Expr::Domain(..)
        | Expr::Warned(..)
        | Expr::Truth(_)
        | Expr::NextValue(_)
        | Expr::Invoked(_) => return fail(Code::Api, "[DESIGN] a slot filled is a constant"),
    })
}

/// Puts the value of `s`'s slot, cast to its type as a logical plan's
/// slots are or as it is (`slot_value`), in its place in `probe`.
///
/// # Errors
///
/// As `logical::filled`; SQLCODE -910 if the probe has no such place.
fn place(probe: &mut Probe, s: &ProbeSlot, values: &[Value], placing: Placing) -> Result<()> {
    let value = slot_value(s, values, placing)?;
    if let (Probe::Predicate { value: Value::Row(fields), .. }, Place::PredicateField(n)) = (&mut *probe, s.place) {
        let mut each = fields.to_vec();
        let Some(field) = each.get_mut(n) else {
            return fail(Code::Api, "[DESIGN] a kept probe has no place for a <dynamic parameter specification>");
        };
        *field = value;
        *fields = each.into();
        return Ok(());
    }
    let at = match (probe, s.place) {
        (Probe::Columns(each), Place::Column(n)) => each.get_mut(n).map(|c| &mut c.value),
        (Probe::Compare { value: first, .. }, Place::Key(n)) => (n == 0).then_some(first),
        (Probe::Prefix { equal, then }, Place::Key(n)) => {
            if n == equal.len() {
                then.as_mut().map(|(_, bound)| bound)
            } else {
                equal.get_mut(n)
            }
        }
        (Probe::Range { equal, .. }, Place::Key(n)) => equal.get_mut(n),
        (Probe::Range { low, .. }, Place::Low) => Some(&mut low.1),
        (Probe::Range { high, .. }, Place::High) => Some(&mut high.1),
        (Probe::Predicate { value: argument, .. }, Place::PredicateValue) => Some(argument),
        (Probe::All, Place::Column(_) | Place::Key(_) | Place::Low | Place::High | Place::PredicateValue)
        | (Probe::Columns(_), Place::Key(_) | Place::Low | Place::High | Place::PredicateValue)
        | (Probe::Compare { .. } | Probe::Prefix { .. } | Probe::Range { .. }, Place::Column(_) | Place::PredicateValue)
        | (Probe::Compare { .. } | Probe::Prefix { .. }, Place::Low | Place::High)
        | (Probe::Predicate { .. }, Place::Column(_) | Place::Key(_) | Place::Low | Place::High)
        | (Probe::Xml(_), Place::Column(_) | Place::Key(_) | Place::Low | Place::High | Place::PredicateValue | Place::PredicateField(_))
        | (
            Probe::All | Probe::Columns(_) | Probe::Compare { .. } | Probe::Prefix { .. } | Probe::Range { .. } | Probe::Predicate { .. },
            Place::PredicateField(_),
        ) => None,
    };
    match at {
        Some(at) => {
            *at = value;
            Ok(())
        }
        None => fail(Code::Api, "[DESIGN] a kept probe has no place for a <dynamic parameter specification>"),
    }
}

// Unit tests: plans made with no table, and the conditions of keeping one
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
