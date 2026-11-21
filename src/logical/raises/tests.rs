//! Unit tests: which expressions the declared types prove cannot raise,
//! at each rule's precision boundary (RULES.md R-TEST-1, R-TEST-8).

use super::*;
use crate::catalog::MemoryCatalog;
use crate::logical::Truth;
use crate::value::{CharUnits, CompOp, Spelled};

fn exact(precision: u32, scale: u32) -> DataType {
    DataType::Exact { precision, scale, spelled: Spelled::Numeric }
}

fn varying(length: u32) -> DataType {
    DataType::Varying { length, units: CharUnits::default() }
}

fn number(units: i128, scale: u32) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(units, scale)))
}

fn arith(a: Expr, op: ArithOp, b: Expr) -> Expr {
    Expr::Arith(Box::new(a), op, Box::new(b))
}

/// Whether `e` can raise, column 0 of `first`'s type and column 1 of
/// `second`'s.
fn raises(e: &Expr, first: &DataType, second: &DataType) -> bool {
    let types = |c: usize| [first.clone(), second.clone()].get(c).cloned();
    can_raise_typed(e, &types)
}

/// Verifies: [X3.135-1986 §5.9 SR4b, GR6a] [DESIGN] RULES.md D373: a sum
/// or difference of exact columns cannot raise while its digits before
/// the point and after it are 37 or 38 in all, and can at 39; a column of
/// no known type, or an approximate one, can.
#[test]
fn sums_raise_past_thirty_eight_digits() {
    let sum = arith(Expr::Column(0), ArithOp::Add, Expr::Column(1));
    let difference = arith(Expr::Column(0), ArithOp::Sub, Expr::Column(1));
    assert!(!raises(&sum, &exact(36, 0), &exact(15, 0)), "37 digits");
    assert!(!raises(&sum, &exact(37, 0), &exact(37, 0)), "38 digits");
    assert!(raises(&sum, &exact(38, 0), &exact(1, 0)), "39 digits");
    assert!(!raises(&difference, &exact(35, 2), &exact(9, 3)), "34 and 3 digits");
    assert!(raises(&difference, &exact(37, 2), &exact(9, 3)), "36 and 3 digits");
    assert!(!raises(&arith(Expr::Column(0), ArithOp::Add, number(1, 0)), &exact(10, 0), &exact(1, 0)), "INTEGER + 1");
    assert!(raises(&sum, &DataType::Approximate { precision: 53, spelled: Spelled::Numeric }, &exact(1, 0)));
    assert!(can_raise_typed(&sum, &|_| None), "no known type");
    let nested = arith(Expr::Neg(Box::new(sum.clone())), ArithOp::Add, number(5, 1));
    assert!(!raises(&nested, &exact(10, 0), &exact(10, 0)), "-(A + B) + 0.5, 13 digits");
}

/// Verifies: [X3.135-1986 §5.9 SR4c, GR6a] [DESIGN] RULES.md D373: a
/// product of exact columns cannot raise while its precisions sum to 37
/// or 38, and can at 39: INTEGER * INTEGER cannot.
#[test]
fn products_raise_past_thirty_eight_digits() {
    let product = arith(Expr::Column(0), ArithOp::Mul, Expr::Column(1));
    assert!(!raises(&product, &exact(19, 0), &exact(18, 0)), "37 digits");
    assert!(!raises(&product, &exact(19, 2), &exact(19, 2)), "38 digits");
    assert!(raises(&product, &exact(20, 0), &exact(19, 0)), "39 digits");
    assert!(!raises(&product, &exact(10, 0), &exact(10, 0)), "INTEGER * INTEGER");
}

/// Verifies: [X3.135-1986 §5.9 GR5, GR6b] [DESIGN] RULES.md D373, D697:
/// a quotient by a constant cannot raise when the constant is at least 1
/// in magnitude, its scale giving way to the dividend's integer digits,
/// even of 38; by 0, by a column or by less than 1 it can. Decision
/// `at_least_one` in `quotient_fits`: true and false each shown.
#[test]
fn quotients_by_constants_raise_only_when_small_or_long() {
    let by = |divisor: Expr| arith(Expr::Column(0), ArithOp::Div, divisor);
    assert!(!raises(&by(number(4, 0)), &exact(10, 2), &exact(1, 0)), "at least 1");
    assert!(!raises(&by(number(-25, 1)), &exact(10, 2), &exact(1, 0)), "-2.5");
    assert!(raises(&by(number(5, 1)), &exact(10, 2), &exact(1, 0)), "0.5");
    assert!(!raises(&by(number(10, 0)), &exact(38, 0), &exact(1, 0)), "38 integer digits");
    assert!(raises(&by(number(0, 0)), &exact(10, 2), &exact(1, 0)), "0");
    assert!(raises(&by(Expr::Column(1)), &exact(10, 2), &exact(10, 0)), "a column");
    let twice = arith(by(number(4, 0)), ArithOp::Add, number(1, 0));
    assert!(raises(&twice, &exact(10, 2), &exact(1, 0)), "a quotient's scale is not bounded");
}

