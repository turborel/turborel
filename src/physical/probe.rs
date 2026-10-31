//! \[DESIGN\] The probes an index can answer for the conjuncts of a filter
//! over its table (RULES.md D34): comparisons of a column with constants,
//! the bounds of a LIKE's prefix, and an OR of these of one column, an IN
//! list among them, as one probe of each of its ranges. Split from
//! physical/access.rs (RULES.md R-SIZE-2).

use std::cmp::Ordering;

use super::estimate::{self, Known};
use crate::index::Probe;
use crate::logical::{Cond, Expr, and_all, compared_with_constant, disjuncts, like_bounds, render_cond, split_and};
use crate::value::{CompOp, DataType, Decimal, Value, compare};

/// A comparison of a column with a constant an index may answer: of the
/// conjunct `at`, answering it `exact`ly, or else only bounding the rows it
/// keeps, as a LIKE's prefix does, so that a filter still judges it.
pub(super) struct Compared {
    pub(super) at: usize,
    pub(super) exact: bool,
    pub(super) column: usize,
    pub(super) op: CompOp,
    pub(super) value: Value,
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

/// The comparisons and ORs of them among `conds`.
pub(super) fn comparisons(conds: &[Cond], known: Known<'_>) -> (Vec<Compared>, Vec<Alternatives>) {
    let (mut compared, mut ors) = (Vec::new(), Vec::new());
    for (at, c) in conds.iter().enumerate() {
        if let Some((column, exact, each)) = bounds(c) {
            compared.extend(each.into_iter().map(|(op, value)| Compared { at, exact, column, op, value }));
        } else if let Some((column, exact, each)) = alternatives(c) {
            ors.push(Alternatives { at, column, exact, each, share: estimate::selectivity(c, known) });
        }
    }
    (compared, ors)
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
        | (DataType::Bit { .. } | DataType::BitVarying { .. }, Value::Bits(_))
        | (DataType::Date | DataType::Time { .. } | DataType::Timestamp { .. }, Value::Datetime(_))
        | (DataType::Interval(_), Value::Interval(_)) => Placed::At(value.clone()),
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

/// `probe`, of an index on `columns`, as a condition over the table's
/// columns, written with their `names`.
pub(super) fn render_probe(probe: &Probe, columns: &[usize], names: &[String]) -> String {
    let compared = |at: usize, op: CompOp, value: &Value| {
        Cond::Compare(Expr::Column(columns.get(at).copied().unwrap_or_default()), op, Expr::Const(value.clone()))
    };
    let equalities = |equal: &[Value]| equal.iter().enumerate().map(|(at, v)| compared(at, CompOp::Eq, v)).collect::<Vec<_>>();
    let conds = match probe {
        Probe::All => Vec::new(),
        Probe::Compare { op, value } => vec![compared(0, *op, value)],
        Probe::Prefix { equal, then } => {
            let next = then.as_ref().map(|(op, value)| compared(equal.len(), *op, value));
            equalities(equal).into_iter().chain(next).collect()
        }
        Probe::Range { equal, low, high } => {
            let next = [compared(equal.len(), low.0, &low.1), compared(equal.len(), high.0, &high.1)];
            equalities(equal).into_iter().chain(next).collect()
        }
    };
    and_all(conds).map_or_else(|| "all keys, in order".to_string(), |c| render_cond(&c, names))
}

// Unit tests: ranges merged, numbers placed among a column's values, and
// probes written (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Spelled;

    fn int(n: i128) -> Value {
        Value::Exact(Decimal::new(n, 0))
    }

    fn exact(scale: u32) -> DataType {
        DataType::Exact { precision: 9, scale, spelled: Spelled::Numeric }
    }

    fn approximate() -> DataType {
        DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision }
    }

    /// The probes of the values `equal` then an OR of `each`, over INTEGER columns.
    fn any_of(equal: &[Value], each: Vec<Vec<(CompOp, Value)>>) -> Option<Vec<Probe>> {
        probes_of_any(equal, &exact(0), &Alternatives { at: 0, column: 0, exact: true, each, share: 1.0_f64 })
    }

