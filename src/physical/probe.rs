//! \[DESIGN\] The probes an index can answer for the conjuncts of a filter
//! over its table (RULES.md D34): comparisons of a column with constants,
//! the bounds of a LIKE's prefix, and an OR of these of one column, an IN
//! list among them, as one probe of each of its ranges. A planner given a
//! prepared statement's values takes a comparison of a column with a `?`
//! as one with its value, noting the slot (RULES.md D45).

use std::cmp::Ordering;

use super::estimate::{self, Known};
use super::kept::{Place, ProbeSlot, evaluated};
use crate::catalog::IndexDesc;
use crate::index::Probe;
use crate::logical::{Cond, Expr, Placing, Slot, compared_with_constant, disjuncts, like_bounds, split_and};
use crate::value::collation::CollationRef;
use crate::value::{ArithOp, CompOp, DataType, DecFloat, Decimal, Value, compare};

mod shown;
pub(super) use shown::render_probe;

/// A comparison of a column with a constant an index may answer: of the
/// conjunct `at`, answering it `exact`ly, or else only bounding the rows it
/// keeps, as a LIKE's prefix does, so that a filter still judges it.
#[derive(Clone)]
pub(super) struct Compared {
    /// The collation it compares by, if not the default: an index serves it
    /// only if its key is by the same (RULES.md D60).
    pub(super) collation: Option<CollationRef>,
    pub(super) at: usize,
    pub(super) exact: bool,
    pub(super) column: usize,
    pub(super) op: CompOp,
    pub(super) value: Value,
    /// The `?` the value is of, a prepared statement's, if it is one's.
    pub(super) slot: Option<Slot>,
    /// The sum or difference of the `?` and a constant the value is of, if
    /// not the `?` alone (`slot_bound`).
    pub(super) expr: Option<Box<Expr>>,
}

impl Compared {
    /// The comparisons of a column of type `column` an index places for
    /// it (`coerce`); of a `?`'s value, only itself, as another
    /// execution's value is placed (`placed_as_is`), else none.
    pub(super) fn placed(&self, column: &DataType) -> Option<Comparisons> {
        if self.slot.as_ref().is_some_and(|s| !placed_as_is(&s.data_type, column)) {
            return None;
        }
        coerce(self.op, &self.value, column)
    }

    /// Its `?`, standing at `place` in a probe, if its value is one's.
    pub(super) fn slot_at(&self, place: Place) -> Option<ProbeSlot> {
        self.slot.clone().map(|slot| ProbeSlot { place, slot, expr: self.expr.clone() })
    }
}

/// A conjunct `at` that is an OR of comparisons of one column with
/// constants (`bounds`), as an IN list of constants is bound: the
/// comparisons of each operand, whether they answer it exactly, and the
/// share of the rows it keeps.
pub(super) struct Alternatives {
    pub(super) at: usize,
    pub(super) column: usize,
    pub(super) exact: bool,
    pub(super) each: Vec<Vec<(CompOp, Value)>>,
    pub(super) share: f64,
}

/// Comparisons of a column with constants: each operator and constant.
type Comparisons = Vec<(CompOp, Value)>;

/// The comparisons and ORs of them among `conds`: with a `?` too, given
/// the `values` of an execution.
pub(super) fn comparisons(conds: &[Cond], known: Known<'_>, values: Option<&[Value]>) -> (Vec<Compared>, Vec<Alternatives>) {
    let (mut compared, mut ors) = (Vec::new(), Vec::new());
    for (at, c) in conds.iter().enumerate() {
        if let Some((column, op, value, collation)) = collated_bound(c) {
            compared.push(Compared { collation: Some(collation), at, exact: true, column, op, value, slot: None, expr: None });
        } else if let Some(((column, op), (slot, expr), value)) = values.and_then(|v| slot_bound(c, v)) {
            compared.push(Compared { collation: None, at, exact: true, column, op, value, slot: Some(slot), expr });
        } else if let Some((column, exact, each)) = bounds(c) {
            let of = |(op, value)| Compared { collation: None, at, exact, column, op, value, slot: None, expr: None };
            compared.extend(each.into_iter().map(of));
        } else if let Some((column, exact, each)) = alternatives(c) {
            ors.push(Alternatives { at, column, exact, each, share: estimate::selectivity(c, known) });
        }
    }
    (compared, ors)
}

