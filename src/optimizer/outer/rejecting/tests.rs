//! Unit tests: the three truth tables over sets of truth values, and the
//! conditions that are never true of a null operand (RULES.md R-TEST-1,
//! R-TEST-8).

use super::*;
use crate::value::{ArithOp, CompOp, Decimal};

const T: Truth = Truth::True;
const F: Truth = Truth::False;
const U: Truth = Truth::Unknown;

fn column(c: usize) -> Expr {
    Expr::Column(c)
}

fn less(a: Expr, b: Expr) -> Cond {
    Cond::Compare(a, CompOp::Lt, b)
}

fn number(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

/// Whether `c` is never true while columns 2 and 3 are null.
fn rejects(c: &Cond) -> bool {
    never_true(c, &|column| column >= 2)
}

/// Verifies: [ISO/IEC 9075:1992 §8.12 GR2] [DESIGN] RULES.md D1061: AND,
/// NOT and IS over single truth values give the truth tables' values, and
/// over sets of them each value some pair gives.
#[test]
fn truth_tables_hold_over_single_values_and_sets() {
    let and = [(T, T, T), (T, F, F), (T, U, U), (F, T, F), (F, F, F), (F, U, F), (U, T, U), (U, F, F), (U, U, U)];
    for (a, b, want) in and {
        assert_eq!(May::only(a).and(May::only(b)), May::only(want), "{a:?} AND {b:?}");
    }
    assert_eq!(May::only(T).not(), May::only(F));
    assert_eq!(May::only(U).not(), May::only(U));
    assert_eq!(May::ANY.and(May::only(F)), May::only(F));
    assert_eq!(May::ANY.and(May::only(U)), May { yes: false, no: true, unknown: true });
    assert_eq!(May::only(U).is(T, false), May::only(F));
    assert_eq!(May::only(U).is(T, true), May::only(T));
    assert_eq!(May::only(U).is(U, false), May::only(T));
    assert_eq!(May::ANY.is(F, false), May { yes: true, no: true, unknown: false });
}

/// Verifies: [X3.135-1986 §5.11 GR2, §5.9 GR1] [DESIGN] RULES.md D1061: a
/// comparison of a column of the null operand, or of arithmetic on one,
/// with any value is never true; of the other operand's columns alone it
/// may be; IS NULL of one is true, and IS NOT NULL never.
#[test]
fn comparisons_with_a_null_operand_s_column_are_never_true() {
    assert!(rejects(&less(column(0), column(2))));
    let scaled = Expr::Arith(Box::new(number(2)), ArithOp::Mul, Box::new(Expr::Neg(Box::new(column(3)))));
    assert!(rejects(&less(column(1), scaled)));
    assert!(!rejects(&less(column(0), column(1))));
    assert!(!rejects(&Cond::IsNull(column(2), false)));
    assert!(rejects(&Cond::IsNull(column(2), true)));
    assert!(!rejects(&Cond::IsNull(column(0), true)));
    assert!(rejects(&Cond::Like(column(2), column(0), None)));
    assert!(!rejects(&Cond::Distinct(column(0), column(2))), "null is distinct from a value");
}

/// Verifies: [ISO/IEC 9075:1992 §8.12 GR2] [DESIGN] RULES.md D1061: a
/// conjunction is never true if either side never is, a disjunction only
/// if both, and NOT of an unknown stays unknown.
///
/// Decision `a.yes && b.yes` of AND. MC/DC: (T T) may be true; (F T) and
/// (T F) never.
#[test]
fn connectives_bound_what_they_join() {
    let never = || less(column(0), column(2));
    let maybe = || less(column(0), column(1));
    let and = |a: Cond, b: Cond| Cond::And(Box::new(a), Box::new(b));
    let or = |a: Cond, b: Cond| Cond::Or(Box::new(a), Box::new(b));
    assert!(!rejects(&and(maybe(), maybe()))); // T T
    assert!(rejects(&and(never(), maybe()))); // F T
    assert!(rejects(&and(maybe(), never()))); // T F
    assert!(rejects(&or(never(), never())));
    assert!(!rejects(&or(never(), maybe())));
    assert!(rejects(&Cond::Not(Box::new(never()))));
    assert!(!rejects(&Cond::Is(Box::new(never()), U, false)), "IS UNKNOWN of an unknown is true");
    assert!(rejects(&Cond::Is(Box::new(never()), U, true)));
}

/// Verifies: [ISO/IEC 9075:1992 §6.9 GR2] [DESIGN] RULES.md D1061: a CASE
/// is null if the first WHEN that can only be true gives a null, as the
/// value an unnested subquery has of no row does (RULES.md D314); not if
/// it gives a value, as COUNT's 0; and of a WHEN that may or may not be
/// true only if its result and every later one are null.
#[test]
fn a_case_is_null_where_its_chosen_result_is() {
    let case = |whens: Vec<(Cond, Expr)>, otherwise: Expr| Expr::Case(whens.into(), Box::new(otherwise));
    let missing = || Cond::IsNull(column(2), false);
    let null = || Expr::Const(Value::Null);
    assert!(rejects(&less(column(0), case(vec![(missing(), null())], column(3)))));
    assert!(!rejects(&less(column(0), case(vec![(missing(), number(0))], column(3)))), "the value of COUNT over no row");
    let unsure = || less(column(0), column(1));
    assert!(rejects(&less(column(0), case(vec![(unsure(), column(2))], column(3)))), "each result null");
    assert!(!rejects(&less(column(0), case(vec![(unsure(), column(2))], number(1)))), "the ELSE a value");
    let passed = Cond::IsNull(column(2), true);
    assert!(rejects(&less(column(0), case(vec![(passed, number(1))], null()))), "a WHEN never true is passed over");
}
