//! \[DESIGN\] The values of the slots of a plan made with them (RULES.md
//! D613). A prepared statement's plan with its slots is chosen by one
//! execution's values, and is kept only once it agrees with the plan made
//! with them written in (D45). Every estimate made while it is planned
//! reads a slot as its value, so the two plans are costed alike: the
//! values are the thread's while it is planned, put back as they were
//! after, so they are no global state (R-SAFE-2).

use std::cell::RefCell;
use std::sync::Arc;

use crate::exec::{NoSubqueries, eval};
use crate::logical::{Cond, Expr, filled_cond};
use crate::value::Value;

std::thread_local! {
    static VALUES: RefCell<Option<Arc<[Value]>>> = const { RefCell::new(None) };
}

/// Puts back the values the thread had (`valued`).
struct Restored(Option<Arc<[Value]>>);

impl Drop for Restored {
    fn drop(&mut self) {
        let was = self.0.take();
        VALUES.with(|v| *v.borrow_mut() = was);
    }
}

/// `f`, run with `values` the thread's slots' values, if given.
pub(in crate::physical) fn valued<T>(values: Option<&[Value]>, f: impl FnOnce() -> T) -> T {
    let Some(values) = values else { return f() };
    let was = VALUES.with(|v| v.borrow_mut().replace(values.into()));
    let _restored = Restored(was);
    f()
}

/// `cond`, its slots filled with the thread's values, if it has values
/// and they fill it (`estimated`).
pub(in crate::physical) fn filled(cond: &Cond) -> Option<Cond> {
    let values = VALUES.with(|v| v.borrow().clone())?;
    estimated(cond, &values)
}

/// \[DESIGN\] `cond` as an estimate reads it with `values`: each slot its
/// value, and each compared sum or difference of constants alone, as a
/// slot less a constant becomes, folded to its value, as the plan made
/// with the values written in has it folded (RULES.md D760). A bound the
/// estimate cannot read as a constant takes the default share of a
/// range, so the plan made with slots would be costed by other rows than
/// the plan it is to agree with. None if the values do not fill it.
pub(in crate::physical) fn estimated(cond: &Cond, values: &[Value]) -> Option<Cond> {
    let filled = filled_cond(cond.clone(), values).ok()?;
    Some(filled.map_exprs(&mut folded))
}

/// `e`, arithmetic of constants, as its value; else itself, as is one
/// whose evaluation raises an error, which the execution then raises.
fn folded(e: Expr) -> Expr {
    if !matches!(e, Expr::Arith(..)) {
        return e;
    }
    match eval(&e, &Vec::new(), &NoSubqueries) {
        Ok(value) => Expr::Const(value),
        Err(_) => e,
    }
}

// Unit tests: the values are the thread's only while `valued` runs
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::logical::Slot;
    use crate::value::{ArithOp, CompOp, DataType, Decimal, Spelled};

    /// Verifies: [DESIGN] RULES.md D613: a slot is filled while the values
    /// are the thread's, and not after, nor on a thread given none.
    #[test]
    fn a_slot_reads_its_value_only_while_its_plan_is_made() {
        let slot = Slot { index: 0, data_type: DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }, at: 0 };
        let cond = Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Dynamic(slot));
        let seven = [Value::Exact(Decimal::new(7, 0))];
        let inside = valued(Some(&seven), || filled(&cond));
        assert!(matches!(inside, Some(Cond::Compare(_, _, Expr::Const(_)))), "{inside:?}");
        assert!(filled(&cond).is_none(), "put back");
        assert!(valued(None, || filled(&cond)).is_none(), "none given");
    }

    /// Verifies: [DESIGN] RULES.md D760: a slot less a constant is read as
    /// the constant its value less the constant is; a sum with a column,
    /// and one that raises an error, are not folded.
    #[test]
    fn a_slot_less_a_constant_is_estimated_as_its_value() {
        let int = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
        let slot = Expr::Dynamic(Slot { index: 0, data_type: int, at: 0 });
        let num = |n: i128| Expr::Const(Value::Exact(Decimal::new(n, 0)));
        let less = |a: Expr, b: Expr| Expr::Arith(Box::new(a), ArithOp::Sub, Box::new(b));
        let compared = |e: Expr| Cond::Compare(Expr::Column(0), CompOp::Ge, e);
        let got = estimated(&compared(less(slot.clone(), num(20))), &[Value::Exact(Decimal::new(3000, 0))]);
        assert!(matches!(&got, Some(Cond::Compare(_, _, Expr::Const(Value::Exact(d)))) if d.to_string() == "2980"), "{got:?}");
        let column = compared(less(Expr::Column(1), num(20)));
        assert!(matches!(estimated(&column, &[]), Some(Cond::Compare(_, _, Expr::Arith(..)))), "a column's");
        let zero = Expr::Arith(Box::new(num(1)), ArithOp::Div, Box::new(num(0)));
        assert!(matches!(estimated(&compared(zero), &[]), Some(Cond::Compare(_, _, Expr::Arith(..)))), "raises");
    }
}
