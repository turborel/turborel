//! \[EXTENSION\] Partition strategies (RULES.md D154): how a partitioned
//! table's rows are divided among its partitions. A strategy is a plug-in
//! registered by name on the database, as an index method is; RANGE, LIST
//! and HASH are built in (`partition/range.rs`, `partition/list.rs`,
//! `partition/hash.rs`). The framework around them routes rows
//! (`db/partitions/write.rs`) and prunes reads (`partition/prune.rs`),
//! and owns the DEFAULT partition, which no strategy sees.

pub mod hash;
mod list;
pub(crate) mod prune;
mod range;

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use crate::catalog::PartitionBound;
use crate::error::{Code, Result, SqlError};
use crate::value::{DataType, Value, assign, compare};

pub use hash::HashStrategy;
pub use list::ListStrategy;
pub use range::RangeStrategy;

// The built-in strategies' names, as PARTITION BY gives them.

/// PARTITION BY RANGE.
pub const RANGE: &str = "RANGE";
/// PARTITION BY LIST.
pub const LIST: &str = "LIST";
/// PARTITION BY HASH.
pub const HASH: &str = "HASH";
/// HASH's bound option giving the modulus.
pub const MODULUS: &str = "MODULUS";
/// HASH's bound option giving the remainder.
pub const REMAINDER: &str = "REMAINDER";

/// What a read's condition says of one key part, as the pruner finds it
/// in the condition's conjuncts (`partition/prune.rs`). A part none
/// restricts is given as none.
#[derive(Debug, Clone)]
pub enum Restriction {
    /// The part equals one of these values: `=` or IN.
    Equal(Vec<Value>),
    /// The part lies in this range, each end a value and whether it is
    /// included; an end not given is unbounded.
    Range {
        /// The lower end.
        low: Option<(Value, bool)>,
        /// The upper end.
        high: Option<(Value, bool)>,
    },
    /// The part is null.
    Null,
}

/// \[EXTENSION\] How a partitioned table's rows are divided among its
/// partitions (RULES.md D154). Registered on the database by its name
/// (`Database::register_partition_strategy`), which PARTITION BY gives.
///
/// Contract (RULES.md R-SAFE-3; checked of every built-in strategy by
/// `test/integration/partitioning/contract.rs`):
/// * `bound` accepts only the forms it reads, never `Default`, and gives
///   a bound every other method takes;
/// * `contains` of a key is true for at most one of siblings whose bounds
///   do not `overlaps`, and `route` gives that one;
/// * `admits` is true of every bound that `contains` a key meeting every
///   restriction: it may be true of others, never false of one of these;
/// * every method is deterministic, the same on any machine, as stored
///   rows depend on it.
pub trait PartitionStrategy: Send + Sync + fmt::Debug {
    /// Its name, an upper-case `<identifier>`: what PARTITION BY gives.
    fn name(&self) -> &str;

    /// That a key of parts of `types` can be divided this way.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 naming the part it cannot divide by.
    fn check_key(&self, types: &[DataType]) -> Result<()>;

    /// The bound a FOR VALUES clause gave, of a key of `types`, checked
    /// and its values assigned to the parts' types.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 of a form it does not read, or of values it cannot
    /// take; an assignment's errors.
    fn bound(&self, types: &[DataType], spec: PartitionBound) -> Result<PartitionBound>;

    /// Whether two siblings' bounds, each as `bound` gave it, could both
    /// hold one key.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the two cannot be siblings.
    fn overlaps(&self, a: &PartitionBound, b: &PartitionBound) -> Result<bool>;

    /// Whether a row of key `key` belongs to `bound`.
    ///
    /// # Errors
    ///
    /// Only a value it cannot read, SQLCODE -910.
    fn contains(&self, bound: &PartitionBound, key: &[Value]) -> Result<bool>;

