//! Unit tests of what a range join's search answers (RULES.md D580, D581).

use std::sync::Arc;

use super::{Bound, expanded_periods, range_cost, ranged};
use crate::ast::PeriodOperator;
use crate::functions::{PeriodTest, ScalarRef};
use crate::logical::{Cond, Expr, render_cond, render_expr};
use crate::physical::CostModel;
use crate::value::{ArithOp, CompOp, Decimal, Value};

/// The names of a pair's columns: two of the left, then two of the right.
fn names() -> Vec<String> {
    ["A.X", "A.K", "B.Y", "B.K"].map(str::to_owned).to_vec()
}

fn col(i: usize) -> Expr {
    Expr::Column(i)
}

fn plus(e: Expr, n: i32) -> Expr {
    Expr::Arith(Box::new(e), ArithOp::Add, Box::new(Expr::Const(Value::Exact(Decimal::new(n.into(), 0)))))
}

fn cmp(a: Expr, op: CompOp, b: Expr) -> Cond {
    Cond::Compare(a, op, b)
}

/// A bound as text, `=` before it if closed.
fn bound(b: Option<&Bound>) -> String {
    let pair = names();
    b.map_or_else(|| "-".to_owned(), |b| format!("{}{}", if b.closed { "=" } else { "" }, render_expr(&b.probe, &pair)))
}

/// What `conjuncts` search by, over two left columns, as text: the sorted
/// value, each bound, the pruning comparison, the keys, the conjuncts
/// searched and the rest.
fn shown(conjuncts: Vec<Cond>) -> Option<String> {
    let r = ranged(conjuncts, 2)?;
    let (pair, own) = (names(), names()[2..].to_vec());
    let pruned = r
        .pruned
        .as_ref()
        .map_or_else(|| "-".to_owned(), |p| format!("{} {:?} {}", render_expr(&p.build, &own), p.op, render_expr(&p.probe, &pair)));
    let rest: Vec<String> = r.rest.iter().map(|c| render_cond(c, &pair)).collect();
    Some(format!(
        "{} [{} {}] {pruned} keys {} searched {} rest {}",
        render_expr(&r.sorted, &own),
        bound(r.lower.as_ref()),
        bound(r.upper.as_ref()),
        r.keys.len(),
        r.searched.len(),
        rest.join("; ")
    ))
}

/// Verifies: [DESIGN] RULES.md D580: a band of a right column is its
/// lower and upper bounds, closed or open, either way round as written;
/// a band of a computed right value is one bound and the pruning
/// comparison; a second bound of a side is the rest's.
#[test]
fn bands_are_bounds_of_the_sorted_value() {
    let band = vec![cmp(col(2), CompOp::Ge, plus(col(0), -5)), cmp(plus(col(0), 5), CompOp::Gt, col(2))];
    let r = shown(band).unwrap();
    assert!(r.starts_with("B.Y [=(A.X + -5) (A.X + 5)] - keys 0 searched 2 rest"), "{r}");
    let computed = vec![cmp(col(0), CompOp::Ge, plus(col(2), -5)), cmp(col(0), CompOp::Le, plus(col(2), 5))];
    let found = shown(computed).unwrap();
    assert!(found.contains("searched 2") && !found.contains("] - keys"), "{found}");
    let twice = vec![cmp(col(2), CompOp::Gt, col(0)), cmp(col(2), CompOp::Gt, col(1)), cmp(col(2), CompOp::Lt, col(1))];
    let parted = shown(twice).unwrap();
    assert!(parted.starts_with("B.Y [A.X A.K] - keys 0 searched 2 rest B.Y > A.K"), "{parted}");
}

/// Verifies: [DESIGN] RULES.md D580: an equality of a column of each
/// operand is a key; conjuncts of one operand alone, or of `<>`, are the
/// rest's; conjuncts of no comparison by order are no range join's.
#[test]
fn keys_and_the_rest_are_parted() {
    let conjuncts = vec![
        cmp(col(3), CompOp::Eq, col(1)),
        cmp(col(0), CompOp::Lt, col(2)),
        cmp(col(0), CompOp::Ne, col(2)),
        cmp(col(0), CompOp::Lt, col(1)),
    ];
    let r = shown(conjuncts).unwrap();
    assert!(r.starts_with("B.Y [A.X -] - keys 1 searched 2 rest "), "{r}");
    assert!(r.contains("A.X <> B.Y") && r.contains("A.X < A.K"), "{r}");
    assert!(shown(vec![cmp(col(0), CompOp::Eq, col(2)), cmp(col(0), CompOp::Ne, col(2))]).is_none());
    assert!(shown(vec![cmp(col(0), CompOp::Lt, col(1))]).is_none());
    assert!(shown(Vec::new()).is_none());
}