/// Verifies: [ISO/IEC 9075:1992 §6.10 GR3a ii, GR5a iv] [DESIGN] RULES.md
/// D373: a CAST to an exact type of no fewer digits before the point, or to
/// a character string type as long as an exact value's longest literal or
/// the operand string's length in the same units, cannot raise; one that
/// narrows, or of another type, can.
#[test]
fn widening_casts_cannot_raise() {
    let cast = |to: DataType| Expr::Cast(Box::new(Expr::Column(0)), to);
    assert!(!raises(&cast(exact(10, 3)), &exact(9, 2), &exact(1, 0)));
    assert!(!raises(&cast(exact(7, 0)), &exact(9, 2), &exact(1, 0)), "the fraction cut");
    assert!(raises(&cast(exact(8, 2)), &exact(9, 2), &exact(1, 0)));
    assert!(!raises(&cast(varying(12)), &exact(9, 2), &exact(1, 0)), "9 digits and 3");
    assert!(raises(&cast(varying(11)), &exact(9, 2), &exact(1, 0)));
    assert!(!raises(&cast(varying(10)), &varying(10), &exact(1, 0)));
    assert!(raises(&cast(varying(9)), &varying(10), &exact(1, 0)));
    let octets = DataType::Varying { length: 40, units: CharUnits::Octets };
    assert!(raises(&cast(octets), &varying(10), &exact(1, 0)), "other units");
    assert!(raises(&cast(DataType::Date), &varying(10), &exact(1, 0)));
    assert!(raises(&cast(exact(10, 0)), &DataType::Date, &exact(1, 0)));
}

/// Verifies: [DESIGN] RULES.md D373: a condition can raise when one of its
/// expressions or conditions can by the types, as `Cond::can_raise` but
/// for the types.
///
/// Decision `cond_raises(a) || cond_raises(b)` of AND. MC/DC: (F F)
/// false; (T F) true; (F T) true.
#[test]
fn conditions_raise_through_their_typed_parts() {
    let types = |c: usize| [exact(10, 0), exact(38, 0)].get(c).cloned();
    let safe = || Cond::Compare(arith(Expr::Column(0), ArithOp::Add, number(1, 0)), CompOp::Gt, Expr::Column(1));
    let risky = || Cond::Compare(arith(Expr::Column(1), ArithOp::Add, number(1, 0)), CompOp::Gt, Expr::Column(0));
    assert!(safe().can_raise() && !cond_can_raise_typed(&safe(), &types));
    assert!(cond_can_raise_typed(&risky(), &types));
    assert!(!cond_can_raise_typed(&Cond::And(Box::new(safe()), Box::new(safe())), &types)); // F F
    assert!(cond_can_raise_typed(&Cond::And(Box::new(risky()), Box::new(safe())), &types)); // T F
    assert!(cond_can_raise_typed(&Cond::And(Box::new(safe()), Box::new(risky())), &types)); // F T
    assert!(cond_can_raise_typed(&Cond::Not(Box::new(risky())), &types));
    assert!(!cond_can_raise_typed(&Cond::Is(Box::new(safe()), Truth::True, false), &types));
    let like = Cond::Like(Expr::Column(0), Expr::Column(1), None);
    assert!(cond_can_raise_typed(&like, &types), "a pattern not constant");
    let truth = Expr::Truth(Box::new(risky()));
    assert!(can_raise_typed(&truth, &types));
}

/// Verifies: [DESIGN] RULES.md D373: a column's declared type is found
/// through products, filters and projections of columns, and of a table
/// value constructor; a computed column's is not.
#[test]
fn column_types_are_found_through_plans() {
    let catalog = MemoryCatalog::default();
    let values = || LogicalPlan::Values { rows: Vec::new(), types: vec![exact(3, 1), varying(4)] };
    let product = LogicalPlan::Product { left: Box::new(values()), right: Box::new(values()) };
    assert!(matches!(plan_column_type(&product, 3, &catalog), Some(DataType::Varying { length: 4, .. })));
    assert!(matches!(plan_column_type(&product, 0, &catalog), Some(DataType::Exact { precision: 3, .. })));
    let project = LogicalPlan::Project { exprs: vec![Expr::Column(1), number(1, 0)], input: Box::new(values()) };
    assert!(matches!(plan_column_type(&project, 0, &catalog), Some(DataType::Varying { .. })));
    assert!(plan_column_type(&project, 1, &catalog).is_none(), "computed");
    let filter = LogicalPlan::Filter { cond: Cond::Const(Truth::True), input: Box::new(values()) };
    assert!(plan_column_type(&filter, 1, &catalog).is_some());
    assert!(plan_column_type(&LogicalPlan::Distinct { input: Box::new(values()) }, 0, &catalog).is_none());
}
