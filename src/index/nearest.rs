//! Distance candidates and their pruning contract (D15, D38, D82).

use super::Key;
use crate::error::Result;
use crate::functions::ScalarRef;
use crate::storage::RowId;
use crate::value::Value;

/// A two-argument distance call with one indexed column and one value.
/// Support follows the function's declared semantics, never its SQL name.
#[derive(Debug, Clone)]
pub struct NearestQuery {
    /// The function evaluated on each fetched visible row.
    pub function: ScalarRef,
    /// The indexed column's position in the key, from zero.
    pub column: usize,
    /// That column's position in the call, zero or one.
    pub argument: usize,
    /// The other argument, including a later prepared parameter's value.
    pub value: Value,
}

/// Candidates `(lower bound, key, row)` in nondecreasing bound order.
/// Bounds are finite, nonnegative and no greater than the function's
/// computed non-NULL distance for that key. Every stable key with a
/// non-NULL distance appears once; extra NULL-distance candidates are
/// allowed. Key projections and counted versions follow [`super::Index`].
///
/// Returning this stream also certifies that the distance call cannot
/// raise a runtime error on any key held throughout the traversal. If an
/// index cannot establish that for the request, `Index::nearest` returns
/// `None` before producing any candidates. The executor then scans. Storage
/// errors remain errors; callers recheck visibility before evaluating a
/// candidate, including any optional concurrently inserted entry.
///
/// The iterator retains its index and reserves its frontier memory from
/// the supplied budget. An error is yielded once and ends the stream.
pub type NeighbourEntries = Box<dyn Iterator<Item = Result<(f64, Key, RowId)>> + Send>;
