//! \[DESIGN\] The conditions rows were chosen by: through an index
//! (`TableStorage::read_by_condition`), or those of a filter a scan is
//! told (`TableStorage::scan_where`).

use std::cmp::Ordering;
use std::sync::Arc;

use super::Row;
use crate::index::Probe;
use crate::value::{CompOp, Value, compare};

mod liked;
mod members;
mod threshold;
pub use liked::Liked;
pub use members::{Held, Members};
pub use threshold::Threshold;

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
    /// returns. For a function or an XML lookup, every key is a candidate,
    /// including nulls. Storage never calls the function because it may
    /// hold a lock here (D34, R-ERR-4).
    pub fn matches(&self, row: &Row) -> bool {
        let key: Option<Vec<&Value>> = self.columns.iter().map(|&c| row.get(c)).collect();
        key.is_some_and(|k| self.matches_key(&k))
    }

    /// As `matches`, of a row whose values of the condition's columns, in
    /// their order, are `key`.
    pub fn matches_key(&self, key: &[&Value]) -> bool {
        if self.probe.opaque() {
            return true;
        }
        !key.iter().any(|v| v.is_null()) && self.probe.answers(key.iter().copied())
    }

    /// Whether a row whose every column's value is within `bounds`, its
    /// least and greatest not null, may match: never narrower than whether
    /// one does, as each column is taken alone. A column with no bounds has
    /// no value not null, so no comparison matches. For a function or an XML
    /// lookup (`Probe::opaque`),
    /// returns true: these bounds cannot tell which rows the function selects.
    pub fn may_match(&self, bounds: &[Option<(Value, Value)>]) -> bool {
        if self.probe.opaque() {
            return true;
        }
        let ranges: Option<Vec<&(Value, Value)>> = self.columns.iter().map(|&c| bounds.get(c).and_then(Option::as_ref)).collect();
        let Some(ranges) = ranges else { return false };
        match &self.probe {
            Probe::All | Probe::Predicate { .. } | Probe::Xml(_) => true,
            Probe::Columns(each) => each.iter().all(|c| ranges.get(c.column).is_some_and(|r| within(c.op, r, &c.value))),
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
    /// [X3.135-1986 §5.13] `column NOT IN (values)`, each a constant not
    /// null.
    NotIn {
        /// The column.
        column: usize,
        /// The constants.
        values: Vec<Value>,
    },
    /// [X3.135-1986 §5.18] An OR of conjuncts of one column: any of them.
    Any(Vec<Conjunct>),
    /// \[DESIGN\] (RULES.md D379) The bound of a sort's first key, `column`,
    /// as it holds rows. A row past it is not among the first. It is true
    /// of every row the sort would keep, and the sort judges each row it
    /// is given, so a scan may pass over a group or a part past it, and may
    /// leave out a row past it or of a null key (RULES.md D1241), or give
    /// either.
    Dynamic {
        /// The column.
        column: usize,
        /// The bound, raised as the sort holds rows.
        threshold: Arc<Threshold>,
    },
    /// \[DESIGN\] (RULES.md D600) The spans of a column's values a hash
    /// join's runtime filter holds its keys in, a span of one value for
    /// each of a few keys. True of every row, as the join's filter judges
    /// them, it passes over groups and parts none of whose values lies in
    /// a span.
    Spans {
        /// The column.
        column: usize,
        /// The spans, least and greatest, ascending and apart.
        spans: Arc<[(Value, Value)]>,
    },
    /// \[DESIGN\] (RULES.md D1294) The keys a hash join's runtime filter
    /// holds, of `column`. True of every row the join keeps, as no built
    /// row matches a row whose key it does not hold, and the join's own
    /// filter and probe judge each row given. So a scan may leave out a
    /// row whose key it does not hold, or give it.
    Among {
        /// The column.
        column: usize,
        /// The keys.
        members: Arc<Members>,
    },
    /// [ISO/IEC 9075-2:2023 §4.3.2, §8.2 GR3b] (RULES.md D1183) `column
    /// <op> constant` of a character string under a collation of NO PAD
    /// that orders strings as their octets do: the octets compared, a
    /// prefix the less, no space padding either.
    Unpadded {
        /// The column.
        column: usize,
        /// The comparison operator.
        op: CompOp,
        /// The constant's octets.
        octets: Vec<u8>,
    },
    /// [X3.135-1986 §5.14] (RULES.md D1251) `column LIKE pattern` or
    /// `column NOT LIKE pattern`, of a constant pattern of no `_`. A
    /// column store's scan judges it of a VARCHAR column's texts in
    /// place, those coded by a table of symbols by the pattern's
    /// machine, and where it does not, gives every row, which the
    /// filter judges as it judges every row given.
    Like {
        /// The column.
        column: usize,
        /// The pattern, whether NOT was written, and its machine.
        like: Arc<Liked>,
    },
}

impl Conjunct {
    /// The columns it reads, in the order `matches_key` takes their values.
    pub fn columns(&self) -> Vec<usize> {
        match self {
            Conjunct::Condition(c) => c.columns.clone(),
            Conjunct::Null { column, .. }
            | Conjunct::In { column, .. }
            | Conjunct::NotIn { column, .. }
            | Conjunct::Dynamic { column, .. }
            | Conjunct::Spans { column, .. }
            | Conjunct::Among { column, .. }
            | Conjunct::Unpadded { column, .. }
            | Conjunct::Like { column, .. } => vec![*column],
            Conjunct::Any(each) => {
                let mut columns: Vec<usize> = each.iter().flat_map(Conjunct::columns).collect();
                columns.sort_unstable();
                columns.dedup();
                columns
            }
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
            Conjunct::NotIn { values, .. } => {
                key.first().is_some_and(|v| values.iter().all(|c| compare(v, c).is_some_and(Ordering::is_ne)))
            }
            // Each of one column, the same.
            Conjunct::Any(each) => each.iter().any(|c| c.matches_key(key)),
            Conjunct::Dynamic { .. } | Conjunct::Spans { .. } | Conjunct::Among { .. } => true,
            Conjunct::Unpadded { op, octets, .. } => key
                .first()
                .and_then(|v| if let Value::Char(s) = v { Some(s.as_bytes().cmp(octets)) } else { None })
                .is_some_and(|o| op.holds(o)),
            // Unknown of a null [X3.135-1986 §5.14 GR4]. A value not a
            // string held, as a large object's, is the filter's to read.
            Conjunct::Like { like, .. } => {
                key.first().is_some_and(|v| if let Value::Char(s) = v { like.keeps(s.as_bytes()) } else { !v.is_null() })
            }
        }
    }

    /// Whether a row whose every column's value is within `bounds`, its
    /// least and greatest not null, may be one it is true of: never
    /// narrower than whether one is. As the bounds say nothing of nulls,
    /// IS NULL may be true of any row.
    pub fn may_match(&self, bounds: &[Option<(Value, Value)>]) -> bool {
        match self {
            Conjunct::Condition(c) => c.may_match(bounds),
            // A LIKE's prefix's bounds are told beside it (`logical/pushed.rs`),
            // and a filter's range and spans beside its keys (RULES.md D1294).
            Conjunct::Null { not: false, .. } | Conjunct::Like { .. } | Conjunct::Among { .. } => true,
            Conjunct::Null { column, not: true } => bounds.get(*column).is_some_and(Option::is_some),
            Conjunct::In { column, values } => {
                bounds.get(*column).and_then(Option::as_ref).is_some_and(|r| values.iter().any(|v| within(CompOp::Eq, r, v)))
            }
            Conjunct::NotIn { column, values } => bounds.get(*column).and_then(Option::as_ref).is_some_and(|r| !one_of(r, values)),
            Conjunct::Any(each) => each.iter().any(|c| c.may_match(bounds)),
            Conjunct::Dynamic { column, threshold } => threshold.may_match(bounds.get(*column).and_then(Option::as_ref)),
            Conjunct::Spans { column, spans } => meets(spans, bounds.get(*column).and_then(Option::as_ref)),
            Conjunct::Unpadded { column, op, octets } => unpadded_within(*op, bounds.get(*column).and_then(Option::as_ref), octets),
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
            Conjunct::NotIn { values, .. } => range.is_some_and(|r| !one_of(r, values)),
            Conjunct::Any(each) => each.iter().any(|c| c.may_match_range(range)),
            Conjunct::Dynamic { threshold, .. } => threshold.may_match(range),
            Conjunct::Spans { spans, .. } => meets(spans, range),
            Conjunct::Unpadded { op, octets, .. } => unpadded_within(*op, range, octets),
            Conjunct::Like { .. } | Conjunct::Among { .. } => true,
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

/// Whether every value from `least` to `greatest` is one of `values`: so
/// if they are one value, which is.
fn one_of((least, greatest): &(Value, Value), values: &[Value]) -> bool {
    compare(least, greatest) == Some(Ordering::Equal) && values.iter().any(|v| compare(least, v) == Some(Ordering::Equal))
}

/// Whether a character string within `range`, its least and greatest as
/// strings compare padded, may hold to `octets` by `op` compared unpadded.
/// Equal octets are equal padded too, so an equality passes over a range
/// the constant is outside; a string apart from the constant, below or
/// above it unpadded may be equal, above or below it padded, as `'a'` is
/// to `'a '` and to `'a'` and a tab, so no other comparison passes over
/// one. A range of no value holds none.
fn unpadded_within(op: CompOp, range: Option<&(Value, Value)>, octets: &[u8]) -> bool {
    let Some(range) = range else { return false };
    match op {
        CompOp::Eq => std::str::from_utf8(octets).is_ok_and(|text| within(CompOp::Eq, range, &Value::Char(text.into()))),
        CompOp::Ne | CompOp::Lt | CompOp::Le | CompOp::Gt | CompOp::Ge => true,
    }
}

/// Whether a value within `range`, if any, may lie in one of `spans`,
/// ascending and apart: the first span not wholly below the range starts
/// within it, found by halving. Of values that do not compare, it may.
fn meets(spans: &[(Value, Value)], range: Option<&(Value, Value)>) -> bool {
    let Some((least, greatest)) = range else { return false };
    let first = spans.partition_point(|(_, end)| compare(end, least) == Some(Ordering::Less));
    spans.get(first).is_some_and(|(start, _)| compare(start, greatest) != Some(Ordering::Greater))
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

    /// Verifies: [DESIGN] Serializable change summaries intersect every
    /// independent key constraint. Either axis alone can exclude a change;
    /// closed boundaries include it. Missing columns cannot match.
    #[test]
    fn independent_conditions_retain_each_coordinate_boundary() {
        use crate::index::ColumnComparison;
        let condition = Condition {
            columns: vec![1, 0],
            probe: Probe::Columns(vec![
                ColumnComparison { column: 0, op: CompOp::Ge, value: n(10) },
                ColumnComparison { column: 1, op: CompOp::Le, value: n(20) },
            ]),
        };
        let ranges = |x, y| vec![Some((n(x), n(x))), Some((n(y), n(y)))];
        assert!(condition.may_match(&ranges(20, 10)));
        assert!(!condition.may_match(&ranges(21, 10)));
        assert!(!condition.may_match(&ranges(20, 9)));
        assert!(!condition.may_match(&[Some((n(20), n(20)))]));
        let missing =
            Condition { columns: vec![0], probe: Probe::Columns(vec![ColumnComparison { column: 1, op: CompOp::Eq, value: n(1) }]) };
        assert!(!missing.may_match(&ranges(1, 1)));
    }

    /// Verifies: [DESIGN] RULES.md D600 (MC/DC): spans pass over bounds
    /// that lie in their gaps, below them or above them, and not over
    /// bounds that meet a span or cover a gap; no row of no bounds; every
    /// row as it judges rows.
    #[test]
    fn spans_pass_over_their_gaps() {
        let spans: Arc<[(Value, Value)]> = vec![(n(10), n(20)), (n(30), n(30)), (n(50), n(60))].into();
        let told = Conjunct::Spans { column: 0, spans };
        let may = |a: i128, b: i128| told.may_match(&[Some((n(a), n(b)))]);
        assert!(!may(21, 29) && !may(31, 49) && !may(0, 9) && !may(61, 99), "in a gap, below, above");
        assert!(may(15, 16) && may(25, 30) && may(29, 31) && may(0, 99) && may(60, 70), "meeting a span");
        assert!(!told.may_match(&[None]) && !told.may_match_range(None), "no value not null");
        assert!(told.may_match_range(Some(&(n(55), n(56)))) && !told.may_match_range(Some(&(n(40), n(41)))));
        assert!(told.matches(&vec![n(25)]), "a row is judged by the join's filter");
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2] (RULES.md D1183) a
    /// conjunct compared unpadded matches a character string by its octets,
    /// a prefix the less, never a null nor another value; and, a zone's
    /// bounds being in the padded order, only an equality passes over a
    /// range the constant is outside: of a range of `'a'` alone, below a
    /// tab after `'a'`, apart from `'a '` and equal to `'a '` may all match
    /// (regression: each would be passed over by the padded order), and a
    /// range of no value holds none.
    #[test]
    fn an_unpadded_conjunct_matches_octets_and_passes_over_by_equality_alone() {
        let told = |op: CompOp, text: &str| Conjunct::Unpadded { column: 0, op, octets: text.as_bytes().to_vec() };
        let text = |s: &str| Value::Char(s.into());
        assert!(told(CompOp::Eq, "a").matches_key(&[&text("a")]) && !told(CompOp::Eq, "a").matches_key(&[&text("a ")]));
        assert!(told(CompOp::Lt, "a ").matches_key(&[&text("a")]) && told(CompOp::Lt, "a ").matches_key(&[&text("a\t")]));
        assert!(told(CompOp::Gt, "a").matches_key(&[&text("a\t")]) && !told(CompOp::Ge, "a ").matches_key(&[&text("a")]));
        assert!(!told(CompOp::Ne, "a").matches_key(&[&Value::Null]) && !told(CompOp::Ne, "a").matches_key(&[&n(1)]));
        assert_eq!(told(CompOp::Le, "b").columns(), [0]);
        let all_a = Some((text("a"), text("a")));
        for (op, constant) in
            [(CompOp::Lt, "a\t"), (CompOp::Le, "a\t"), (CompOp::Ne, "a "), (CompOp::Gt, ""), (CompOp::Ge, "a"), (CompOp::Eq, "a ")]
        {
            assert!(told(op, constant).may_match(std::slice::from_ref(&all_a)), "{op:?} {constant:?}");
            assert!(told(op, constant).may_match_range(all_a.as_ref()), "{op:?} {constant:?}, a range");
            assert!(!told(op, constant).may_match(&[None]), "{op:?} {constant:?}, no value");
        }
        assert!(!told(CompOp::Eq, "b").may_match(std::slice::from_ref(&all_a)) && !told(CompOp::Eq, "b").may_match_range(all_a.as_ref()));
        assert!(!Conjunct::Unpadded { column: 0, op: CompOp::Eq, octets: vec![0xFF] }.may_match(std::slice::from_ref(&all_a)));
    }
}