/// `c`, the sort key of a column compared with a constant's: the column,
/// the operator, the constant's key and the collation.
fn collated_bound(c: &Cond) -> Option<(usize, CompOp, Value, CollationRef)> {
    let Cond::Compare(left, op, right) = c else { return None };
    let (key, constant, op) = match (left, right) {
        (Expr::SortKey(..), Expr::Const(_)) => (left, right, *op),
        (Expr::Const(_), Expr::SortKey(..)) => (right, left, op.flip()),
        _ => return None,
    };
    let (Expr::SortKey(column, collation), Expr::Const(value)) = (key, constant) else { return None };
    let Expr::Column(column) = **column else { return None };
    Some((column, op, value.clone(), collation.clone()))
}

/// `c`, a column compared with a `?`, or with a sum or difference of one
/// and a constant that raises nothing (`Expr::can_raise`, RULES.md D613):
/// the column and the operator, the slot and the expression if not the
/// slot alone, and its value among `values`, cast to the slot's type as
/// filling casts it and evaluated as folding would; none if it is not one,
/// or the value does not cast, which filling the plan then reports.
fn slot_bound(c: &Cond, values: &[Value]) -> Option<SlotBound> {
    let Cond::Compare(left, op, right) = c else { return None };
    let (column, op, other) = match (left, right) {
        (Expr::Column(column), other) => (*column, *op, other),
        (other, Expr::Column(column)) => (*column, op.flip(), other),
        _ => return None,
    };
    let (slot, expr) = if let Expr::Dynamic(slot) = other {
        (slot.clone(), None)
    } else if let Expr::Arith(a, ArithOp::Add | ArithOp::Sub, b) = other {
        (shifted_slot(a, b)?, Some(Box::new(other.clone())))
    } else {
        return None;
    };
    if expr.as_ref().is_some_and(|e| e.can_raise()) {
        return None;
    }
    let shown = expr.as_deref().map_or_else(|| Expr::Dynamic(slot.clone()), Clone::clone);
    let value = evaluated(shown, values, Placing::Cast).ok()?;
    Some(((column, op), (slot, expr), value))
}

/// A comparison `slot_bound` finds: the column and operator, the slot and
/// its expression if not the slot alone, and its value.
type SlotBound = ((usize, CompOp), (Slot, Option<Box<Expr>>), Value);

/// The slot of `a op b`, a sum or difference, if one is a slot and the
/// other a constant.
fn shifted_slot(a: &Expr, b: &Expr) -> Option<Slot> {
    match (a, b) {
        (Expr::Dynamic(slot), Expr::Const(_)) | (Expr::Const(_), Expr::Dynamic(slot)) => Some(slot.clone()),
        _ => None,
    }
}

/// Of the comparisons `all`, those an index `ix` serves: of a column of its
/// key, by the collation the key is by; so no probe of raw text reaches an
/// index of sort keys, nor one of sort keys an index of text.
pub(super) fn served_by(all: &[Compared], ix: &IndexDesc) -> Vec<Compared> {
    let serves = |c: &&Compared| {
        let keyed = ix.columns.iter().position(|k| *k == c.column).and_then(|at| ix.collations.get(at)).and_then(Option::as_ref);
        match (keyed, &c.collation) {
            (None, None) => true,
            (Some(a), Some(b)) => a.behaves_as(b),
            (Some(_), None) | (None, Some(_)) => false,
        }
    };
    all.iter().filter(serves).cloned().collect()
}

/// `c` as comparisons of one column with constants, each true of every
/// row it keeps: itself, if it is one, exactly; a LIKE's bounds
/// (`like_bounds`), not exactly.
fn bounds(c: &Cond) -> Option<(usize, bool, Comparisons)> {
    if let Some((column, op, value)) = compared_with_constant(c) {
        return Some((column, true, vec![(op, value.clone())]));
    }
    let Cond::Like(Expr::Column(column), pattern, escape) = c else { return None };
    let each = like_bounds(pattern, escape.as_ref());
    (!each.is_empty()).then_some((*column, false, each))
}

