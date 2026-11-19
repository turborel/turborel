//! \[EXTENSION\] VECTOR_DISTANCE of many rows at once (RULES.md D169): of
//! rows that name one metric that measures many in one call, as a
//! plug-in's does, and one query vector, the distances to every row's
//! other vector in one call (`metric::distances`). Other rows are measured
//! a row at a time, as `call` measures them.

use super::distance::Distance;
use super::vector_at;
use crate::error::Result;
use crate::value::Value;
use crate::vector::metric::{self, Metrics, metric_named};
use crate::vector::vector_of;

/// The values of VECTOR_DISTANCE of `rows`, if they can be measured in one
/// call: none if not, as of rows of a null, of several metrics, of a metric
/// that measures one pair at a time, or of no vector the same in every row.
///
/// # Errors
///
/// As `metric::distances`.
pub(super) fn call_many(rows: &[Vec<Value>], metrics: &Metrics) -> Option<Result<Vec<Value>>> {
    let first_row = rows.first()?;
    if rows.iter().any(|r| r.iter().any(Value::is_null) || r.len() != first_row.len()) {
        return None;
    }
    let (name, first) = Distance::VectorDistance.chosen(first_row).ok()?;
    let same_metric = rows.iter().all(|r| Distance::VectorDistance.chosen(r).is_ok_and(|(n, f)| n == name && f == first));
    let found = metric_named(metrics, &name).filter(|m| m.batches())?;
    let query_at = [first, first.checked_add(1)?]
        .into_iter()
        .find(|at| rows.iter().all(|r| same(r.get(*at), first_row.get(*at))))
        .filter(|at| *at == first || found.properties().symmetric)?;
    if !same_metric {
        return None;
    }
    let other_at = if query_at == first { first.checked_add(1)? } else { first };
    Some(measured(rows, (query_at, other_at), &*found))
}

/// The distances from each row's vector at `query_at`, the same in all, to
/// its vector at `other_at`, by `found`.
fn measured(rows: &[Vec<Value>], (query_at, other_at): (usize, usize), found: &dyn metric::VectorMetric) -> Result<Vec<Value>> {
    let query = vector_at(rows.first().map_or(&[][..], Vec::as_slice), query_at)?;
    let candidates = rows.iter().map(|r| vector_at(r, other_at)).collect::<Result<Vec<_>>>()?;
    let got = metric::distances(found, query, &candidates)?;
    Ok(got.into_iter().map(|d| d.map_or(Value::Null, Value::Approx)).collect())
}

/// Whether `a` and `b` are one vector, bit for bit.
fn same(a: Option<&Value>, b: Option<&Value>) -> bool {
    let (Some(a), Some(b)) = (a.and_then(vector_of), b.and_then(vector_of)) else { return false };
    (a.element, a.dims, a.bytes, a.indices) == (b.element, b.dims, b.bytes, b.indices)
}
