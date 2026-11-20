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
/// the point and after it are 17 or 18 in all, and can at 19; a column of
/// no known type, or an approximate one, can.
#[test]
fn sums_raise_past_eighteen_digits() {
    let sum = arith(Expr::Column(0), ArithOp::Add, Expr::Column(1));
    let difference = arith(Expr::Column(0), ArithOp::Sub, Expr::Column(1));
    assert!(!raises(&sum, &exact(16, 0), &exact(15, 0)), "17 digits");
    assert!(!raises(&sum, &exact(17, 0), &exact(17, 0)), "18 digits");
    assert!(raises(&sum, &exact(18, 0), &exact(1, 0)), "19 digits");
    assert!(!raises(&difference, &exact(15, 2), &exact(9, 3)), "14 and 3 digits");
    assert!(raises(&difference, &exact(17, 2), &exact(9, 3)), "16 and 3 digits");
    assert!(!raises(&arith(Expr::Column(0), ArithOp::Add, number(1, 0)), &exact(10, 0), &exact(1, 0)), "INTEGER + 1");
    assert!(raises(&sum, &DataType::Approximate { precision: 53, spelled: Spelled::Numeric }, &exact(1, 0)));
    assert!(can_raise_typed(&sum, &|_| None), "no known type");
    let nested = arith(Expr::Neg(Box::new(sum.clone())), ArithOp::Add, number(5, 1));
    assert!(!raises(&nested, &exact(10, 0), &exact(10, 0)), "-(A + B) + 0.5, 13 digits");
}

/// Verifies: [X3.135-1986 §5.9 SR4c, GR6a] [DESIGN] RULES.md D373: a
/// product of exact columns cannot raise while its precisions sum to 17
/// or 18, and can at 19.
#[test]
fn products_raise_past_eighteen_digits() {
    let product = arith(Expr::Column(0), ArithOp::Mul, Expr::Column(1));
    assert!(!raises(&product, &exact(9, 0), &exact(8, 0)), "17 digits");
    assert!(!raises(&product, &exact(9, 2), &exact(9, 2)), "18 digits");
    assert!(raises(&product, &exact(10, 0), &exact(9, 0)), "19 digits");
    assert!(raises(&product, &exact(10, 0), &exact(10, 0)), "INTEGER * INTEGER");
}

/// Verifies: [X3.135-1986 §5.9 GR5, GR6b] [DESIGN] RULES.md D373: a
/// quotient by a constant cannot raise when the constant is at least 1 in
/// magnitude and its digits before the point and the dividend's scale sum
/// to at most 18; by 0, by a column, by less than 1 or past 18 digits it
/// can.
///
/// Decision `at_least_one && whole + scale <= 18` in `quotient_fits`.
/// MC/DC: (T T) false; (F T) true shows `at_least_one`; (T F) true shows
/// the digits.
#[test]
fn quotients_by_constants_raise_only_when_small_or_long() {
    let by = |divisor: Expr| arith(Expr::Column(0), ArithOp::Div, divisor);
    assert!(!raises(&by(number(4, 0)), &exact(10, 2), &exact(1, 0)), "T T");
    assert!(!raises(&by(number(-25, 1)), &exact(10, 2), &exact(1, 0)), "T T, -2.5");
    assert!(raises(&by(number(5, 1)), &exact(10, 2), &exact(1, 0)), "F T, 0.5");
    assert!(raises(&by(number(10, 0)), &exact(18, 17), &exact(1, 0)), "T F, 2 + 17 digits");
    assert!(!raises(&by(number(10, 0)), &exact(18, 16), &exact(1, 0)), "T T, 2 + 16 digits");
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
    let types = |c: usize| [exact(10, 0), exact(18, 0)].get(c).cloned();
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