/// `c`, an OR, as the comparisons (`bounds`) of each of its operands, or
/// of each conjunct of one, of one column, and whether they answer each
/// exactly.
fn alternatives(c: &Cond) -> Option<(usize, bool, Vec<Comparisons>)> {
    let Cond::Or(..) = c else { return None };
    let mut operands = Vec::new();
    disjuncts(c, &mut operands);
    let (mut column, mut exact, mut each) = (None, true, Vec::new());
    for operand in operands {
        // An operand may be a conjunction of them, as BETWEEN is.
        let mut conjuncts = Vec::new();
        split_and(operand.clone(), &mut conjuncts);
        let mut comparisons = Vec::new();
        for conjunct in &conjuncts {
            let (of, answered, more) = bounds(conjunct)?;
            if *column.get_or_insert(of) != of {
                return None;
            }
            exact &= answered;
            comparisons.extend(more);
        }
        each.push(comparisons);
    }
    Some((column?, exact, each))
}

/// The values of a column between two bounds, either open: above `low`
/// by its operator, `>` or `>=`, and below `high` by its, `<` or `<=`.
pub(super) struct Interval {
    low: Option<(CompOp, Value)>,
    high: Option<(CompOp, Value)>,
}

impl Interval {
    /// The values all of `each` (`column op value`) are true of; none if
    /// one is `<>`, which no interval is.
    fn of(each: &[(CompOp, Value)]) -> Option<Interval> {
        let mut out = Interval { low: None, high: None };
        for (op, value) in each {
            match op {
                CompOp::Eq => {
                    out.low = Some(tighter(out.low.take(), (CompOp::Ge, value.clone()), Ordering::Greater));
                    out.high = Some(tighter(out.high.take(), (CompOp::Le, value.clone()), Ordering::Less));
                }
                CompOp::Gt | CompOp::Ge => out.low = Some(tighter(out.low.take(), (*op, value.clone()), Ordering::Greater)),
                CompOp::Lt | CompOp::Le => out.high = Some(tighter(out.high.take(), (*op, value.clone()), Ordering::Less)),
                CompOp::Ne => return None,
            }
        }
        Some(out)
    }

    /// Whether no value is between its bounds.
    fn is_empty(&self) -> bool {
        let (Some((low_op, low)), Some((high_op, high))) = (&self.low, &self.high) else { return false };
        match compare(low, high) {
            Some(Ordering::Greater) => true,
            Some(Ordering::Equal) => *low_op == CompOp::Gt || *high_op == CompOp::Lt,
            Some(Ordering::Less) | None => false,
        }
    }

    /// Whether `next`, whose lower bound is not below this one's, has a
    /// value in common with it or just after its values.
    fn touches(&self, next: &Interval) -> bool {
        let (Some((high_op, high)), Some((low_op, low))) = (&self.high, &next.low) else { return true };
        match compare(low, high) {
            Some(Ordering::Less) => true,
            Some(Ordering::Equal) => *high_op == CompOp::Le || *low_op == CompOp::Ge,
            Some(Ordering::Greater) | None => false,
        }
    }

    /// The probe of the values `equal` of an index's first columns, then
    /// of this interval of the next; none if it has no bound.
    fn probe(self, equal: &[Value]) -> Option<Probe> {
        if let (Some((CompOp::Ge, low)), Some((CompOp::Le, high))) = (&self.low, &self.high)
            && compare(low, high) == Some(Ordering::Equal)
        {
            return Some(match equal {
                [] => Probe::Compare { op: CompOp::Eq, value: low.clone() },
                _ => Probe::Prefix { equal: [equal, std::slice::from_ref(low)].concat(), then: None },
            });
        }
        Some(match (self.low, self.high, equal) {
            (Some(low), Some(high), _) => Probe::Range { equal: equal.to_vec(), low: Box::new(low), high: Box::new(high) },
            (Some((op, value)), None, []) | (None, Some((op, value)), []) => Probe::Compare { op, value },
            (Some(bound), None, _) | (None, Some(bound), _) => Probe::Prefix { equal: equal.to_vec(), then: Some(bound) },
            (None, None, _) => return None,
        })
    }
}

/// Of two bounds on one side, the one that leaves fewer values: the
/// greater value of lower bounds (`further` Greater), the less of upper
/// ones; of one value, the bound that leaves it out.
fn tighter(was: Option<(CompOp, Value)>, bound: (CompOp, Value), further: Ordering) -> (CompOp, Value) {
    let Some(was) = was else { return bound };
    match compare(&bound.1, &was.1) {
        Some(Ordering::Equal) => {
            if matches!(bound.0, CompOp::Gt | CompOp::Lt) {
                bound
            } else {
                was
            }
        }
        Some(o) => {
            if o == further {
                bound
            } else {
                was
            }
        }
        None => was,
    }
}

