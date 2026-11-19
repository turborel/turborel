//! \[EXTENSION\] The plug-in point of work on another branch, mirrored
//! here as its design gives it, so the adapter of `partition.rs` is
//! written whole against it: partitioning's `PartitionStrategy` with its
//! bounds and restrictions (D154). When that branch is merged, this file
//! gives way to its own trait and the adapter implements that, unchanged
//! but for its paths.

use crate::error::Result;
use crate::value::{DataType, Value};

/// A partition's bound.
#[derive(Debug, Clone)]
pub enum PartitionBound {
    /// DEFAULT, holding what no sibling does.
    Default,
    /// FROM .. TO, the upper end left out.
    Range {
        /// The lower end.
        from: Vec<RangeDatum>,
        /// The upper end.
        to: Vec<RangeDatum>,
    },
    /// IN, of keys.
    List(Vec<Vec<Value>>),
    /// WITH, of named values, as HASH's MODULUS and REMAINDER.
    With(Vec<(String, Value)>),
}

/// An end of a range.
#[derive(Debug, Clone)]
pub enum RangeDatum {
    /// A value.
    Value(Value),
    /// Below every value.
    MinValue,
    /// Above every value.
    MaxValue,
}

/// What a read's condition says of one key part.
#[derive(Debug, Clone)]
pub enum Restriction {
    /// Equal to one of these.
    Equal(Vec<Value>),
    /// Between these ends, each inclusive or not.
    Range {
        /// The lower end.
        low: Option<(Value, bool)>,
        /// The upper end.
        high: Option<(Value, bool)>,
    },
    /// IS NULL.
    Null,
}

/// How a partitioned table's rows are divided among its partitions.
pub trait PartitionStrategy: Send + Sync + std::fmt::Debug {
    /// The name PARTITION BY gives.
    fn name(&self) -> &str;
    /// Whether a key of these types can be divided this way.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2A000 with the rule broken.
    fn check_key(&self, types: &[DataType]) -> Result<()>;
    /// The bound a FOR VALUES clause gives, checked and normalized.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2A000 of a form or value it does not take.
    fn bound(&self, types: &[DataType], spec: &PartitionBound) -> Result<PartitionBound>;
    /// Whether two siblings' bounds could hold one key.
    ///
    /// # Errors
    ///
    /// The strategy's.
    fn overlaps(&self, a: &PartitionBound, b: &PartitionBound) -> Result<bool>;
    /// Whether a row of `key` belongs to `bound`.
    ///
    /// # Errors
    ///
    /// The strategy's.
    fn contains(&self, bound: &PartitionBound, key: &[Value]) -> Result<bool>;
    /// The sibling holding `key`, if any. Default: the first that
    /// `contains` it.
    ///
    /// # Errors
    ///
    /// The strategy's.
    fn route(&self, bounds: &[&PartitionBound], key: &[Value]) -> Result<Option<usize>> {
        for (at, bound) in bounds.iter().enumerate() {
            if self.contains(bound, key)? {
                return Ok(Some(at));
            }
        }
        Ok(None)
    }
    /// Whether a partition of `bound` may hold a row meeting every
    /// restriction, true when unsure.
    ///
    /// # Errors
    ///
    /// The strategy's.
    fn admits(&self, bound: &PartitionBound, restrictions: &[Option<Restriction>]) -> Result<bool>;
}