    /// How many of `probes` find `n`, as `Probe::answers` says.
    fn finding(probes: &[Probe], n: i128) -> usize {
        probes.iter().filter(|p| p.answers([&int(n)])).count()
    }

    /// The one comparison of `each`.
    fn only(each: &[(CompOp, Value)]) -> (CompOp, Value) {
        let [one] = each else { panic!("{each:?}") };
        one.clone()
    }

    /// Whether `c` is `op` and a value equal to `v`.
    fn same(c: &(CompOp, Value), op: CompOp, v: &Value) -> bool {
        c.0 == op && compare(&c.1, v) == Some(Ordering::Equal)
    }

    /// Verifies: [DESIGN] RULES.md D34: an OR of ranges of one column, an IN
    /// list among them, is probed a range at a time, the ranges that overlap
    /// or touch joined, so each value is found by one probe at most and
    /// exactly by those its operands are true of; equal values once; one
    /// range unbounded both ways, or a comparison `<>`, is no probe.
    #[test]
    fn ranges_of_an_or_are_merged() {
        let eq = |n: i128| vec![(CompOp::Eq, int(n))];
        let probes =
            any_of(&[], vec![eq(3), eq(7), eq(3), vec![(CompOp::Lt, int(2))], vec![(CompOp::Ge, int(7)), (CompOp::Le, int(9))]]).unwrap();
        assert_eq!(probes.len(), 3, "{probes:?}");
        for n in -1..12_i128 {
            let expected = usize::from(n < 2 || n == 3 || (7..=9).contains(&n));
            assert_eq!(finding(&probes, n), expected, "{n}");
        }
        // Touching at a value one of them keeps: one range; at one neither keeps: two.
        let touching = any_of(&[], vec![vec![(CompOp::Lt, int(5))], vec![(CompOp::Ge, int(5)), (CompOp::Lt, int(8))]]).unwrap();
        assert_eq!(touching.len(), 1, "{touching:?}");
        assert_eq!((finding(&touching, 5), finding(&touching, 8)), (1, 0));
        let apart = any_of(&[], vec![vec![(CompOp::Lt, int(5))], vec![(CompOp::Gt, int(5))]]).unwrap();
        assert_eq!(apart.len(), 2, "{apart:?}");
        assert_eq!((finding(&apart, 4), finding(&apart, 5), finding(&apart, 6)), (1, 0, 1));
        let nested = any_of(&[], vec![vec![(CompOp::Ge, int(1)), (CompOp::Le, int(9))], vec![(CompOp::Gt, int(2)), (CompOp::Lt, int(4))]]);
        assert_eq!(nested.map(|p| p.len()), Some(1));
        let same = any_of(&[], vec![vec![(CompOp::Gt, int(1)), (CompOp::Ge, int(1)), (CompOp::Le, int(4)), (CompOp::Lt, int(4))]]).unwrap();
        assert_eq!((finding(&same, 1), finding(&same, 2), finding(&same, 4)), (0, 1, 0), "the stricter of two bounds");
        assert!(any_of(&[], vec![vec![(CompOp::Lt, int(5))], vec![(CompOp::Gt, int(3))]]).is_none(), "every value");
        assert!(any_of(&[], vec![eq(1), vec![(CompOp::Ne, int(3))]]).is_none(), "<>");
        assert!(any_of(&[], vec![vec![(CompOp::Gt, int(5)), (CompOp::Lt, int(5))]]).is_none(), "no value");
        // Of an index's next column, after the values of its first.
        let prefixed =
            any_of(&[int(1)], vec![eq(2), vec![(CompOp::Gt, int(5))], vec![(CompOp::Gt, int(-9)), (CompOp::Lt, int(-5))]]).unwrap();
        let [Probe::Range { .. }, Probe::Prefix { then: None, .. }, Probe::Prefix { then: Some(_), .. }] = prefixed.as_slice() else {
            panic!("{prefixed:?}")
        };
    }