/// `intervals` as few apart: sorted by their lower bounds, each joined
/// with those that touch it. So no value is in two of them, and a probe of
/// each finds each row once.
fn merged(mut intervals: Vec<Interval>) -> Vec<Interval> {
    intervals.sort_by(|a, b| match (&a.low, &b.low) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some((a_op, a)), Some((b_op, b))) => {
            compare(a, b).unwrap_or(Ordering::Equal).then((*a_op == CompOp::Gt).cmp(&(*b_op == CompOp::Gt)))
        }
    });
    let mut out: Vec<Interval> = Vec::new();
    for next in intervals {
        match out.last_mut() {
            Some(last) => {
                if last.touches(&next) {
                    last.high = higher(last.high.take(), next.high);
                } else {
                    out.push(next);
                }
            }
            None => out.push(next),
        }
    }
    out
}

/// Of two upper bounds, the one that leaves more values: none, which
/// leaves every one, or the greater value; of one value, the bound that
/// keeps it.
fn higher(a: Option<(CompOp, Value)>, b: Option<(CompOp, Value)>) -> Option<(CompOp, Value)> {
    let (a, b) = (a?, b?);
    Some(match compare(&a.1, &b.1) {
        Some(Ordering::Greater) => a,
        Some(Ordering::Less) => b,
        Some(Ordering::Equal) | None => {
            if a.0 == CompOp::Le {
                a
            } else {
                b
            }
        }
    })
}

/// The probes of the values `equal` of an index's first columns, then of
/// the next, of type `data_type`, by any of `alternatives`: one for each of
/// its ranges, merged so that none overlaps another, in the order of their
/// values, so that their rows, appended, are in the index's order; none if
/// one has no bound or a comparison none an index places (`coerce`).
pub(super) fn probes_of_any(equal: &[Value], data_type: &DataType, alternatives: &Alternatives) -> Option<Vec<Probe>> {
    let mut intervals = Vec::new();
    for each in &alternatives.each {
        let coerced: Vec<Vec<(CompOp, Value)>> = each.iter().map(|(op, value)| coerce(*op, value, data_type)).collect::<Option<_>>()?;
        let interval = Interval::of(&coerced.concat())?;
        if !interval.is_empty() {
            intervals.push(interval);
        }
    }
    let probes: Vec<Probe> = merged(intervals).into_iter().map(|i| i.probe(equal)).collect::<Option<_>>()?;
    (!probes.is_empty()).then_some(probes)
}

/// How many steps from a decimal's nearest `f64`, as `Decimal::to_f64`
/// gives it, the `f64`s on either side of it are looked for: it is within
/// a few, rounded twice at most.
const NEIGHBOUR_STEPS: usize = 8;

/// Where a constant falls among a column's values: one of them, or
/// between two with none between.
enum Placed {
    At(Value),
    Between(Value, Value),
}

