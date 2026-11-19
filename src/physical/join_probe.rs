//! \[DESIGN\] A join's index probe bound to one outer row. Equality
//! prefixes, independent comparisons and NULL-rejecting function probes
//! cannot match when one of their compared outer values is NULL. A
//! spatial window's probe reads values beside its geometry, a relate's
//! pattern or a distance's limit and unit, as a ROW (`spatial::window`).

use std::sync::Arc;

use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError};
use crate::index::{ColumnComparison, Predicate, Probe};
use crate::logical::Cond;
use crate::value::{CompOp, Value};

/// A value a spatial window's probe reads after the outer geometry.
#[derive(Debug, Clone)]
pub enum Supplied {
    /// A constant: a relate's pattern, a distance's limit or unit scale.
    Constant(Value),
    /// A column of the outer row, a distance's limit.
    Outer(usize),
}

/// The values a spatial window's probe reads after the outer geometry,
/// boxed apart so a probe of none stays as small as one was (D135).
#[derive(Debug, Clone)]
pub struct Fields(pub Box<[Supplied]>);

/// The rows an index join keeps that match none (`Plan::IndexJoin`).
#[derive(Debug, Clone)]
pub enum Preserved {
    /// None: an inner join.
    Inner,
    /// Each outer row no table row matches, with nulls: a LEFT outer join
    /// [ISO/IEC 9075:1992 §7.5].
    Outer,
    /// Those, then each table row no outer row matched: a FULL outer join.
    Both(Box<Unmatched>),
}

/// Of a FULL outer index join, which table rows the probes left are kept:
/// those its own filter is true of, over the pair's columns, after nulls
/// for the outer row's `width` columns.
#[derive(Debug, Clone)]
pub struct Unmatched {
    /// The table's filter, over the pair's columns.
    pub filter: Option<Cond>,
    /// The outer operand's degree.
    pub width: usize,
}

/// How a spatial index join may read its tables beyond a probe for each
/// outer row (D85, D138), boxed apart so a join of probes alone stays as
/// small as one was. Either is admitted only for two columns of one SRID
/// under a windowed relationship, so the join raises no error of its own.
#[derive(Debug, Clone)]
pub enum Spatial {
    /// Of an unordered inner join, probes, or a scan of the table matched
    /// with the outer rows held, in memory or in partitions of the spill
    /// area (`exec/join/indexed/adaptive.rs`).
    Adaptive,
    /// A traversal of the outer table's R-tree and the probed table's
    /// together, pairing the entries whose envelopes meet; probes if
    /// either tree cannot be read so (`exec/join/indexed/paired.rs`).
    Paired(Box<Paired>),
}

/// The outer table of a synchronized traversal: the table its operand
/// reads, the R-tree on its geometry column, the operand's filter over
/// the table's columns, the table's column each of the operand's is if
/// it reads them through a projection, the columns the query reads, and
/// whether each row is followed by its id (`LogicalPlan::Scan`).
#[derive(Debug, Clone)]
pub struct Paired {
    /// The table.
    pub table: QualifiedName,
    /// Its R-tree.
    pub index: Box<str>,
    /// The operand's filter, which cannot raise.
    pub filter: Option<Cond>,
    /// Of an operand reading the table through a projection of its
    /// columns, the table's column each of the operand's is.
    pub through: Option<Vec<usize>>,
    /// The table's columns the query reads.
    pub columns: Vec<usize>,
    /// Whether each row is followed by its id.
    pub located: bool,
}

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
    /// A supported, NULL-rejecting function predicate, with its other
    /// argument supplied by the outer row. Planning proves that pruning
    /// preserves errors and retains any required exact recheck.
    Predicate {
        /// The function and the indexed column's argument position.
        predicate: Box<Predicate>,
        /// Position within the outer row, from zero.
        outer: usize,
        /// What a window reads beside the geometry, none of most calls.
        beside: Option<Box<Fields>>,
    },
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
            JoinProbe::Predicate { predicate, outer, beside } => {
                let geometry = value(*outer)?;
                if geometry.is_null() {
                    return Ok(None);
                }
                // A null beside it is the index's to judge: a distance of a
                // null limit may still raise, so every entry may be read.
                let value = if let Some(beside) = beside {
                    let read = |s: &Supplied| match s {
                        Supplied::Constant(v) => Ok(v.clone()),
                        Supplied::Outer(at) => value(*at),
                    };
                    let fields = std::iter::once(Ok(geometry)).chain(beside.0.iter().map(read)).collect::<Result<Vec<_>>>()?;
                    Value::Row(Arc::from(fields))
                } else {
                    geometry
                };
                Ok(Some(Probe::Predicate { predicate: predicate.clone(), value }))
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

    /// Verifies: [DESIGN] D85: a spatial join binds the chosen outer
    /// geometry without changing either argument position. An unrelated
    /// NULL is harmless, a NULL operand rejects the pair, and a missing
    /// outer column reports the same -910 as comparison probes.
    #[test]
    fn spatial_join_binding_keeps_arguments_and_null_semantics() {
        use crate::functions::ScalarRef;
        use crate::geometry::{Geometry, GeometryFunction};
        let point = Geometry::point(1.0, 2.0, 4326).unwrap().to_value();
        for argument in [0_usize, 1] {
            let predicate = Predicate { function: ScalarRef(Arc::new(GeometryFunction::Intersects)), column: 0, argument: Some(argument) };
            let binding = JoinProbe::Predicate { predicate: Box::new(predicate), outer: 1, beside: None };
            let probe = binding.bind(&[Value::Null, point.clone()], 1).unwrap().unwrap();
            let Probe::Predicate { predicate: bound, value } = probe else { panic!("lost spatial predicate") };
            assert_eq!(bound.argument, Some(argument));
            assert_eq!(bound.column, 0);
            assert_eq!(crate::value::compare(&value, &point), Some(std::cmp::Ordering::Equal));
            assert!(binding.bind(&[point.clone(), Value::Null], 1).unwrap().is_none());
            assert_eq!(binding.bind(std::slice::from_ref(&point), 1).unwrap_err().code, Code::Api);
        }
    }

    /// Verifies: [DESIGN] D135: a window's probe reads its constants and
    /// outer columns after the geometry, as a ROW; a null geometry probes
    /// nothing, a null beside it is left to the index, and a missing outer
    /// column reports -910.
    #[test]
    fn spatial_windows_bind_what_they_read_beside_the_geometry() {
        use crate::functions::ScalarRef;
        use crate::geometry::{Geometry, GeometryFunction};
        let point = Geometry::point(1.0, 2.0, 0).unwrap().to_value();
        let predicate = Predicate { function: ScalarRef(Arc::new(GeometryFunction::Distance)), column: 0, argument: Some(1) };
        let beside = Some(Box::new(Fields(Box::new([Supplied::Outer(2), Supplied::Constant(Value::Approx(1.0))]))));
        let binding = JoinProbe::Predicate { predicate: Box::new(predicate), outer: 0, beside };
        for limit in [n(3), Value::Null] {
            let Some(Probe::Predicate { value: Value::Row(fields), .. }) = binding.bind(&[point.clone(), n(0), limit.clone()], 1).unwrap()
            else {
                panic!("no window")
            };
            assert_eq!(fields.len(), 3);
            assert_eq!(fields[2].to_string(), "1E0");
            assert_eq!(fields[1].is_null(), limit.is_null());
        }
        assert!(binding.bind(&[Value::Null, n(0), n(3)], 1).unwrap().is_none());
        assert_eq!(binding.bind(&[point, n(0)], 1).unwrap_err().code, Code::Api);
    }
}
