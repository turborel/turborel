//! \[DESIGN\] The conditions rows were chosen by: through an index
//! (`TableStorage::read_by_condition`), or those of a filter a scan is
//! told (`TableStorage::scan_where`). Split from storage.rs (RULES.md
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

/// \[DESIGN\] A condition true of every row a filter keeps, that a scan
/// is told (`TableStorage::scan_where`).
#[derive(Debug, Clone)]
pub enum Conjunct {
    /// Of an index's kind: the values of its columns answer its probe.
    Condition(Condition),
    /// [X3.135-1986 §5.15] `column IS NULL`, or IS NOT NULL if `not`.
    Null {
        /// The column.
        column: usize,
        /// IS NOT NULL was written.
        not: bool,
    },
    /// [X3.135-1986 §5.13] `column IN (values)`, each a constant.
    In {
        /// The column.
        column: usize,
        /// The constants.
        values: Vec<Value>,
    },
}

impl Conjunct {
    /// The columns it reads, in the order `matches_key` takes their values.
    pub fn columns(&self) -> Vec<usize> {
        match self {
            Conjunct::Condition(c) => c.columns.clone(),
            Conjunct::Null { column, .. } | Conjunct::In { column, .. } => vec![*column],
        }
    }

    /// Whether `row` may be one it is true of.
    pub fn matches(&self, row: &Row) -> bool {
        let key: Option<Vec<&Value>> = self.columns().iter().map(|&c| row.get(c)).collect();
        key.is_some_and(|k| self.matches_key(&k))
    }

    /// As `matches`, of a row whose values of its columns, in their order,
    /// are `key`.
    pub fn matches_key(&self, key: &[&Value]) -> bool {
        match self {
            Conjunct::Condition(c) => c.matches_key(key),
            Conjunct::Null { not, .. } => key.first().is_some_and(|v| v.is_null() != *not),
            Conjunct::In { values, .. } => key.first().is_some_and(|v| values.iter().any(|c| compare(v, c) == Some(Ordering::Equal))),
        }
    }

    /// Whether a row whose every column's value is within `bounds`, its
    /// least and greatest not null, may be one it is true of: never
    /// narrower than whether one is. As the bounds say nothing of nulls,
    /// IS NULL may be true of any row.
    pub fn may_match(&self, bounds: &[Option<(Value, Value)>]) -> bool {
        match self {
            Conjunct::Condition(c) => c.may_match(bounds),
            Conjunct::Null { not: false, .. } => true,
            Conjunct::Null { column, not: true } => bounds.get(*column).is_some_and(Option::is_some),
            Conjunct::In { column, values } => {
                bounds.get(*column).and_then(Option::as_ref).is_some_and(|r| values.iter().any(|v| within(CompOp::Eq, r, v)))
            }
        }
    }
}

impl Conjunct {
    /// As `may_match`, of a conjunct of one column, whose values not null
    /// are within `range`, if any: no allocation for a comparison or a list.
    pub fn may_match_range(&self, range: Option<&(Value, Value)>) -> bool {
        match self {
            Conjunct::Condition(Condition { probe: Probe::Compare { op, value }, .. }) => range.is_some_and(|r| within(*op, r, value)),
            Conjunct::In { values, .. } => range.is_some_and(|r| values.iter().any(|v| within(CompOp::Eq, r, v))),
            Conjunct::Null { not, .. } => !*not || range.is_some(),
            Conjunct::Condition(c) => {
                let Some(&column) = c.columns.first() else { return true };
                let mut bounds = vec![None; column.saturating_add(1)];
                if let Some(slot) = bounds.get_mut(column) {
                    *slot = range.cloned();
                }
                c.may_match(&bounds)
            }
        }
    }
}

/// \[DESIGN\] Conditions rows were chosen by, each true of every row
/// chosen: those of a filter a scan is told (`TableStorage::scan_where`),
/// or an index's one (`TableStorage::read_by_condition`).
#[derive(Debug, Clone)]
pub struct Conjunction(pub Vec<Conjunct>);

impl Conjunction {
    /// The rows `condition` chose.
    pub fn of(condition: Condition) -> Conjunction {
        Conjunction(vec![Conjunct::Condition(condition)])
    }

    /// Its conjuncts.
    pub fn conjuncts(&self) -> &[Conjunct] {
        &self.0
    }

    /// The columns its conjuncts read, each once.
    pub fn columns(&self) -> Vec<usize> {
        let mut columns: Vec<usize> = self.0.iter().flat_map(Conjunct::columns).collect();
        columns.sort_unstable();
        columns.dedup();
        columns
    }

    /// Whether `row` may be one chosen: each conjunct matches it.
    pub fn matches(&self, row: &Row) -> bool {
        self.0.iter().all(|c| c.matches(row))
    }

    /// Whether a row whose every column's value is within `bounds` may be
    /// one chosen: each conjunct may match it.
    pub fn may_match(&self, bounds: &[Option<(Value, Value)>]) -> bool {
        self.0.iter().all(|c| c.may_match(bounds))
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

    /// Verifies: [DESIGN] RULES.md D29, D31 [X3.135-1986 §5.13, §5.15]: a
    /// conjunct of IS NULL, IS NOT NULL or IN matches a row as the predicate
    /// is true of it, IN never of a null; within bounds of no null, IS NULL
    /// may match any row, IS NOT NULL a row of a value, IN a row whose
    /// bounds hold one of its values; a conjunction matches as each does.
    #[test]
    fn conjuncts_match_as_their_predicates() {
        let null = Conjunct::Null { column: 0, not: false };
        let not_null = Conjunct::Null { column: 0, not: true };
        let listed = Conjunct::In { column: 1, values: vec![n(3), n(7)] };
        let row = vec![Value::Null, n(7)];
        assert!(null.matches(&row) && !not_null.matches(&row) && listed.matches(&row));
        assert!(!listed.matches(&vec![n(1), Value::Null]), "IN of a null");
        assert!(!listed.matches(&vec![n(1), n(5)]));
        let bounds = vec![None, Some((n(4), n(6)))];
        assert!(null.may_match(&bounds), "nulls not known");
        assert!(!not_null.may_match(&bounds), "no value in column 0");
        assert!(!listed.may_match(&bounds), "neither 3 nor 7 within 4 to 6");
        assert!(listed.may_match(&[None, Some((n(6), n(9)))]));
        // Of one column's range alone, as a zone map tests it.
        let range = (n(4), n(6));
        let below = Conjunct::Condition(Condition { columns: vec![1], probe: Probe::Compare { op: CompOp::Lt, value: n(5) } });
        assert!(below.may_match_range(Some(&range)) && !below.may_match_range(None), "no value: none below");
        assert!(!listed.may_match_range(Some(&range)) && listed.may_match_range(Some(&(n(7), n(9)))));
        assert!(null.may_match_range(None) && !not_null.may_match_range(None));
        let ranged = Conjunct::Condition(Condition {
            columns: vec![1],
            probe: Probe::Range { equal: Vec::new(), low: Box::new((CompOp::Ge, n(5))), high: Box::new((CompOp::Le, n(9))) },
        });
        assert!(ranged.may_match_range(Some(&range)) && !ranged.may_match_range(Some(&(n(10), n(12)))));
        let both = Conjunction(vec![null, listed]);
        assert_eq!(both.columns(), vec![0, 1]);
        assert!(both.matches(&row) && !both.may_match(&bounds));
    }
}
