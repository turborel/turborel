//! Tests of the order a filter judges its conjuncts in.

use super::{cost, ranked};
use crate::logical::{Cond, Expr};
use crate::physical::CostModel;
use crate::physical::estimate::Known;
use crate::value::{ArithOp, CompOp, Decimal, Value};

/// A constant `n`.
fn constant(n: i64) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n.into(), 0)))
}

/// Column `c` compared by `op` with `n`.
fn compared(c: usize, op: CompOp, n: i64) -> Cond {
    Cond::Compare(Expr::Column(c), op, constant(n))
}

/// The columns each of `conds` compares, in their order.
fn columns(conds: &[Cond]) -> Vec<usize> {
    conds
        .iter()
        .filter_map(|c| match c {
            Cond::Compare(Expr::Column(c), ..) | Cond::IsNull(Expr::Column(c), _) => Some(*c),
            Cond::Compare(..) | Cond::IsNull(..) => None,
            Cond::Const(_)
            | Cond::Or(..)
            | Cond::And(..)
            | Cond::Not(_)
            | Cond::Distinct(..)
            | Cond::Like(..)
            | Cond::Exists(_)
            | Cond::Quantified(..)
            | Cond::Is(..)
            | Cond::Unique(..)
            | Cond::Match { .. }
            | Cond::Value(_)
            | Cond::In(..) => None,
        })
        .collect()
}

/// Verifies: [DESIGN] (RULES.md D650) of conjuncts of the same cost, the
/// one dropping more rows is judged first: an equality, keeping one row in
/// a hundred by default, before an order comparison, keeping a third,
/// though written after it.
#[test]
fn the_conjunct_dropping_more_is_judged_first() {
    let model = CostModel::default();
    let known = Known { stats: None, cost: &model };
    let conds = vec![compared(0, CompOp::Gt, 5), compared(1, CompOp::Eq, 7)];
    assert_eq!(columns(&ranked(conds, known)), vec![1, 0]);
}

/// Verifies: [DESIGN] (RULES.md D650) of conjuncts dropping the same
/// share, the cheaper is judged first, a column compared with a constant
/// before a comparison of two columns, which a program evaluates; and
/// conjuncts of the same rank keep their written order.
#[test]
fn the_cheaper_conjunct_is_judged_first_and_ties_keep_their_order() {
    let model = CostModel::default();
    let known = Known { stats: None, cost: &model };
    let two = Cond::Compare(Expr::Column(2), CompOp::Gt, Expr::Column(3));
    let conds = vec![two, compared(0, CompOp::Gt, 5), compared(1, CompOp::Lt, 5)];
    assert_eq!(columns(&ranked(conds, known)), vec![0, 1, 2]);
    assert!(cost(&compared(0, CompOp::Gt, 5)) < cost(&Cond::Compare(Expr::Column(2), CompOp::Gt, Expr::Column(3))));
}

/// Verifies: [DESIGN] (RULES.md D650, D15) a filter one of whose
/// conjuncts can raise an error keeps the order it was given, though
/// another order would rank better.
#[test]
fn a_filter_that_can_raise_keeps_its_order() {
    let model = CostModel::default();
    let known = Known { stats: None, cost: &model };
    let sum = Expr::Arith(Box::new(Expr::Column(0)), ArithOp::Add, Box::new(constant(1)));
    let raising = Cond::Compare(sum, CompOp::Gt, constant(5));
    let conds = vec![compared(2, CompOp::Gt, 5), raising, compared(1, CompOp::Eq, 7)];
    assert_eq!(columns(&ranked(conds, known)), vec![2, 1]);
}