/// Verifies: [DESIGN] RULES.md D580: a value of no column, or of an outer
/// reference or CASE, is compared by no search; one of negation and CAST
/// is.
#[test]
fn values_computed_once_a_row_alone_are_searched() {
    let constant = Expr::Const(Value::Exact(Decimal::new(3, 0)));
    assert!(shown(vec![cmp(constant, CompOp::Lt, col(2))]).is_none());
    assert!(shown(vec![cmp(Expr::Outer(0, "O.Z".to_owned()), CompOp::Lt, col(2))]).is_none());
    let case = Expr::Case(vec![(Cond::IsNull(col(0), false), col(1))].into(), Box::new(col(0)));
    assert!(shown(vec![cmp(case, CompOp::Lt, col(2))]).is_none());
    assert!(shown(vec![cmp(Expr::Neg(Box::new(col(0))), CompOp::Lt, col(2))]).is_some());
}

/// A period predicate of `op` of `args`, of periods `written` or not.
fn period(op: PeriodOperator, written: [bool; 2], args: Vec<Expr>) -> Cond {
    Cond::Value(Expr::Call(ScalarRef(Arc::new(PeriodTest { op, written })), args))
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR4a v, GR4b]; [DESIGN] RULES.md
/// D581: each predicate of two `<period reference>`s, and CONTAINS of a
/// point, is its comparisons; one of a written period, or another
/// condition, stays whole.
#[test]
fn period_predicates_are_their_comparisons_8_20() {
    let pair = names();
    let of = |op, args: Vec<Expr>| -> Vec<String> {
        expanded_periods(vec![period(op, [false, false], args)]).iter().map(|c| render_cond(c, &pair)).collect()
    };
    let four = || vec![col(0), col(1), col(2), col(3)];
    assert_eq!(of(PeriodOperator::Overlaps, four()), ["A.X < B.K", "A.K > B.Y"]);
    assert_eq!(of(PeriodOperator::Equals, four()), ["A.X = B.Y", "A.K = B.K"]);
    assert_eq!(of(PeriodOperator::Contains, four()), ["A.X <= B.Y", "A.K >= B.K"]);
    assert_eq!(of(PeriodOperator::Precedes, four()), ["A.K <= B.Y"]);
    assert_eq!(of(PeriodOperator::Succeeds, four()), ["A.X >= B.K"]);
    assert_eq!(of(PeriodOperator::ImmediatelyPrecedes, four()), ["A.K = B.Y"]);
    assert_eq!(of(PeriodOperator::ImmediatelySucceeds, four()), ["A.X = B.K"]);
    assert_eq!(of(PeriodOperator::Contains, vec![col(0), col(1), col(2)]), ["A.X <= B.Y", "A.K > B.Y"]);
    for written in [[true, false], [false, true]] {
        assert_eq!(expanded_periods(vec![period(PeriodOperator::Overlaps, written, four())]).len(), 1);
    }
    let other = cmp(col(0), CompOp::Lt, col(2));
    assert_eq!(expanded_periods(vec![other.clone(), Cond::Not(Box::new(other))]).len(), 2);
}

/// Verifies: [DESIGN] RULES.md D580: a range join's cost grows more with
/// the rows it sorts than with those that search, and a pruning
/// comparison adds to it.
#[test]
fn range_costs_weigh_the_sort_and_the_search() {
    let model = CostModel::default();
    let sorted_more = range_cost(&model, (100.0_f64, 10_000.0_f64), (50.0_f64, 50.0_f64), false);
    let searched_more = range_cost(&model, (10_000.0_f64, 100.0_f64), (50.0_f64, 50.0_f64), false);
    assert!(sorted_more > searched_more, "{sorted_more} {searched_more}");
    let plain = range_cost(&model, (100.0_f64, 100.0_f64), (50.0_f64, 50.0_f64), false);
    assert!(range_cost(&model, (100.0_f64, 100.0_f64), (50.0_f64, 50.0_f64), true) > plain);
    assert!(range_cost(&model, (-1.0_f64, -1.0_f64), (0.0_f64, 0.0_f64), false) >= 0.0_f64);
}
