//! \[EXTENSION\] PARTITION BY LIST (RULES.md D154): each partition holds
//! the keys it lists, a key equal to one listed as values compare
//! [X3.135-1986 §5.11], a null to a null listed. A key of one part is
//! listed as values, one of several as rows of values.

use super::{LIST, PartitionStrategy, Restriction, meets, refused, same, scalar_key, typed};
use crate::catalog::PartitionBound;
use crate::error::Result;
use crate::value::{DataType, Value};

/// The built-in LIST strategy.
#[derive(Debug, Clone, Copy, Default)]
pub struct ListStrategy;

impl PartitionStrategy for ListStrategy {
    fn name(&self) -> &str {
        LIST
    }

    fn check_key(&self, types: &[DataType]) -> Result<()> {
        scalar_key(LIST, types)
    }

    fn bound(&self, types: &[DataType], spec: PartitionBound) -> Result<PartitionBound> {
        let PartitionBound::List(keys) = spec else {
            return refused("[EXTENSION] a partition of PARTITION BY LIST is FOR VALUES IN (...) or DEFAULT");
        };
        let keys = keys.into_iter().map(|key| typed(key, types)).collect::<Result<Vec<_>>>()?;
        Ok(PartitionBound::List(keys))
    }

    fn overlaps(&self, a: &PartitionBound, b: &PartitionBound) -> Result<bool> {
        let (Some(a), Some(b)) = (listed(a), listed(b)) else { return Ok(false) };
        Ok(a.iter().any(|x| b.iter().any(|y| same_key(x, y))))
    }

    fn contains(&self, bound: &PartitionBound, key: &[Value]) -> Result<bool> {
        Ok(listed(bound).is_some_and(|keys| keys.iter().any(|k| same_key(k, key))))
    }

    fn admits(&self, bound: &PartitionBound, restrictions: &[Option<Restriction>]) -> Result<bool> {
        let fits = |key: &Vec<Value>| key.iter().enumerate().all(|(at, v)| meets(v, restrictions.get(at).and_then(Option::as_ref)));
        Ok(listed(bound).is_some_and(|keys| keys.iter().any(fits)))
    }
}

/// The keys a list bound lists; none of another bound.
fn listed(bound: &PartitionBound) -> Option<&[Vec<Value>]> {
    match bound {
        PartitionBound::List(keys) => Some(keys),
        PartitionBound::Default | PartitionBound::Range { .. } | PartitionBound::With(_) => None,
    }
}

/// Whether two keys are the same, part by part.
fn same_key(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
}
