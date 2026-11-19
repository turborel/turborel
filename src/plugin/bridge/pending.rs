//! \[EXTENSION\] The plug-in points of work on other branches, mirrored
//! here as their designs give them, so the adapters of `backup.rs` and
//! `partition.rs` are written whole against them: backups' `BackupTarget`
//! and `TargetProvider` (D160-D166), and partitioning's
//! `PartitionStrategy` with its bounds and restrictions (D154). When those
//! branches are merged, this file gives way to their own traits and the
//! adapters implement those, unchanged but for their paths.

use std::sync::Arc;

use crate::error::Result;
use crate::value::{DataType, Value};

/// Where backups and archived log go: a flat store of objects.
pub trait BackupTarget: Send + Sync {
    /// Its location, as named in SQL and settings.
    fn location(&self) -> String;
    /// Begins writing object `name`, giving an upload's handle.
    ///
    /// # Errors
    ///
    /// The target's.
    fn begin(&self, name: &str) -> Result<u64>;
    /// Appends `bytes` to upload `upload`.
    ///
    /// # Errors
    ///
    /// The target's.
    fn write(&self, upload: u64, bytes: &[u8]) -> Result<()>;
    /// Makes the upload its object, durably.
    ///
    /// # Errors
    ///
    /// The target's.
    fn finish(&self, upload: u64) -> Result<()>;
    /// Abandons an upload, leaving no object.
    ///
    /// # Errors
    ///
    /// The target's.
    fn abandon(&self, upload: u64) -> Result<()>;
    /// Up to `max` bytes of `name` from `offset`, empty at its end.
    ///
    /// # Errors
    ///
    /// The target's.
    fn read(&self, name: &str, offset: u64, max: u32) -> Result<Vec<u8>>;
    /// The names of the objects beginning with `prefix`, sorted.
    ///
    /// # Errors
    ///
    /// The target's.
    fn list(&self, prefix: &str) -> Result<Vec<String>>;
    /// Removes object `name`, if it exists.
    ///
    /// # Errors
    ///
    /// The target's.
    fn remove(&self, name: &str) -> Result<()>;
}

/// Opens targets of one scheme from a location.
pub trait TargetProvider: Send + Sync {
    /// The scheme, before the colon of a location.
    fn scheme(&self) -> &str;
    /// The target at `location`.
    ///
    /// # Errors
    ///
    /// The provider's, of a location it cannot open.
    fn open(&self, location: &str) -> Result<Arc<dyn BackupTarget>>;
}

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
