//! \[DESIGN\] IN lists judged by a set (RULES.md D31). An `<in predicate>`
//! of constants is bound as an OR of equalities (`binder/predicates.rs`),
//! which the planner reads as such: a probe of each value of an index, a
//! scan told the list. The condition a filter then judges row by row, a
//! long such list among it, is judged by a set of its values' buckets
//! (`value::bucket`), made once as the plan is made: a row's value is
//! looked for in its bucket, not compared with every value. A row's
//! judgment is that of the OR [X3.135-1986 §5.13 GR2, §5.18 GR2]: true if
//! the value equals one, as SQL compares them, a string padded with
//! spaces [§5.11 GR5], numbers by algebraic value [GR4]; else unknown if
//! it or one of the list is null [§5.11 GR2]; else false. Split from
//! logical.rs (RULES.md R-SIZE-2).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{Cond, Expr, Truth, in_list};
use crate::value::{Bucket, Value, bucket, compare};

/// \[DESIGN\] The fewest constants of an IN list judged by a set: of a
/// shorter one, comparing the value with each costs no more than finding
/// its bucket (`bench/joins.rs` measures lists of 4 to 64), and the
/// planner and EXPLAIN keep the OR the binder wrote.
pub const IN_SET_LEAST: usize = 8;

/// \[DESIGN\] The constants of an IN list, as a set if there are
/// `IN_SET_LEAST` of them or more.
#[derive(Debug)]
pub struct InSet {
    /// The constants, as written.
    values: Vec<Value>,
    /// Whether one is null.
    nulls: bool,
    /// Of a set, the places in `values` of each bucket's constants.
    buckets: Option<HashMap<Bucket, Vec<usize>>>,
    /// Of a set of character strings alone, each without its trailing
    /// spaces, as two strings are equal if they are equal so.
    texts: Option<HashSet<String>>,
}

impl InSet {
    /// The set of `values`.
    pub fn of(values: Vec<Value>) -> InSet {
        let nulls = values.iter().any(Value::is_null);
        let long = values.len() >= IN_SET_LEAST;
        let buckets = long.then(|| {
            let mut buckets: HashMap<Bucket, Vec<usize>> = HashMap::new();
            for (at, v) in values.iter().enumerate() {
                if let Some(b) = bucket(v) {
                    buckets.entry(b).or_default().push(at);
                }
            }
            buckets
        });
        let strings = |v: &Value| match v {
            Value::Char(s) => Some(Some(s.trim_end_matches(' ').to_owned())),
            Value::Null => Some(None),
            Value::Exact(_) | Value::Approx(_) | Value::Bits(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_) => None,
        };
        let texts =
            long.then(|| values.iter().map(strings).collect::<Option<Vec<_>>>()).flatten().map(|t| t.into_iter().flatten().collect());
        InSet { values, nulls, buckets, texts }
    }

    /// The constants, as written.
    pub fn values(&self) -> &[Value] {
        &self.values
    }

    /// Whether it is judged as a set, not one constant after another.
    pub fn hashed(&self) -> bool {
        self.buckets.is_some()
    }

    /// [X3.135-1986 §5.13 GR2, §5.18 GR2] `v` IN the constants: true if it
    /// equals one, else unknown if it or one of them is null, else false.
    pub fn truth(&self, v: &Value) -> Truth {
        let equal = |at: &usize| self.values.get(*at).is_some_and(|c| compare(v, c) == Some(Ordering::Equal));
        if v.is_null() {
            return Truth::Unknown;
        }
        let found = match &self.buckets {
            Some(buckets) => bucket(v).and_then(|b| buckets.get(&b)).is_some_and(|places| places.iter().any(equal)),
            None => (0..self.values.len()).any(|at| equal(&at)),
        };
        self.decided(found)
    }

    /// Whether it is a set of character strings alone, nulls aside.
    pub fn of_strings(&self) -> bool {
        self.texts.is_some()
    }

    /// [X3.135-1986 §5.13 GR2, §5.11 GR5] The string `s` IN the constants,
    /// if they are a set of strings: looked for without its trailing
    /// spaces.
    pub fn text_truth(&self, s: &str) -> Option<Truth> {
        let texts = self.texts.as_ref()?;
        Some(self.decided(texts.contains(s.trim_end_matches(' '))))
    }

    /// The truth of IN of a value not null, `found` among the constants or
    /// not.
    fn decided(&self, found: bool) -> Truth {
        if found {
            Truth::True
        } else if self.nulls {
            Truth::Unknown
        } else {
            Truth::False
        }
    }
}

