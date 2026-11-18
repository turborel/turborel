//! \[EXTENSION\] An inverted index's approximate search (the vector
//! design document §9.2, §9.4, R9.3, R13.1, R13.2; RULES.md D94, D99):
//! the rows of the greatest dot product with a query, by block-max WAND
//! (`engine/sparse/wand.rs`), as the candidates of a call of DOT.
//!
//! DOT is `-(a·b)` (R6.8), so the rows nearest by it are those of the
//! greatest dot product, and a candidate's distance is its score negated.
//! The index serves DOT alone. A range search's radius `r` keeps the rows
//! of a dot product of at least `-r`; the executor drops those at `-r`
//! exactly, as `distance < r` asks.
//!
//! It certifies the call cannot raise only of a column fixing its
//! dimension count and format, and a query vector of both: DOT raises
//! for vectors of another count or format (R6.4), and of these never, its
//! sum of products of finite binary32 weights staying finite. Else it
//! declines and the executor reads the table.
//!
//! The rows asked are the query's, over a post-filter's share, widened
//! as the query asks (`index/vector/ask.rs`). An in-filter is asked of a
//! row whose bound passes the threshold, holding no latch of the index,
//! as the engine asks it (`RowFilter`).

use std::sync::Arc;

use crate::engine::sparse::{Ask, Found, SparseTree, SparseVector};
use crate::error::{Code, Result};
use crate::index::vector::{Ask as Asked, entry_key};
use crate::index::{NearestQuery, NearestSupport, NeighbourEntries, RowFilter, same_metric};
use crate::memory::MemoryBudget;
use crate::value::{DataType, Value};
use crate::vector::metric::DOT;
use crate::vector::{Element, Layout, Shape, shape_of, single, vector_of};

/// What a search's candidates reserve their memory as.
const CANDIDATES: &str = "an inverted index's approximate candidates";
/// Bytes a candidate costs, near enough: its distance, key and row.
const CANDIDATE_BYTES: usize = 64;

/// The dimension count and format a column of `t` fixes, if it is a
/// VECTOR column of SPARSE layout fixing both.
pub(super) fn fixed_sparse(t: &DataType) -> Option<(u32, Element)> {
    let Shape { dims, element, layout } = shape_of(t)?;
    match layout? {
        Layout::Sparse => Some((dims?, element?)),
        Layout::Dense => None,
    }
}

/// How the method serves `query` on `columns`: approximately, of an
/// approximate search ascending by a vector metric on its one column of a
/// fixed sparse shape; else not. Whether the metric is DOT, the index's,
/// is the planner's to ask (`IndexMethod::metric`).
pub(super) fn support(query: &NearestQuery, columns: &[DataType]) -> NearestSupport {
    let asked = query.approximate.is_some() && query.metric.is_some() && !query.descending && query.column == 0;
    let [column] = columns else { return NearestSupport::No };
    if asked && fixed_sparse(column).is_some() { NearestSupport::Approximate } else { NearestSupport::No }
}

/// `value` as a sparse vector, if it is a vector of `shape`, dense or
/// sparse: a dense one keeps its dimensions not zero.
fn query_vector(value: &Value, (dims, element): (u32, Element)) -> Option<SparseVector> {
    let v = vector_of(value)?;
    if v.dims != dims || v.element != element {
        return None;
    }
    let entries = (0..v.stored()).filter_map(|i| Some((v.index(i)?, single(v.value(i)?))));
    SparseVector::new(dims, entries).ok()
}

/// The candidates of an approximate search of `tree`, of a column of
/// `shape`, or none to decline: (distance, row), nearest first.
///
/// # Errors
///
/// Those of the tree and of `filter`.
pub(super) fn candidates(
    tree: &SparseTree,
    shape: Option<(u32, Element)>,
    (query, budget): (&NearestQuery, &Arc<MemoryBudget>),
    filter: Option<&mut dyn RowFilter>,
) -> Result<Option<Vec<(f64, u64)>>> {
    let (Some(approximation), Some(call), Some(shape)) = (query.approximate.as_ref(), query.metric.as_deref(), shape) else {
        return Ok(None);
    };
    let Some(q) = query_vector(&query.value, shape).filter(|_| same_metric(DOT, call)) else { return Ok(None) };
    let asked = Asked::of(approximation);
    let key = entry_key();
    let mut admits = filter.map(|f| move |row: u64| f.admits(&key, row));
    let ask = Ask { filter: admits.as_mut().map(|a| -> &mut dyn FnMut(u64) -> Result<bool> { a }), rescore: None };
    let found = match asked.radius {
        Some(radius) => tree.within(&q, -radius, ask, budget),
        None => tree.top_k(&q, asked.list(asked.wanted), ask, budget),
    };
    match found {
        Ok(Found { hits, .. }) => Ok(Some(hits.into_iter().map(|hit| (-hit.score, hit.row)).collect())),
        // Its memory refused: the scan, budgeted as it is, reads the table
        // instead (D38).
        Err(error) => {
            if error.code == Code::Resources {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

/// `found`'s candidates as a stream retaining `held`, their memory
/// reserved of `budget`; none if it cannot be.
pub(super) fn stream<T: Send + Sync + 'static>(
    held: Arc<T>,
    found: Vec<(f64, u64)>,
    budget: &Arc<MemoryBudget>,
) -> Option<NeighbourEntries> {
    let reservation = budget.reserve(found.len().saturating_mul(CANDIDATE_BYTES), CANDIDATES).ok()?;
    let key = entry_key();
    Some(Box::new(found.into_iter().map(move |(distance, row)| {
        let _held = (&held, &reservation);
        Ok((distance, key.clone(), row))
    })))
}
