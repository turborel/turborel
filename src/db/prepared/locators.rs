//! [ISO/IEC 9075-2:2023 §4.37.5] Which of the values an execution of a
//! prepared statement is given may hold a large object sent by its
//! locator (RULES.md D941).
//!
//! A client sends a large object by its locator, and the execution reads
//! the value it names in its place (`db/lobs.rs`, `Session::unlocated`).
//! That reading copies every value. An execution none of whose values is
//! a large object, or holds one, has nothing to read, so its values are
//! taken as they are.

use crate::value::Value;

/// Whether `v` may hold a large object a client sent by its locator: a
/// large object, or a value that holds values. Any other value is the one
/// `Session::unlocated` gives of it, so it is not looked at there
/// (RULES.md D941).
pub(super) fn may_hold(v: &Value) -> bool {
    match v {
        Value::Lob(_) | Value::Structured(_) => true,
        Value::Array(held) | Value::Multiset(held) | Value::Row(held) => held.iter().any(may_hold),
        Value::Null
        | Value::Reference(_)
        | Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Json(_)
        | Value::Extension(_)
        | Value::Xml(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::may_hold;
    use crate::value::lob::Lob;
    use crate::value::{Decimal, Value};

    /// Verifies: [DESIGN] RULES.md D941: a large object, and an array, a
    /// multiset or a row that holds one at any depth, may hold a locator;
    /// a number, a string, and an array of them hold none. MC/DC of the
    /// decision: a large object itself, one held, none held.
    #[test]
    fn only_a_large_object_or_what_holds_one_may_hold_a_locator() {
        let lob = Value::Lob(Lob::of_text("x"));
        let number = Value::Exact(Decimal::new(1, 0));
        assert!(may_hold(&lob));
        assert!(!may_hold(&number));
        assert!(!may_hold(&Value::Char("a".to_owned())));
        assert!(!may_hold(&Value::Null));
        let plain: Arc<[Value]> = vec![number.clone(), Value::Null].into();
        assert!(!may_hold(&Value::Array(Arc::clone(&plain))));
        assert!(!may_hold(&Value::Multiset(Arc::clone(&plain))));
        let holding: Arc<[Value]> = vec![number, Value::Array(vec![lob].into())].into();
        assert!(may_hold(&Value::Array(Arc::clone(&holding))));
        assert!(may_hold(&Value::Row(holding)));
    }
}
