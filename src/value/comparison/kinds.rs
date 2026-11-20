//! Unit tests of comparisons across kinds of value (`comparison.rs`):
//! a large object or a plug-in's value on the right, a truth value with
//! another kind, an infinite double, and values nested past a type's
//! depth.

use std::sync::Arc;

use super::*;

fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.2] [DESIGN] RULES.md D68, a
/// comparison reads the same in either order: a string and a character
/// large object held inline compare as its kind says, reversed of the
/// object on the right; an FT_Pattern value on the right compares by its
/// kind, reversed; a truth value with a number, or a number with a truth
/// value, is not comparable [§4.6.2].
#[test]
fn either_order_compares_alike() {
    let lob = Value::Lob(Lob::of_text("b"));
    assert_eq!(compare(&lob, &text("a")), Some(Ordering::Greater));
    assert_eq!(compare(&text("a"), &lob), Some(Ordering::Less));
    let (x, y) = (crate::fulltext::types::ft_pattern("x").unwrap(), crate::fulltext::types::ft_pattern("y").unwrap());
    assert_eq!(compare(&x, &y), Some(Ordering::Less));
    assert_eq!(compare(&y, &x), Some(Ordering::Greater));
    assert_eq!(compare(&text("x"), &x), None);
    let one = Value::Exact(Decimal::new(1, 0));
    assert_eq!(compare(&Value::Bool(true), &one), None);
    assert_eq!(compare(&one, &Value::Bool(true)), None);
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.2 GR3] a DECFLOAT compares with an
/// infinite double as every finite number does: less than positive
/// infinity, greater than negative infinity.
#[test]
fn a_decfloat_is_between_the_infinities() {
    let d = Value::DecFloat(DecFloat::new(5, 0, 34).unwrap());
    assert_eq!(compare(&d, &Value::Approx(f64::INFINITY)), Some(Ordering::Less));
    assert_eq!(compare(&d, &Value::Approx(f64::NEG_INFINITY)), Some(Ordering::Greater));
}

/// Verifies: [DESIGN] RULES.md R-CF-6, a value nested deeper than a type
/// may (16 levels) is refused with SQLCODE -910 as it is searched for a
/// large object, alone or as a part; one as deep as a type may is
/// searched, a large object found in it.
#[test]
fn values_nested_past_a_types_depth_are_refused() {
    let nest = |depth: usize, inner: Value| (0..depth).fold(inner, |v, _| Value::Array(Arc::from(vec![v])));
    let deep = nest(MAX_TYPE_NESTING.saturating_add(2), text("a"));
    assert_eq!(any_lob(std::slice::from_ref(&deep)).unwrap_err().code, Code::Api);
    let row = Value::Row(Arc::from(vec![text("b"), deep.clone()]));
    assert_eq!(any_lob(&[row]).unwrap_err().code, Code::Api);
    assert_eq!(compare_checked(&deep, &deep).unwrap_err().code, Code::Api);
    assert!(any_lob(&[nest(MAX_TYPE_NESTING, Value::Lob(Lob::of_text("c")))]).unwrap());
}
