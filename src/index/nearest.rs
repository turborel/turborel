//! Distance candidates and their pruning contract (D15, D38, D85), and
//! the relaxed contract of an approximate index (RULES.md D94).

use std::fmt;
use std::sync::Arc;

use super::Key;
use crate::error::Result;
use crate::fulltext::rank::Statistics;
use crate::functions::ScalarRef;
use crate::storage::RowId;
use crate::value::Value;

/// \[EXTENSION\] How an index method serves a distance call
/// (`IndexMethod::supports_nearest`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NearestSupport {
    /// It does not.
    No,
    /// Under [`NeighbourEntries`]' certified contract: its candidates
    /// are every nearest row, so any query may read them.
    Exact,
    /// Under the relaxed contract of [`Approximation`]: it may miss rows,
    /// so only a query asking for an approximate search reads it (FETCH
    /// APPROX, VECTOR_SEARCH, or VECTOR.APPROX_DEFAULT ON; the vector
    /// design document R13.1), and only of a call of its own metric.
    Approximate,
}

/// A two-argument distance call with one indexed column and one value, or
/// a score's (`ScalarFunction::ranks`, RULES.md D87), the column and its
/// value first and its other arguments constants. Support follows the
/// function's declared semantics, never its SQL name.
#[derive(Clone)]
pub struct NearestQuery {
    /// The function evaluated on each fetched visible row.
    pub function: ScalarRef,
    /// The indexed column's position in the key, from zero.
    pub column: usize,
    /// That column's position in the call, zero or one.
    pub argument: usize,
    /// The other argument, including a later prepared parameter's value.
    pub value: Value,
    /// Whether the greatest call comes first, of a score: the stream's
    /// bounds are then the negated upper bounds of the calls, so they are
    /// lower bounds of the negated call, and the contract below holds of
    /// it.
    pub descending: bool,
    /// The statistics a score reads, the snapshot its statement's calls
    /// read too (`CallContext::ranking_statistics`), so the bounds and the
    /// calls agree.
    pub statistics: Option<Arc<dyn Statistics>>,
    /// \[EXTENSION\] The metric the call measures by, upper case, of a
    /// vector distance (`ScalarFunction::vector_metric`): an approximate
    /// index serves only a call of its own metric, EUCLIDEAN and
    /// EUCLIDEAN_SQUARED serving each other (R13.1, [`same_metric`]).
    pub metric: Option<String>,
    /// \[EXTENSION\] Of an approximate search, what it asks; none of an
    /// exact one, which only an index of `NearestSupport::Exact` serves.
    pub approximate: Option<Approximation>,
}

impl fmt::Debug for NearestQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NearestQuery")
            .field("function", &self.function)
            .field("column", &self.column)
            .field("argument", &self.argument)
            .field("value", &self.value)
            .field("descending", &self.descending)
            .field("statistics", &self.statistics.is_some())
            .field("metric", &self.metric)
            .field("approximate", &self.approximate)
            .finish()
    }
}

/// \[EXTENSION\] What an approximate search asks of an index of
/// `NearestSupport::Approximate` (the vector design document §9.2, R9.3,
/// R9.4, R13.2). Each `Option` none leaves the index's own setting.
///
/// The relaxed contract of its stream (`Index::approximate`): candidates
/// `(distance, key, row)`, each pair at most once, in the order the index
/// ranks them, its `distance` the index's own measure of the key by its
/// metric, which may be negative (DOT) and need bound nothing. It may
/// miss nearest rows, and ends after its candidates. It still retains its
/// index, reserves its memory from the budget given, and yields a storage
/// error once, ending there. As of the certified stream, returning one
/// certifies that the distance call cannot raise on any key the index
/// holds (a vector of its own dimensions and format, and a query value of
/// them, or null); else the index returns none before any candidate, and
/// the executor reads the table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Approximation {
    /// The rows asked for, OFFSET's and FETCH FIRST's: at least these the
    /// search tries to give.
    pub first: usize,
    /// The candidates a graph search keeps: HNSW's ef, DiskANN's L.
    pub search_list: Option<u32>,
    /// DiskANN's beam width W.
    pub beam_width: Option<u32>,
    /// IVF's lists probed.
    pub probes: Option<u32>,
    /// The share of the true nearest rows to find, a percentage (R9.4).
    pub accuracy: Option<u8>,
    /// How many times wider than it would search this one searches: 1 at
    /// first, doubled each time the rows found meet the condition too
    /// seldom (R9.3). An index multiplies its search list and probes by
    /// it, the search list kept at least `first`.
    pub widen: u32,
    /// Of a range search (§9.4, R9.1), the distance no row the query keeps
    /// reaches: the search may stop past it.
    pub radius: Option<f64>,
    /// Where the query's WHERE condition is applied (R13.2).
    pub filter: FilterHint,
}

/// \[EXTENSION\] How an approximate search meets its WHERE condition
/// (R13.2). The executor checks every candidate's row against the whole
/// condition whichever it is, so a hint never decides which rows qualify.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterHint {
    /// No condition, or one the index is told nothing of.
    None,
    /// The index may ask of a candidate whether its row qualifies
    /// ([`RowFilter`]), passing over those that do not as it searches.
    InFilter,
    /// The condition keeps about this share of the rows: the index may
    /// search for `first` divided by it.
    PostFilter(f64),
}

/// \[EXTENSION\] Whether the visible row of an entry meets a query's WHERE
/// condition, as an approximate search asks of its candidates
/// (`FilterHint::InFilter`).
///
/// Contract (RULES.md R-SAFE-3): the index calls it only while
/// `Index::approximate` runs, never from the stream it returns; and holds
/// no lock or latch of its own while it does (R-ERR-4), since the call
/// reads the table. An error ends the search, the index returning it.
pub trait RowFilter {
    /// Whether the row `row`, of the entry under `key`, qualifies.
    ///
    /// # Errors
    ///
    /// Those of reading the row or evaluating the condition.
    fn admits(&mut self, key: &Key, row: RowId) -> Result<bool>;
}

/// The name EUCLIDEAN_SQUARED is matched under: EUCLIDEAN's, whose order
/// it keeps; L2_SQUARED is its other name (the vector design document §6).
fn matched(metric: &str) -> &str {
    let squared = ["EUCLIDEAN_SQUARED", "L2_SQUARED"].iter().any(|m| metric.eq_ignore_ascii_case(m));
    if squared { "EUCLIDEAN" } else { metric }
}

/// \[EXTENSION\] Whether an index of metric `index` serves a call of
/// metric `call` (R13.1): the same name in any case, EUCLIDEAN and
/// EUCLIDEAN_SQUARED serving each other.
pub fn same_metric(index: &str, call: &str) -> bool {
    matched(index).eq_ignore_ascii_case(matched(call))
}

/// Candidates `(lower bound, key, row)` in nondecreasing bound order, of
/// the call, or of the negated call of a `descending` query.
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
/// An approximate index's stream keeps the relaxed contract of
/// [`Approximation`] instead.
pub type NeighbourEntries = Box<dyn Iterator<Item = Result<(f64, Key, RowId)>> + Send>;
