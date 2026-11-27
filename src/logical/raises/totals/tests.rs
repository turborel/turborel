//! Unit tests: which sums the declared types keep within an exact
//! number's digits, at the boundary, and the type a grouping's column is
//! within (RULES.md R-TEST-1, R-TEST-8).

use super::*;
use crate::catalog::MemoryCatalog;
use crate::functions::Registry;
use crate::lexer::kw;
use crate::logical::raises::{Judge, can_raise_judged, cond_can_raise_judged, plan_column_type};
use crate::logical::{Cond, Expr, LogicalPlan, Subquery};
use crate::value::{ArithOp, CompOp, Decimal, Value};

fn exact(precision: u32, scale: u32) -> DataType {
    DataType::Exact { precision, scale, spelled: Spelled::Numeric }
}

/// The set function `name` of `arg`, its result typed of an argument of
/// type `of`.
fn call(name: &str, arg: Expr, of: &DataType) -> AggCall {
    let func = Registry::with_set_functions().aggregate(name).unwrap();
    let result = func.0.result_type(Some(of.clone())).unwrap();
    AggCall { func, arg: Some(arg), distinct: false, filter: None, distribution: None, result, label: name.to_owned() }
}

/// The grouping by column 0 of a table value constructor of `types`,
/// computing `aggs`.
fn grouping(types: Vec<DataType>, aggs: Vec<AggCall>) -> LogicalPlan {
    let input = LogicalPlan::Values { rows: Vec::new(), types };
    LogicalPlan::Aggregate { groups: vec![0], collations: Vec::new(), aggs, seeds: Vec::new(), input: Box::new(input) }
}

/// Verifies: [ISO/IEC 9075:1992 §6.5 GR2b iv] [DESIGN] RULES.md D1060: a
/// SUM or AVG of exact values of 19 digits has a sum of 38 at most and
/// raises nothing, and one of 20 digits can; of an expression, by the
/// digits its operators bound it to; of approximate values or a column of
/// no known type it can.
///
/// Decision `p + ROW_DIGITS <= MAX_EXACT_PRECISION`: 38 true, 39 false.
#[test]
fn sums_fit_while_their_digits_and_the_rows_do() {
    let column = |t: DataType| move |c: usize| (c == 0).then(|| t.clone());
    for name in [kw::SUM, kw::AVG] {
        let of = |t: DataType| total_fits(&call(name, Expr::Column(0), &t), &column(t.clone()));
        assert!(of(exact(19, 2)), "{name}: 38 digits");
        assert!(!of(exact(20, 2)), "{name}: 39 digits");
        assert!(of(exact(10, 0)), "{name}: INTEGER");
        assert!(!of(DataType::Approximate { precision: 53, spelled: Spelled::Float }), "{name}: approximate");
        assert!(!total_fits(&call(name, Expr::Column(0), &exact(5, 0)), &|_| None), "{name}: no known type");
    }
    let product = Expr::Arith(Box::new(Expr::Column(0)), ArithOp::Mul, Box::new(Expr::Column(0)));
    assert!(total_fits(&call(kw::SUM, product.clone(), &exact(18, 0)), &column(exact(9, 0))), "18 digits of a product");
    assert!(!total_fits(&call(kw::SUM, product, &exact(20, 0)), &column(exact(10, 0))), "20 digits of a product");
}

/// Verifies: [X3.135-1986 §5.8 SR9b, SR9c] [DESIGN] RULES.md D1060: a
/// grouping's column has its grouping column's type, a SUM's the digits
/// of its argument and of the rows, an AVG's its argument's with the
/// scale AVG adds, MAX's its argument's and COUNT's its result's; a sum
/// past an exact number's digits has its result's type, and a grouping
/// with seeded groups none.
#[test]
fn a_grouping_s_columns_are_typed_by_their_set_functions() {
    let catalog = MemoryCatalog::default();
    let types = vec![exact(4, 0), exact(15, 2), exact(30, 0)];
    let aggs = vec![
        call(kw::SUM, Expr::Column(1), &exact(15, 2)),
        call(kw::AVG, Expr::Column(1), &exact(15, 2)),
        call(kw::MAX, Expr::Column(1), &exact(15, 2)),
        call(kw::COUNT, Expr::Column(1), &exact(15, 2)),
        call(kw::SUM, Expr::Column(2), &exact(30, 0)),
    ];
    let plan = grouping(types.clone(), aggs.clone());
    let shape = |column: usize| {
        let Some(DataType::Exact { precision, scale, .. }) = plan_column_type(&plan, column, &catalog) else { return None };
        Some((precision, scale))
    };
    assert_eq!(shape(0), Some((4, 0)), "the grouping column");
    assert_eq!(shape(1), Some((34, 2)), "SUM");
    assert_eq!(shape(2), Some((17, 4)), "AVG");
    assert_eq!(shape(3), Some((15, 2)), "MAX");
    assert!(shape(4).is_some(), "COUNT");
    assert_eq!(shape(5), Some((MAX_EXACT_PRECISION, 0)), "a sum that may not fit");
    assert_eq!(shape(6), None, "no such column");
    let LogicalPlan::Aggregate { input, .. } = plan else { panic!() };
    let seeded = LogicalPlan::Aggregate { groups: vec![0], collations: Vec::new(), aggs, seeds: vec![Vec::new()], input };
    assert!(plan_column_type(&seeded, 1, &catalog).is_none());
}

/// Verifies: [ISO/IEC 9075:1992 §7.11 GR1] [DESIGN] RULES.md D1060: a
/// subquery of an expression or a condition is judged as the judgment's
/// `subquery` says, told whether it stands for one row; its outer
/// expressions are judged with the expression that holds it.
#[test]
fn a_judgment_asks_of_each_subquery() {
    let sub = |outer: Vec<Expr>| std::sync::Arc::new(Subquery::new(LogicalPlan::Values { rows: Vec::new(), types: Vec::new() }, outer));
    let types = |_: usize| Some(exact(38, 0));
    let quiet = Judge { types: &types, subquery: &|_, _| false };
    let one_only = Judge { types: &types, subquery: &|_, one| one };
    let scalar = Cond::Compare(Expr::Column(0), CompOp::Gt, Expr::Subquery(sub(Vec::new()), 0));
    assert!(!cond_can_raise_judged(&scalar, &quiet));
    assert!(cond_can_raise_judged(&scalar, &one_only));
    assert!(crate::logical::cond_can_raise_typed(&scalar, &types), "taken as raising where no more is known");
    let exists = Cond::Exists(sub(Vec::new()));
    assert!(!cond_can_raise_judged(&exists, &one_only), "EXISTS stands for no one row");
    let compared = Cond::Quantified(vec![Expr::Column(0)], CompOp::Eq, crate::logical::Quantifier::One, sub(Vec::new()));
    assert!(cond_can_raise_judged(&compared, &one_only) && !cond_can_raise_judged(&compared, &quiet));
    let one = Expr::Const(Value::Exact(Decimal::new(1, 0)));
    let risky = Expr::Arith(Box::new(Expr::Column(0)), ArithOp::Add, Box::new(one));
    assert!(can_raise_judged(&Expr::Subquery(sub(vec![risky]), 0), &quiet), "an outer expression that can raise");
}
