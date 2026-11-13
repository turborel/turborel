//! [ISO/IEC 9075-2:2023 §8.15 GR1] Whether two values are distinct: the
//! test of the `<distinct predicate>`, which is never Unknown, and of the
//! keys of a join on IS NOT DISTINCT FROM, where a null matches a null.
//!
//! Each recursion is into an array's elements or a row's fields, so it is
//! as deep as types nest, bounded by `MAX_TYPE_NESTING` (RULES.md R-CF-6).

use std::cmp::Ordering;

use super::types::MAX_TYPE_NESTING;
use super::{Bucket, Value, bucket, compare};
use crate::error::{Code, Result, SqlError};

/// A value nested deeper than any type nests.
fn too_deep() -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] a value nested deeper than the {MAX_TYPE_NESTING} levels a type may"))
}

/// [ISO/IEC 9075-2:2023 §8.15 GR1] Whether `a` and `b` are distinct: two
/// nulls are not \[GR1a\]; a null and a value are \[GR1b\]; two arrays are
/// if their cardinalities differ or an element at one place is \[GR1c v\],
/// two rows if a pair of fields is \[GR1c iv\]; any other two if they are
/// not equal \[GR1c i, ii\].
///
/// # Errors
///
/// SQLCODE -910 if a value nests deeper than a type may.
pub fn distinct(a: &Value, b: &Value) -> Result<bool> {
    distinct_at(a, b, 0)
}

/// `distinct`, `depth` levels into arrays and rows.
fn distinct_at(a: &Value, b: &Value, depth: usize) -> Result<bool> {
    if depth > MAX_TYPE_NESTING {
        return Err(too_deep());
    }
    // [GR1a, GR1b] Of a null, distinct exactly when the other is no null.
    if a.is_null() || b.is_null() {
        return Ok(a.is_null() != b.is_null());
    }
    if let (Value::Array(x), Value::Array(y)) | (Value::Row(x), Value::Row(y)) = (a, b) {
        return parts_distinct(x, y, depth.saturating_add(1));
    }
    Ok(compare(a, b) != Some(Ordering::Equal))
}