    /// Verifies: [X3.135-1986 §5.11 GR4] a number of the other kind than its
    /// column's is placed among the column's values: one of them if it
    /// equals one, else between the two on either side, the operator turned
    /// so the comparisons keep the same values; `<>` of one between none.
    #[test]
    fn numbers_are_placed_among_a_columns_values() {
        // 0.1 is between two f64s; 0.5 is one.
        let tenth = Value::Exact(Decimal::new(1, 1));
        let [eq, lt, gt] = [CompOp::Eq, CompOp::Lt, CompOp::Gt].map(|op| coerce(op, &tenth, &approximate()).unwrap());
        let [(CompOp::Gt, Value::Approx(below)), (CompOp::Lt, Value::Approx(above))] = eq.as_slice() else { panic!("{eq:?}") };
        assert_eq!(below.next_up(), *above);
        assert!(same(&only(&lt), CompOp::Le, &Value::Approx(*below)));
        assert!(same(&only(&gt), CompOp::Ge, &Value::Approx(*above)));
        assert!(coerce(CompOp::Ne, &tenth, &approximate()).is_none());
        let negative = coerce(CompOp::Eq, &Value::Exact(Decimal::new(-3, 1)), &approximate()).unwrap();
        assert_eq!(negative.len(), 2, "{negative:?}");
        let half = only(&coerce(CompOp::Eq, &Value::Exact(Decimal::new(5, 1)), &approximate()).unwrap());
        assert!(same(&half, CompOp::Eq, &Value::Approx(0.5_f64)));
        // Of a column of scale 2: 3.25 is one of its values, 3.3 and -0.001 are not.
        let at = only(&coerce(CompOp::Eq, &Value::Approx(3.25_f64), &exact(2)).unwrap());
        assert_eq!((at.0, compare(&at.1, &Value::Exact(Decimal::new(325, 2)))), (CompOp::Eq, Some(Ordering::Equal)));
        let up_to = only(&coerce(CompOp::Le, &Value::Approx(3.3_f64), &exact(2)).unwrap());
        let past = only(&coerce(CompOp::Gt, &Value::Approx(3.3_f64), &exact(2)).unwrap());
        assert_eq!((up_to.0, past.0), (CompOp::Le, CompOp::Ge));
        assert_eq!(compare(&up_to.1, &Value::Exact(Decimal::new(329, 2))), Some(Ordering::Equal), "{up_to:?}");
        assert_eq!(compare(&past.1, &Value::Exact(Decimal::new(330, 2))), Some(Ordering::Equal), "{past:?}");
        let small = only(&coerce(CompOp::Gt, &Value::Approx(-0.001_f64), &exact(2)).unwrap());
        assert_eq!(compare(&small.1, &int(0)), Some(Ordering::Equal), "{small:?}");
        assert!(coerce(CompOp::Eq, &Value::Approx(f64::INFINITY), &exact(2)).is_none());
        assert!(coerce(CompOp::Eq, &Value::Approx(1e300_f64), &exact(2)).is_none(), "past every exact value");
        assert!(coerce(CompOp::Eq, &Value::Char("A".into()), &exact(2)).is_none());
    }

    /// Verifies: [DESIGN] a probe is shown as the comparisons it makes of
    /// the index's columns.
    #[test]
    fn probes_are_written_as_comparisons() {
        let names = ["A".to_string(), "B".to_string()];
        let range = Probe::Range { equal: vec![int(1)], low: Box::new((CompOp::Gt, int(2))), high: Box::new((CompOp::Le, int(4))) };
        assert_eq!(render_probe(&range, &[1, 0], &names), "B = 1 AND A > 2 AND A <= 4");
        assert_eq!(render_probe(&Probe::Prefix { equal: vec![int(1)], then: None }, &[1, 0], &names), "B = 1");
        assert_eq!(render_probe(&Probe::All, &[0], &names), "all keys, in order");
    }
}
