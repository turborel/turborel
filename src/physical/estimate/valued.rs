//! \[DESIGN\] The values of the slots of a plan made with them (RULES.md
//! D613). A prepared statement's plan with its slots is chosen by one
//! execution's values, and is kept only once it agrees with the plan made
//! with them written in (D45). Every estimate made while it is planned
//! reads a slot as its value, so the two plans are costed alike: the
//! values are the thread's while it is planned, put back as they were
//! after, so they are no global state (R-SAFE-2).

use std::cell::RefCell;
use std::sync::Arc;

use crate::logical::{Cond, filled_cond};
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
/// and they fill it.
pub(super) fn filled(cond: &Cond) -> Option<Cond> {
    let values = VALUES.with(|v| v.borrow().clone())?;
    filled_cond(cond.clone(), &values).ok()
}

// Unit tests: the values are the thread's only while `valued` runs
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::logical::{Expr, Slot};
    use crate::value::{CompOp, DataType, Decimal, Spelled};

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
}