/// [ISO/IEC 9075-2:2023 §8.15 GR1c iv, v] Whether the parts `x` and `y` of
/// two arrays or rows are distinct: of other counts, or a pair distinct.
fn parts_distinct(x: &[Value], y: &[Value], depth: usize) -> Result<bool> {
    if x.len() != y.len() {
        return Ok(true);
    }
    for (p, q) in x.iter().zip(y) {
        if distinct_at(p, q, depth)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The bucket of `v` as a join key: if `nulls`, a key on which a null
/// matches a null, as IS NOT DISTINCT FROM says, the null value's bucket
/// `Bucket::Null`, an array's or row's of its parts so; else `bucket`'s,
/// none for a null, which equals nothing [X3.135-1986 §5.11 GR2].
///
/// # Errors
///
/// SQLCODE -910 if a value nests deeper than a type may.
pub fn key_bucket(v: &Value, nulls: bool) -> Result<Option<Bucket>> {
    if nulls { null_bucket(v, 0) } else { Ok(bucket(v)) }
}

/// The bucket of `v` on which a null matches a null, `depth` levels into
/// arrays and rows.
fn null_bucket(v: &Value, depth: usize) -> Result<Option<Bucket>> {
    if depth > MAX_TYPE_NESTING {
        return Err(too_deep());
    }
    match v {
        Value::Null => Ok(Some(Bucket::Null)),
        Value::Array(parts) | Value::Row(parts) => {
            let mut buckets = Vec::with_capacity(parts.len());
            for p in parts.iter() {
                let Some(b) = null_bucket(p, depth.saturating_add(1))? else { return Ok(None) };
                buckets.push(b);
            }
            Ok(Some(Bucket::Composite(buckets.into())))
        }
        Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Extension(_) => Ok(bucket(v)),
    }
}

/// Whether `a` and `b` match as values of a join key: if `nulls`, not
/// distinct [ISO/IEC 9075-2:2023 §8.15]; else equal, a null equal to
/// nothing [X3.135-1986 §5.11 GR2].
///
/// # Errors
///
/// SQLCODE -910 if a value nests deeper than a type may.
pub fn key_equal(a: &Value, b: &Value, nulls: bool) -> Result<bool> {
    if nulls { Ok(!distinct(a, b)?) } else { Ok(compare(a, b) == Some(Ordering::Equal)) }
}

// Unit tests: each case of GR1, and the keys of a join on IS NOT DISTINCT
// FROM (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::value::Decimal;

    fn n(units: i128) -> Value {
        Value::Exact(Decimal::new(units, 0))
    }

    fn array(parts: Vec<Value>) -> Value {
        Value::Array(Arc::from(parts))
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.15 GR1a, GR1b, GR1c i] two nulls
    /// are not distinct, a null and a value are either way round, two
    /// values are if unequal, by the comparison rules: 1 and 1.0E0 are one
    /// value, 'a' and 'a ' too, as pad space compares them. MC/DC: the
    /// first null alone, the second alone, both, neither.
    #[test]
    fn values_are_distinct_8_15() {
        assert!(!distinct(&Value::Null, &Value::Null).unwrap(), "GR1a");
        assert!(distinct(&Value::Null, &n(1)).unwrap(), "GR1b, first null");
        assert!(distinct(&n(1), &Value::Null).unwrap(), "GR1b, second null");
        assert!(!distinct(&n(1), &Value::Approx(1.0_f64)).unwrap(), "GR1c i, equal");
        assert!(distinct(&n(1), &n(2)).unwrap(), "GR1c i, unequal");
        assert!(!distinct(&Value::Char("a".into()), &Value::Char("a ".into())).unwrap(), "pad space");
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.15 GR1c iv, v] arrays of other
    /// cardinalities are distinct, and of one if an element at one place
    /// is, a null element not distinct from a null; rows as their fields.
    /// MC/DC: the cardinality alone decides, then a pair of elements.
    #[test]
    fn composites_are_distinct_8_15() {
        assert!(distinct(&array(vec![n(1)]), &array(vec![n(1), n(2)])).unwrap(), "cardinality");
        assert!(!distinct(&array(vec![n(1), Value::Null]), &array(vec![n(1), Value::Null])).unwrap(), "null elements");
        assert!(distinct(&array(vec![n(1), Value::Null]), &array(vec![n(1), n(2)])).unwrap(), "a null and a value");
        assert!(!distinct(&array(vec![]), &array(vec![])).unwrap(), "empty");
        let row = |a, b| Value::Row(Arc::from(vec![a, b]));
        assert!(!distinct(&row(n(1), Value::Null), &row(n(1), Value::Null)).unwrap());
        assert!(distinct(&row(n(1), Value::Null), &row(Value::Null, Value::Null)).unwrap());
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.15] [X3.135-1986 §5.11 GR2] a key
    /// on which nulls match buckets a null, a null element too, and finds
    /// it equal to a null; a plain key buckets none and finds a null equal
    /// to nothing. Values equal by comparison share a bucket either way.
    #[test]
    fn join_keys_match_nulls_when_asked_8_15() {
        assert_eq!(key_bucket(&Value::Null, true).unwrap(), Some(Bucket::Null));
        assert_eq!(key_bucket(&Value::Null, false).unwrap(), None);
        let with_null = array(vec![n(1), Value::Null]);
        assert_eq!(key_bucket(&with_null, true).unwrap(), key_bucket(&array(vec![n(1), Value::Null]), true).unwrap());
        assert_eq!(key_bucket(&with_null, false).unwrap(), None);
        assert_eq!(key_bucket(&n(1), true).unwrap(), key_bucket(&Value::Approx(1.0_f64), true).unwrap());
        assert!(key_equal(&Value::Null, &Value::Null, true).unwrap());
        assert!(!key_equal(&Value::Null, &Value::Null, false).unwrap());
        assert!(!key_equal(&Value::Null, &n(1), true).unwrap());
        assert!(key_equal(&n(1), &n(1), false).unwrap());
    }
}
