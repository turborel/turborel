//! Unit tests of whether an expression or condition can raise (`expr.rs`).

use super::*;
use crate::value::Decimal;

fn column() -> Expr {
    Expr::Column(0)
}

fn quotient() -> Expr {
    Expr::Arith(Box::new(Expr::Const(Value::Exact(Decimal::new(1, 0)))), ArithOp::Div, Box::new(column()))
}

fn compare(left: Expr, right: Expr) -> Cond {
    Cond::Compare(left, CompOp::Gt, right)
}

/// Verifies: [DESIGN] arithmetic can raise ([X3.135-1986 §5.9 GR5]); columns,
/// constants and the negation of either cannot; CASE through its parts.
#[test]
fn expressions_that_can_raise() {
    assert!(!column().can_raise());
    assert!(!Expr::Neg(Box::new(column())).can_raise());
    assert!(quotient().can_raise());
    assert!(Expr::Neg(Box::new(quotient())).can_raise());
    let case = |c: Cond, e: Expr| Expr::Case(vec![(c, e)].into(), Box::new(column()));
    assert!(!case(compare(column(), column()), column()).can_raise());
    assert!(case(compare(quotient(), column()), column()).can_raise());
    assert!(case(compare(column(), column()), quotient()).can_raise());
    assert!(Expr::Cast(Box::new(column()), DataType::character(1)).can_raise());
}

/// Verifies: [DESIGN] a condition can raise exactly when one of its operands can.
///
/// Decision `left.can_raise() || right.can_raise()` in `Compare`, `And` and `Or`.
/// MC/DC: (F F) false; (T F) true shows `left`; (F T) true shows `right`.
#[test]
fn conditions_raise_through_their_operands() {
    assert!(!compare(column(), column()).can_raise()); // F F
    assert!(compare(quotient(), column()).can_raise()); // T F
    assert!(compare(column(), quotient()).can_raise()); // F T
    let safe = || compare(column(), column());
    let risky = || compare(quotient(), column());
    assert!(!Cond::And(Box::new(safe()), Box::new(safe())).can_raise()); // F F
    assert!(Cond::And(Box::new(risky()), Box::new(safe())).can_raise()); // T F
    assert!(Cond::And(Box::new(safe()), Box::new(risky())).can_raise()); // F T
    assert!(!Cond::Or(Box::new(safe()), Box::new(safe())).can_raise()); // F F
    assert!(Cond::Or(Box::new(risky()), Box::new(safe())).can_raise()); // T F
    assert!(Cond::Or(Box::new(safe()), Box::new(risky())).can_raise()); // F T
    assert!(Cond::Not(Box::new(risky())).can_raise());
    assert!(!Cond::IsNull(Expr::Column(0), false).can_raise());
    assert!(Cond::Is(Box::new(risky()), Truth::True, false).can_raise());
}
