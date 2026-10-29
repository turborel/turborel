//! \[DESIGN\] The condition rows were chosen by through an index
//! (`TableStorage::read_by_condition`). Split from storage.rs (RULES.md
//! R-SIZE-2).

use std::cmp::Ordering;

use super::Row;
use crate::index::Probe;
use crate::value::{CompOp, Value, compare};

/// \[DESIGN\] A condition rows were chosen by through an index: their values
/// in `columns`, the index's, answer `probe` (`TableStorage::read_by_condition`).
#[derive(Debug, Clone)]
pub struct Condition {
    /// The index's columns.
    pub columns: Vec<usize>,
    /// What the index was asked.
    pub probe: Probe,
}

impl Condition {
    /// Whether `row` may be one the index chooses: every column of the key
    /// not null (`Index::insert`), and the key answering the probe
    /// (`Probe::answers`), so this is never narrower than what the index
    /// returns.
    pub fn matches(&self, row: &Row) -> bool {
        let key: Option<Vec<&Value>> = self.columns.iter().map(|&c| row.get(c)).collect();
        key.is_some_and(|k| self.matches_key(&k))
    }

    /// As `matches`, of a row whose values of the condition's columns, in
    /// their order, are `key`.
    pub fn matches_key(&self, key: &[&Value]) -> bool {
        !key.iter().any(|v| v.is_null()) && self.probe.answers(key.iter().copied())
    }

    /// Whether a row whose every column's value is within `bounds`, its
    /// least and greatest not null, may match: never narrower than whether
    /// one does, as each column is taken alone. A column with no bounds has
    /// no value not null, so no row matches.
    pub fn may_match(&self, bounds: &[Option<(Value, Value)>]) -> bool {
        let ranges: Option<Vec<&(Value, Value)>> = self.columns.iter().map(|&c| bounds.get(c).and_then(Option::as_ref)).collect();
        let Some(ranges) = ranges else { return false };
        match &self.probe {
            Probe::All => true,
            Probe::Compare { op, value } => ranges.first().is_some_and(|r| within(*op, r, value)),
            Probe::Prefix { equal, then } => {
                equal.iter().zip(&ranges).all(|(v, r)| within(CompOp::Eq, r, v))
                    && then.as_ref().is_none_or(|(op, v)| ranges.get(equal.len()).is_some_and(|r| within(*op, r, v)))
            }
            Probe::Range { equal, low, high } => {
                let next = ranges.get(equal.len());
                equal.iter().zip(&ranges).all(|(v, r)| within(CompOp::Eq, r, v))
                    && next.is_some_and(|r| within(low.0, r, &low.1) && within(high.0, r, &high.1))
            }
        }
    }
}

/// Whether a value from `least` to `greatest` may compare with `v` by `op`:
/// values that do not compare may.
fn within(op: CompOp, (least, greatest): &(Value, Value), v: &Value) -> bool {
    let not = |a: &Value, b: &Value, o: Ordering| compare(a, b) != Some(o);
    match op {
        CompOp::Eq => not(least, v, Ordering::Greater) && not(v, greatest, Ordering::Greater),
        CompOp::Ne => !(compare(least, v) == Some(Ordering::Equal) && compare(greatest, v) == Some(Ordering::Equal)),
        CompOp::Lt => not(least, v, Ordering::Greater) && not(least, v, Ordering::Equal),
        CompOp::Le => not(least, v, Ordering::Greater),
        CompOp::Gt => not(greatest, v, Ordering::Less) && not(greatest, v, Ordering::Equal),
        CompOp::Ge => not(greatest, v, Ordering::Less),
    }
}

// Unit tests: whether rows within bounds may match a range probe
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    fn n(v: i128) -> Value {
        Value::Exact(Decimal::new(v, 0))
    }

    /// Verifies: [DESIGN] RULES.md D29: rows changed within bounds may
    /// match a range of an index's columns only if the bounds meet it: of
    /// the range's column, and of the columns held equal before it.
    #[test]
    fn rows_within_bounds_may_match_a_range() {
        let range = |equal: Vec<Value>| Condition {
            columns: vec![0, 1],
            probe: Probe::Range { equal, low: Box::new((CompOp::Ge, n(10))), high: Box::new((CompOp::Lt, n(20))) },
        };
        let bounds = |a: (i128, i128), b: (i128, i128)| vec![Some((n(a.0), n(a.1))), Some((n(b.0), n(b.1)))];
        let alone = Condition {
            columns: vec![1],
            probe: Probe::Range { equal: Vec::new(), low: Box::new((CompOp::Ge, n(10))), high: Box::new((CompOp::Lt, n(20))) },
        };
        assert!(alone.may_match(&bounds((0, 0), (15, 30))), "overlapping");
        assert!(!alone.may_match(&bounds((0, 0), (20, 30))), "past the upper bound, exclusive");
        assert!(!alone.may_match(&bounds((0, 0), (0, 9))), "below the lower");
        assert!(range(vec![n(1)]).may_match(&bounds((0, 2), (0, 30))), "the equal column within");
        assert!(!range(vec![n(5)]).may_match(&bounds((0, 2), (0, 30))), "the equal column without");
        assert!(!alone.may_match(&[Some((n(0), n(0))), None]), "a column of no value not null");
    }
}