    /// Of siblings' bounds, the one holding `key`, if any: by default the
    /// first that `contains` it.
    ///
    /// # Errors
    ///
    /// As `contains`.
    fn route(&self, bounds: &[&PartitionBound], key: &[Value]) -> Result<Option<usize>> {
        for (at, bound) in bounds.iter().enumerate() {
            if self.contains(bound, key)? {
                return Ok(Some(at));
            }
        }
        Ok(None)
    }

    /// Whether a partition of `bound` may hold a row meeting every
    /// restriction, one for each key part, none of a part unrestricted;
    /// true when it cannot tell.
    ///
    /// # Errors
    ///
    /// As `contains`.
    fn admits(&self, bound: &PartitionBound, restrictions: &[Option<Restriction>]) -> Result<bool>;

    /// Whether a DEFAULT partition may be a sibling of its partitions:
    /// by default, yes.
    fn takes_default(&self) -> bool {
        true
    }
}

/// The built-in strategies: RANGE, LIST and HASH.
pub fn builtin() -> Vec<Arc<dyn PartitionStrategy>> {
    vec![Arc::new(RangeStrategy), Arc::new(ListStrategy), Arc::new(HashStrategy)]
}

/// A refusal of a definition's rule.
pub(crate) fn refused<T>(msg: impl Into<String>) -> Result<T> {
    Err(SqlError::new(Code::SyntaxRule, msg))
}

/// Whether a key part of type `t` can be compared, hashed and kept in a
/// bound: of a scalar type the standard defines, compared as values are
/// [X3.135-1986 §5.11].
pub(crate) fn scalar(t: &DataType) -> bool {
    matches!(
        t,
        DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Exact { .. }
            | DataType::Approximate { .. }
            | DataType::DecFloat { .. }
            | DataType::Binary { .. }
            | DataType::VarBinary { .. }
            | DataType::Boolean
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Interval(_)
    )
}

/// That every part of `types` is `scalar`, else a refusal naming the
/// strategy `name`.
pub(crate) fn scalar_key(name: &str, types: &[DataType]) -> Result<()> {
    match types.iter().position(|t| !scalar(t)) {
        Some(at) => {
            refused(format!("[EXTENSION] key part {} of PARTITION BY {name} is of a type it cannot divide by", at.saturating_add(1)))
        }
        None => Ok(()),
    }
}

/// `values` assigned to `types`, one each.
///
/// # Errors
///
/// SQLCODE -103 of a count not the key's; an assignment's errors.
pub(crate) fn typed(values: Vec<Value>, types: &[DataType]) -> Result<Vec<Value>> {
    if values.len() != types.len() {
        return refused(format!("[EXTENSION] a bound of {} values for a key of {} parts", values.len(), types.len()));
    }
    values.into_iter().zip(types).map(|(v, t)| assign(v, t, "EXTENSION")).collect()
}

/// Whether two values are the same key part: equal, or both null.
pub(crate) fn same(a: &Value, b: &Value) -> bool {
    match (a.is_null(), b.is_null()) {
        (true, true) => true,
        (false, false) => compare(a, b) == Some(Ordering::Equal),
        (true, false) | (false, true) => false,
    }
}

/// Whether `v` meets `r`: of no restriction, any value does.
pub(crate) fn meets(v: &Value, r: Option<&Restriction>) -> bool {
    match r {
        None => true,
        Some(Restriction::Null) => v.is_null(),
        Some(Restriction::Equal(values)) => values.iter().any(|e| same(e, v)),
        Some(Restriction::Range { low, high }) => within(v, low.as_ref(), high.as_ref()),
    }
}

/// Whether `v`, not null, lies between `low` and `high`.
fn within(v: &Value, low: Option<&(Value, bool)>, high: Option<&(Value, bool)>) -> bool {
    let above =
        low.is_none_or(|(l, inclusive)| matches!((compare(v, l), inclusive), (Some(Ordering::Greater), _) | (Some(Ordering::Equal), true)));
    let below =
        high.is_none_or(|(h, inclusive)| matches!((compare(v, h), inclusive), (Some(Ordering::Less), _) | (Some(Ordering::Equal), true)));
    !v.is_null() && above && below
}

#[cfg(test)]
mod tests;
