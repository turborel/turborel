//! \[DESIGN\] IN lists judged by a set (RULES.md D31). An `<in predicate>`
//! of constants is bound as an OR of equalities (`binder/predicates.rs`),
//! which the planner reads as such: a probe of each value of an index, a
//! scan told the list. The condition a filter then judges row by row, a
//! long such list among it, is judged by a set of its values' buckets
//! (`value::bucket`), made once as the plan is made: a row's value is
//! looked for in its bucket, not compared with every value. A row's
//! judgment is that of the OR [X3.135-1986 §5.13 GR2, §5.18 GR2]: true if
//! the value equals one, as SQL compares them, a string padded with spaces
//! [§5.11 GR5], numbers by algebraic value [GR4]; else unknown if it or
//! one of the list is null [§5.11 GR2]; else false.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use super::{Cond, Expr, Truth, in_list};
use crate::error::Result;
use crate::value::{Bucket, CompOp, Value, any_lob, bucket, compare, compare_checked};

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
    /// Of constants that are character strings alone, nulls aside, each
    /// without its trailing spaces, as two strings are equal if they are
    /// equal so: of a list of any length, as a vector of strings is
    /// judged by them, no value made of each (RULES.md D1126), in order
    /// and each once, so a string is looked for by halving them (D1131).
    texts: Option<Vec<String>>,
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
            Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Lob(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Json(_)
            | Value::Extension(_)
            | Value::Xml(_) => None,
        };
        let texts = values.iter().map(strings).collect::<Option<Vec<_>>>().map(|t| {
            let mut texts: Vec<String> = t.into_iter().flatten().collect();
            texts.sort_unstable();
            texts.dedup();
            texts
        });
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

    /// `truth`, of a value that may be or hold a large object, compared
    /// with each constant in turn, read as it must be (`value::proxies`).
    ///
    /// # Errors
    ///
    /// As reading a large object fails.
    pub fn truth_read(&self, v: &Value) -> Result<Truth> {
        if v.is_null() || !any_lob(std::slice::from_ref(v))? {
            return Ok(self.truth(v));
        }
        for c in &self.values {
            if compare_checked(v, c)? == Some(Ordering::Equal) {
                return Ok(Truth::True);
            }
        }
        Ok(self.decided(false))
    }

    /// Whether its constants are character strings alone, nulls aside.
    pub fn of_strings(&self) -> bool {
        self.texts.is_some()
    }

    /// [X3.135-1986 §5.13 GR2, §5.11 GR5] The string `s` IN the constants,
    /// if they are strings alone: looked for without its trailing spaces.
    pub fn text_truth(&self, s: &str) -> Option<Truth> {
        let texts = self.texts.as_ref()?;
        let unpadded = s.trim_end_matches(' ');
        // Unpadded once as the set is made, in order: a string is compared
        // with as many constants as halving them takes, not hashed.
        Some(self.decided(texts.binary_search_by(|c| c.as_str().cmp(unpadded)).is_ok()))
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
    if let Cond::Or(..) = cond
        && let Some((computed, values)) = computed_list(&cond, 0)
    {
        return Cond::In(computed.clone(), Arc::new(InSet::of(values)));
    }
    match cond {
        Cond::Or(a, b) => Cond::Or(Box::new(judged_by_sets(*a)), Box::new(judged_by_sets(*b))),
        Cond::And(a, b) => Cond::And(Box::new(judged_by_sets(*a)), Box::new(judged_by_sets(*b))),
        Cond::Not(a) => Cond::Not(Box::new(judged_by_sets(*a))),
        Cond::Is(a, truth, not) => Cond::Is(Box::new(judged_by_sets(*a)), truth, not),
        other @ (Cond::Const(_)
        | Cond::Compare(..)
        | Cond::Distinct(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Quantified(..)
        | Cond::Unique(..)
        | Cond::Match { .. }
        | Cond::In(..)
        | Cond::Value(_)) => other,
    }
}

/// \[DESIGN\] (RULES.md D1067) The expression and constants of `cond` if it
/// is an OR of equalities of one computed expression with constants, as
/// the binder writes an `<in predicate>` of an expression that is no
/// column, `SUBSTRING(C FROM 1 FOR 2) IN ('13', '31')`. The OR computes
/// the expression once for each constant, as it evaluates every operand
/// (RULES.md D15), and the set computes it once, so a list of two is
/// already judged by its set. The expression is deterministic and reads
/// no subquery, so it has one value and raises one error however often
/// it is computed. Past `MAX_JUDGED_DEPTH` none is found (RULES.md
/// R-CF-6).
fn computed_list(cond: &Cond, depth: usize) -> Option<(&Expr, Vec<Value>)> {
    if depth > super::MAX_JUDGED_DEPTH {
        return None;
    }
    if let Cond::Or(a, b) = cond {
        let (e, mut list) = computed_list(a, depth.saturating_add(1))?;
        let (other, more) = computed_list(b, depth.saturating_add(1))?;
        list.extend(more);
        return same(e, other, 0).then_some((e, list));
    }
    let Cond::Compare(a, CompOp::Eq, b) = cond else { return None };
    let ((computed, Expr::Const(value)) | (Expr::Const(value), computed)) = (a, b) else { return None };
    recomputable(computed, 0).then(|| (computed, vec![value.clone()]))
}

/// Whether `e` is computed of columns and constants by deterministic
/// functions, arithmetic and casts alone, and is no column or constant
/// itself.
fn recomputable(e: &Expr, depth: usize) -> bool {
    let own = match e {
        Expr::Call(f, _) => f.0.deterministic(),
        Expr::Neg(_) | Expr::Arith(..) | Expr::Cast(..) => true,
        Expr::Column(_) | Expr::Const(_) => depth > 0,
        Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Case(..)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Subquery(..)
        | Expr::Truth(_)
        | Expr::Warned(..)
        | Expr::NextValue(_)
        | Expr::Invoked(_) => false,
    };
    let mut parts = depth <= super::MAX_JUDGED_DEPTH;
    e.for_each_child(&mut |child| parts = parts && recomputable(child, depth.saturating_add(1)));
    own && parts
}

/// Whether `a` and `b`, each `recomputable`, are the same expression: the
/// same operator or function of the same parts, a constant as it is
/// written.
fn same(a: &Expr, b: &Expr, depth: usize) -> bool {
    let own = same_node(a, b);
    let (mut of_a, mut of_b) = (Vec::new(), Vec::new());
    a.for_each_child(&mut |child| of_a.push(child));
    b.for_each_child(&mut |child| of_b.push(child));
    let deeper = depth.saturating_add(1);
    own && depth <= super::MAX_JUDGED_DEPTH && of_a.len() == of_b.len() && of_a.iter().zip(&of_b).all(|(x, y)| same(x, y, deeper))
}

/// Whether `a` and `b`, each `recomputable`, are the same node, their parts
/// aside: the same column, constant, function, operator or cast.
fn same_node(a: &Expr, b: &Expr) -> bool {
    match a {
        Expr::Column(x) => {
            let Expr::Column(y) = b else { return false };
            x == y
        }
        Expr::Const(x) => {
            let Expr::Const(y) = b else { return false };
            format!("{x:?}") == format!("{y:?}")
        }
        Expr::Call(f, _) => {
            let Expr::Call(g, _) = b else { return false };
            Arc::ptr_eq(&f.0, &g.0)
        }
        Expr::Neg(_) => matches!(b, Expr::Neg(_)),
        Expr::Arith(_, x, _) => {
            let Expr::Arith(_, y, _) = b else { return false };
            x == y
        }
        Expr::Cast(_, x) => {
            let Expr::Cast(_, y) = b else { return false };
            x == y
        }
        Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Case(..)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Subquery(..)
        | Expr::Truth(_)
        | Expr::Warned(..)
        | Expr::NextValue(_)
        | Expr::Invoked(_) => false,
    }
}

// Unit tests: what a set finds, and which ORs become one (RULES.md
// R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::logical::{or_all, render_cond};
    use crate::value::{ArithOp, Decimal};

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
        let texts: Vec<Value> = (0_i32..10_i32).map(|n| Value::Char(format!("t{n}  ").into())).chain([Value::Null]).collect();
        let set = InSet::of(texts);
        assert!(set.of_strings());
        assert_eq!(set.text_truth("t3"), Some(Truth::True));
        assert_eq!(set.text_truth("t3 "), Some(Truth::True));
        assert_eq!(set.text_truth("t3\t"), Some(Truth::Unknown), "not equal, and a null among them");
        assert_eq!(set.truth(&Value::Char("t9".into())), Truth::True);
        let mixed: Vec<Value> = (0_i32..10_i32).map(|n| Value::Char(format!("t{n}").into())).chain([int(1)]).collect();
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

    /// Verifies: [DESIGN] RULES.md D1067, D15: an OR of two equalities or
    /// more of one computed expression with constants, either way round,
    /// is judged by their set, the expression computed once; one of two
    /// expressions, and a short one of a column, stay the OR they are.
    #[test]
    fn a_list_of_a_computed_expression_is_a_set() {
        let names = ["A".to_string(), "B".to_string()];
        let plus = |c: usize| Expr::Arith(Box::new(Expr::Column(c)), ArithOp::Add, Box::new(Expr::Const(int(1))));
        let is = |e: Expr, v: i128| Cond::Compare(e, CompOp::Eq, Expr::Const(int(v)));
        let or = |a: Cond, b: Cond| Cond::Or(Box::new(a), Box::new(b));
        let made = judged_by_sets(or(is(plus(0), 3), is(plus(0), 5)));
        assert!(matches!(made, Cond::In(..)) && render_cond(&made, &names).ends_with("IN (3, 5)"), "{}", render_cond(&made, &names));
        let turned = or(Cond::Compare(Expr::Const(int(3)), CompOp::Eq, plus(0)), or(is(plus(0), 5), is(plus(0), 7)));
        assert!(matches!(judged_by_sets(turned), Cond::In(..)));
        assert!(matches!(judged_by_sets(or(is(plus(0), 3), is(plus(1), 5))), Cond::Or(..)), "two expressions");
        let other = Expr::Arith(Box::new(Expr::Column(0)), ArithOp::Add, Box::new(Expr::Const(int(2))));
        assert!(matches!(judged_by_sets(or(is(plus(0), 3), is(other, 5))), Cond::Or(..)), "another constant in it");
        assert!(matches!(judged_by_sets(in_list_of(&[int(1), int(2)])), Cond::Or(..)), "a column's short list");
    }
}
