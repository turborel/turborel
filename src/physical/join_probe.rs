//! \[DESIGN\] A join's index probe bound to one outer row. Equality
//! prefixes and independent comparisons share SQL NULL handling; neither
//! can match when one of its compared outer values is NULL.

use crate::error::{Code, Result, SqlError};
use crate::index::{ColumnComparison, Probe};
use crate::value::{CompOp, Value};

/// A comparison of an index key column with an outer row column.
#[derive(Debug, Clone)]
pub struct JoinComparison {
    /// Position within the index key, from zero.
    pub column: usize,
    /// The index coordinate is on the left of this operator.
    pub op: CompOp,
    /// Position within the outer row, from zero.
    pub outer: usize,
}

/// The values each outer row supplies to an index nested-loop join.
#[derive(Debug, Clone)]
pub enum JoinProbe {
    /// Outer columns equated with successive index key columns.
    Prefix(Box<[usize]>),
    /// Independent comparisons, all of which must hold.
    Columns(Box<[JoinComparison]>),
}

impl JoinProbe {
    /// Binds this probe to `row`, for an index of `width` columns. None
    /// means a compared value is NULL, so the probe cannot match.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a referenced outer column is missing.
    pub fn bind(&self, row: &[Value], width: usize) -> Result<Option<Probe>> {
        let value =
            |at: usize| row.get(at).cloned().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] an index join's outer column is missing"));
        match self {
            JoinProbe::Prefix(keys) => {
                let values = keys.iter().map(|&k| value(k)).collect::<Result<Vec<_>>>()?;
                if values.iter().any(Value::is_null) {
                    return Ok(None);
                }
                let probe = match (values.len(), width) {
                    (1, 1) => Probe::Compare { op: CompOp::Eq, value: values.into_iter().next().unwrap_or(Value::Null) },
                    _ => Probe::Prefix { equal: values, then: None },
                };
                Ok(Some(probe))
            }
            JoinProbe::Columns(each) => {
                let each = each.iter().map(|c| Ok(ColumnComparison { column: c.column, op: c.op, value: value(c.outer)? }));
                let each = each.collect::<Result<Vec<_>>>()?;
                let has_null = each.iter().any(|c| c.value.is_null());
                Ok((!has_null).then_some(Probe::Columns(each)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    fn n(value: i128) -> Value {
        Value::Exact(Decimal::new(value, 0))
    }

    /// Verifies: [DESIGN] Each join probe binds the requested outer
    /// position, including repeated bounds on one key column. Either
    /// compared NULL alone prevents a match; an unrelated NULL does not.
    /// Missing outer columns report -910 for both probe kinds.
    #[test]
    fn joining_binds_positions_and_preserves_null_semantics() {
        let window = JoinProbe::Columns(
            vec![JoinComparison { column: 0, op: CompOp::Ge, outer: 1 }, JoinComparison { column: 0, op: CompOp::Le, outer: 2 }]
                .into_boxed_slice(),
        );
        let probe = window.bind(&[Value::Null, n(3), n(5)], 2).unwrap().unwrap();
        assert!(probe.answers(&[n(4), n(0)]));
        assert!(!probe.answers(&[n(2), n(0)]));
        assert!(!probe.answers(&[n(6), n(0)]));
        for row in [[n(0), Value::Null, n(5)], [n(0), n(3), Value::Null]] {
            assert!(window.bind(&row, 2).unwrap().is_none());
        }
        let prefix = JoinProbe::Prefix(vec![1].into_boxed_slice());
        assert!(prefix.bind(&[n(0), Value::Null], 1).unwrap().is_none());
        let one = prefix.bind(&[n(0), n(3)], 1).unwrap().unwrap();
        assert!(matches!(one, Probe::Compare { .. }));
        let many = prefix.bind(&[n(0), n(3)], 2).unwrap().unwrap();
        assert!(matches!(many, Probe::Prefix { .. }));
        for binding in [window, prefix] {
            assert_eq!(binding.bind(&[n(1)], 2).unwrap_err().code, Code::Api);
        }
    }
}