/// "column `op` `value`", the column of type `column`, as comparisons with
/// values an index on it places as SQL compares them [X3.135-1986 §5.11],
/// true of the same values of the column: itself, of a value of its kind;
/// of a number of the other kind, comparisons with the column's values on
/// either side of it, the operator turned to keep the same values. None if
/// there are none such.
pub(super) fn coerce(op: CompOp, value: &Value, column: &DataType) -> Option<Vec<(CompOp, Value)>> {
    let placed = match (column, value) {
        (DataType::Character { .. } | DataType::Varying { .. }, Value::Char(_))
        | (DataType::Exact { .. }, Value::Exact(_))
        | (DataType::Approximate { .. }, Value::Approx(_))
        | (DataType::Binary { .. } | DataType::VarBinary { .. }, Value::Binary(_))
        | (DataType::Boolean, Value::Bool(_))
        | (DataType::Date | DataType::Time { .. } | DataType::Timestamp { .. }, Value::Datetime(_))
        | (DataType::DecFloat { .. }, Value::DecFloat(_))
        | (DataType::Interval(_), Value::Interval(_)) => Placed::At(value.clone()),
        // [ISO/IEC 9075-2:2023 §4.5.2] A DECFLOAT value holds an exact
        // one's digits exactly, so it is that value among the column's.
        (DataType::DecFloat { .. }, Value::Exact(d)) => Placed::At(Value::DecFloat(DecFloat::of_decimal(d)?)),
        (DataType::Approximate { .. }, Value::Exact(d)) => approximate_place(d)?,
        (DataType::Exact { scale, .. }, Value::Approx(f)) => exact_place(*f, *scale)?,
        _ => return None,
    };
    match (placed, op) {
        (Placed::At(at), _) => Some(vec![(op, at)]),
        // No value of the column equals one between two of them.
        (Placed::Between(below, above), CompOp::Eq) => Some(vec![(CompOp::Gt, below), (CompOp::Lt, above)]),
        (Placed::Between(below, _), CompOp::Lt | CompOp::Le) => Some(vec![(CompOp::Le, below)]),
        (Placed::Between(_, above), CompOp::Gt | CompOp::Ge) => Some(vec![(CompOp::Ge, above)]),
        (Placed::Between(..), CompOp::Ne) => None,
    }
}

/// Whether a value of type `given` is placed among the values of a column
/// of type `column` as it is (`coerce`), so a `?` of that type may stand
/// in a probe of the column for its value (`physical/kept.rs`).
pub(super) fn placed_as_is(given: &DataType, column: &DataType) -> bool {
    matches!(
        (column, given),
        (DataType::Character { .. } | DataType::Varying { .. }, DataType::Character { .. } | DataType::Varying { .. })
            | (DataType::Exact { .. }, DataType::Exact { .. })
            | (DataType::Approximate { .. }, DataType::Approximate { .. })
            | (DataType::Binary { .. } | DataType::VarBinary { .. }, DataType::Binary { .. } | DataType::VarBinary { .. })
            | (DataType::Boolean, DataType::Boolean)
            | (DataType::Date, DataType::Date)
            | (DataType::Time { .. }, DataType::Time { .. })
            | (DataType::Timestamp { .. }, DataType::Timestamp { .. })
            | (DataType::Interval(_), DataType::Interval(_))
    )
}

/// [X3.135-1986 §5.11 GR4] The exact number `d` among approximate values:
/// the `f64` equal to it, or the two on either side of it, found from its
/// nearest by stepping.
fn approximate_place(d: &Decimal) -> Option<Placed> {
    let exact = Value::Exact(*d);
    let mut at = d.to_f64();
    for _ in 0..NEIGHBOUR_STEPS {
        let order = compare(&Value::Approx(at), &exact)?;
        let next = match order {
            Ordering::Equal => return Some(Placed::At(Value::Approx(at))),
            Ordering::Less => at.next_up(),
            Ordering::Greater => at.next_down(),
        };
        if compare(&Value::Approx(next), &exact)? == order.reverse() {
            let (below, above) = if order == Ordering::Less { (at, next) } else { (next, at) };
            return Some(Placed::Between(Value::Approx(below), Value::Approx(above)));
        }
        at = next;
    }
    None
}

/// [X3.135-1986 §5.11 GR4] The approximate number `f` among the values of
/// an exact column of `scale`, every one a multiple of 10^-scale: that
/// equal to it, or the two on either side of it, from it rounded to the
/// scale (Rust's formatting rounds exactly), one unit of the scale apart.
fn exact_place(f: f64, scale: u32) -> Option<Placed> {
    if !f.is_finite() {
        return None;
    }
    let written = format!("{f:.digits$}", digits = usize::try_from(scale).ok()?);
    let units: i128 = written.replace('.', "").parse().ok()?;
    let (rounded, unit) = (Decimal::new(units, scale), Decimal::new(1, scale));
    Some(match compare(&Value::Exact(rounded), &Value::Approx(f))? {
        Ordering::Equal => Placed::At(Value::Exact(rounded)),
        Ordering::Less => Placed::Between(Value::Exact(rounded), Value::Exact(rounded.checked_add(unit).ok()?)),
        Ordering::Greater => Placed::Between(Value::Exact(rounded.checked_sub(unit).ok()?), Value::Exact(rounded)),
    })
}

// Unit tests: ranges merged, numbers placed among a column's values, and
// probes written (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