/// `cond` with each OR of `IN_SET_LEAST` or more equalities of one column
/// with constants, as the binder writes an `<in predicate>` of constants,
/// made a judgment by their set (`Cond::In`); through AND, OR, NOT and IS,
/// not into expressions. It means the same, and raises nothing: a column
/// and constants cannot.
pub fn judged_by_sets(cond: Cond) -> Cond {
    if let Cond::Or(..) = cond
        && let Some((column, values)) = in_list(&cond).filter(|(_, values)| values.len() >= IN_SET_LEAST)
    {
        return Cond::In(Expr::Column(column), Arc::new(InSet::of(values)));
    }
    match cond {
        Cond::Or(a, b) => Cond::Or(Box::new(judged_by_sets(*a)), Box::new(judged_by_sets(*b))),
        Cond::And(a, b) => Cond::And(Box::new(judged_by_sets(*a)), Box::new(judged_by_sets(*b))),
        Cond::Not(a) => Cond::Not(Box::new(judged_by_sets(*a))),
        Cond::Is(a, truth, not) => Cond::Is(Box::new(judged_by_sets(*a)), truth, not),
        other @ (Cond::Const(_)
        | Cond::Compare(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Quantified(..)
        | Cond::Unique(_)
        | Cond::Match { .. }
        | Cond::In(..)) => other,
    }
}

// Unit tests: what a set finds, and which ORs become one (RULES.md
// R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::logical::{or_all, render_cond};
    use crate::value::{CompOp, Decimal};

    fn int(n: i128) -> Value {
        Value::Exact(Decimal::new(n, 0))
    }

    /// The OR of equalities of column 0 with `values`, as the binder writes IN.
    fn in_list_of(values: &[Value]) -> Cond {
        or_all(values.iter().map(|v| Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Const(v.clone()))).collect()).unwrap()
    }

    /// Verifies: [X3.135-1986 §5.13 GR2, §5.18 GR2, §5.11 GR2, GR4, GR5]:
    /// a value IN a set is true if it equals one of its constants, an exact
    /// number an approximate one of the same algebraic value, a string one
    /// padded; else unknown if it or one of them is null; else false; as a
    /// short list, judged one constant after another, judges it.
    #[test]
    fn a_set_judges_as_the_or_of_its_equalities() {
        let long: Vec<Value> = (0..20_i128).map(int).chain([Value::Approx(0.5_f64)]).collect();
        let mut with_null = long.clone();
        with_null.push(Value::Null);
        for (values, hashed) in [(long.clone(), true), (vec![int(3), Value::Approx(0.5_f64)], false)] {
            let set = InSet::of(values);
            assert_eq!(set.hashed(), hashed);
            assert_eq!(set.truth(&int(3)), Truth::True);
            assert_eq!(set.truth(&Value::Approx(3.0_f64)), Truth::True, "3E0 is 3");
            assert_eq!(set.truth(&Value::Exact(Decimal::new(5, 1))), Truth::True, "0.5 is 0.5E0");
            assert_eq!(set.truth(&Value::Exact(Decimal::new(31, 1))), Truth::False);
            assert_eq!(set.truth(&Value::Null), Truth::Unknown);
            assert!(set.text_truth("3").is_none() && !set.of_strings(), "not strings");
        }
        let nulled = InSet::of(with_null);
        assert_eq!((nulled.truth(&int(4)), nulled.truth(&int(40))), (Truth::True, Truth::Unknown));
    }

    /// Verifies: [X3.135-1986 §5.13 GR2, §5.11 GR5]: a string IN a set of
    /// strings is looked for without its trailing spaces, as strings
    /// compare padded; a set holding a number is no set of strings.
    #[test]
    fn a_set_of_strings_finds_them_padded() {
        let texts: Vec<Value> = (0_i32..10_i32).map(|n| Value::Char(format!("t{n}  "))).chain([Value::Null]).collect();
        let set = InSet::of(texts);
        assert!(set.of_strings());
        assert_eq!(set.text_truth("t3"), Some(Truth::True));
        assert_eq!(set.text_truth("t3 "), Some(Truth::True));
        assert_eq!(set.text_truth("t3\t"), Some(Truth::Unknown), "not equal, and a null among them");
        assert_eq!(set.truth(&Value::Char("t9".into())), Truth::True);
        let mixed: Vec<Value> = (0_i32..10_i32).map(|n| Value::Char(format!("t{n}"))).chain([int(1)]).collect();
        assert_eq!(InSet::of(mixed).text_truth("t1"), None, "not strings alone");
    }

    /// Verifies: [DESIGN] RULES.md D31: an OR of `IN_SET_LEAST` equalities
    /// or more of one column with constants is made a set, under AND,
    /// NOT and IS too; a shorter one, one of two columns, and one already a
    /// set are left as they are.
    #[test]
    fn long_in_lists_become_sets() {
        let names = ["A".to_string(), "B".to_string()];
        let values: Vec<Value> = (0..8_i128).map(int).collect();
        let long = in_list_of(&values);
        let made = judged_by_sets(Cond::Not(Box::new(long.clone())));
        assert_eq!(render_cond(&made, &names), "NOT (A IN (0, 1, 2, 3, 4, 5, 6, 7))");
        let both = Cond::And(Box::new(long.clone()), Box::new(Cond::Is(Box::new(long.clone()), Truth::True, false)));
        let shown = render_cond(&judged_by_sets(both), &names);
        assert_eq!(shown.matches(" IN (").count(), 2, "{shown}");
        let short = in_list_of(&values[..7]);
        assert!(matches!(judged_by_sets(short), Cond::Or(..)));
        let other = Cond::Compare(Expr::Column(1), CompOp::Eq, Expr::Const(int(9)));
        let two = Cond::Or(Box::new(long), Box::new(other));
        let Cond::Or(a, b) = judged_by_sets(two) else { panic!("an OR of two columns stays one") };
        assert!(matches!((*a, *b), (Cond::In(..), Cond::Compare(..))));
        let set = Cond::In(Expr::Column(0), Arc::new(InSet::of(values)));
        assert!(matches!(judged_by_sets(set.clone()), Cond::In(..)));
        // Its column renumbered as any condition's is; it raises nothing.
        let moved = crate::logical::map_cond_columns(set, &|c| c.saturating_add(1));
        assert!(render_cond(&moved, &names).starts_with("B IN (0, 1"));
        assert!(!moved.can_raise() && moved.subqueries().is_empty());
    }
}
