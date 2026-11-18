//! \[EXTENSION\] Top-k and range search by dot product: Block-Max WAND,
//! after Ding and Suel, "Faster Top-k Document Retrieval Using Block-Max
//! Indexes", SIGIR 2011, §4, over WAND, after Broder, Carmel, Herscovici,
//! Soffer and Zien, "Efficient Query Evaluation using a Two-Level
//! Retrieval Process", CIKM 2003. The vector design's §10.4 asks for it:
//! "query by block-max WAND; exact on the sparse dot product".
//!
//! Cursors are kept in row order. The pivot is the first cursor at which
//! the dimensions' bounds summed pass the threshold: no row before it can
//! pass. The cursors up to it are moved shallowly to the pivot's row, and
//! their blocks' bounds summed; if those do not pass either, every row up
//! to the nearest block end is skipped undecoded. Otherwise the pivot's
//! row is scored, or the cursors before it are moved to it.
//!
//! Scores are summed in double precision. A row's score from stored
//! weights is widened by their blocks' errors into a bound of its true
//! score; a row whose bound passes is scored again by the caller, if it
//! gives a rescoring, from the row itself, and that exact score is the one
//! kept. So with a rescoring the top k are exactly a brute force's, ties
//! going to the lower row; without, with FLOAT32 weights too.
//!
//! A row with several versions indexed, which only the caller can tell
//! apart, has the most of each dimension's postings over its versions as
//! its score unrescored: a bound, not exact.
//!
//! Negative weights are allowed: a dimension's bound is the most it adds,
//! 0 if that is negative, as a row without it gets 0 of it. Rows sharing
//! no dimension with the query score 0 and are never visited, so a search
//! whose k-th score, or whose least score asked, is not above 0 says it is
//! incomplete, for the caller to search exactly instead.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;

use super::cursor::Cursor;
use super::{SparseTree, SparseVector, keys};
use crate::error::Result;
use crate::memory::{MemoryBudget, Reservation};
use crate::storage::RowId;

/// \[DESIGN\] How far a bound summed in double precision is widened
/// before it is compared, past any rounding of the sums.
const BOUND_SLACK: f64 = 1.0e-9;
/// \[DESIGN\] The least a bound is widened by, past rounding near 0.
const LEAST_SLACK: f64 = 1.0e-30;
/// \[DESIGN\] The bytes a cursor holds: a block and its postings decoded.
const CURSOR_BYTES: usize = 16_384;
/// \[DESIGN\] The bytes a hit held takes.
const HIT_BYTES: usize = 32;
/// \[DESIGN\] The hits a range search gathers between reservations.
const RANGE_BATCH: usize = 1_024;

/// \[EXTENSION\] A row found, and its score: exact if from its row (a
/// rescoring) or from FLOAT32 weights, else from weights as stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// The row.
    pub row: RowId,
    /// Its dot product with the query.
    pub score: f64,
    /// Whether `score` is exact.
    pub exact: bool,
}

/// \[EXTENSION\] What a search found, best first, and whether it is
/// complete: whether no row it did not visit could be among them.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// The rows, by score descending, then row ascending.
    pub hits: Vec<Hit>,
    /// False if rows sharing no dimension with the query could be among
    /// them: the caller searches exactly instead (R9.2).
    pub complete: bool,
}

/// A rescoring: the exact score of a row, or none if the row is not to
/// be kept, as one not visible or not meeting a condition.
pub type Rescore<'a> = &'a mut dyn FnMut(RowId) -> Result<Option<f64>>;

/// An in-filter: whether a row qualifies, as `Index::approximate`'s
/// `RowFilter` answers, asked holding no latch of the index.
pub type Filter<'a> = &'a mut dyn FnMut(RowId) -> Result<bool>;

/// \[EXTENSION\] How a search is asked: an in-filter of rows, asked only
/// of a row whose bound passes, before it is rescored, and a rescoring.
#[derive(Default)]
pub struct Ask<'a> {
    /// Rows to keep; none keeps every row.
    pub filter: Option<Filter<'a>>,
    /// The exact score of a row whose bound passes; none takes the score
    /// of the weights stored.
    pub rescore: Option<Rescore<'a>>,
}

/// What a search keeps, and the threshold a row's bound must pass.
trait Collect {
    /// The threshold, none while any row passes.
    fn threshold(&self) -> Option<f64>;
    /// Whether a bound equal to the threshold fails it.
    fn strict(&self) -> bool;
    /// Keeps `hit` if it passes.
    fn offer(&mut self, hit: Hit) -> Result<()>;
}

/// Whether `bound`, widened past rounding, passes `collect`'s threshold.
fn passes(bound: f64, collect: &dyn Collect) -> bool {
    let widened = bound + bound.abs() * BOUND_SLACK + LEAST_SLACK;
    let Some(threshold) = collect.threshold() else { return true };
    if collect.strict() { widened > threshold } else { widened >= threshold }
}

/// A hit, ordered so the worst is greatest: the heap's top.
struct Worst(Hit);

impl Ord for Worst {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.score.total_cmp(&self.0.score).then(self.0.row.cmp(&o.0.row))
    }
}
impl PartialOrd for Worst {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl PartialEq for Worst {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Worst {}

/// The k best rows.
struct TopK {
    k: usize,
    heap: BinaryHeap<Worst>,
}

impl Collect for TopK {
    fn threshold(&self) -> Option<f64> {
        if self.heap.len() < self.k { None } else { self.heap.peek().map(|w| w.0.score) }
    }
    fn strict(&self) -> bool {
        true
    }
    fn offer(&mut self, hit: Hit) -> Result<()> {
        if self.heap.len() < self.k {
            self.heap.push(Worst(hit));
        } else if self.heap.peek().is_some_and(|worst| Worst(hit) < *worst) {
            self.heap.pop();
            self.heap.push(Worst(hit));
        }
        Ok(())
    }
}

/// Every row of at least a score.
struct Range {
    least: f64,
    hits: Vec<Hit>,
    held: Reservation,
    /// The bytes held for the cursors.
    base: usize,
}

impl Collect for Range {
    fn threshold(&self) -> Option<f64> {
        Some(self.least)
    }
    fn strict(&self) -> bool {
        false
    }
    fn offer(&mut self, hit: Hit) -> Result<()> {
        if hit.score < self.least {
            return Ok(());
        }
        if self.hits.len().is_multiple_of(RANGE_BATCH) {
            let more = self.hits.len().saturating_add(RANGE_BATCH).saturating_mul(HIT_BYTES);
            self.held.resize(more.saturating_add(self.base))?;
        }
        self.hits.push(hit);
        Ok(())
    }
}

impl SparseTree {
    /// The `k` rows of the greatest dot product with `query`, kept by
    /// `ask`, best first, ties to the lower row.
    ///
    /// # Errors
    ///
    /// The tree's and the rescoring's; SQLCODE -918 if its cursors and
    /// hits cannot be reserved of `budget`.
    pub fn top_k(&self, query: &SparseVector, k: usize, ask: Ask<'_>, budget: &Arc<MemoryBudget>) -> Result<Found> {
        let heap = k.saturating_mul(HIT_BYTES);
        let _held = budget.reserve(heap.saturating_add(query.nnz().saturating_mul(CURSOR_BYTES)), "a sparse index search")?;
        let mut collect = TopK { k, heap: BinaryHeap::with_capacity(k.min(RANGE_BATCH)) };
        if k > 0 {
            self.search(query, &mut collect, ask)?;
        }
        let complete = collect.heap.len() == k && collect.threshold().is_none_or(|t| t > 0.0_f64);
        let mut hits: Vec<Hit> = collect.heap.into_iter().map(|w| w.0).collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.row.cmp(&b.row)));
        Ok(Found { hits, complete })
    }

    /// Every row of a dot product with `query` of at least `least`, kept
    /// by `ask`, best first: a range search (§9.4), complete only if
    /// `least` is above 0.
    ///
    /// # Errors
    ///
    /// As `top_k`.
    pub fn within(&self, query: &SparseVector, least: f64, ask: Ask<'_>, budget: &Arc<MemoryBudget>) -> Result<Found> {
        let base = query.nnz().saturating_mul(CURSOR_BYTES);
        let held = budget.reserve(base, "a sparse index search")?;
        let mut collect = Range { least, hits: Vec::new(), held, base };
        self.search(query, &mut collect, ask)?;
        let mut hits = collect.hits;
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.row.cmp(&b.row)));
        Ok(Found { hits, complete: least > 0.0_f64 })
    }

    /// Block-Max WAND over `query`'s dimensions.
    fn search(&self, query: &SparseVector, collect: &mut dyn Collect, mut ask: Ask<'_>) -> Result<()> {
        let mut cursors = self.cursors(query)?;
        loop {
            cursors.sort_by_key(|c| (c.doc.is_none(), c.doc));
            let Some((pivot, doc)) = pivot(&cursors, collect) else { return Ok(()) };
            let mut blocks = 0.0_f64;
            for cursor in cursors.iter_mut().take(pivot.saturating_add(1)) {
                cursor.shallow(doc)?;
                blocks += cursor.block_bound();
            }
            if !passes(blocks, collect) {
                skip(&mut cursors, pivot, doc)?;
            } else if cursors.first().and_then(|c| c.doc) == Some(doc) {
                evaluate(&mut cursors, doc, collect, &mut ask)?;
            } else {
                for cursor in cursors.iter_mut().take(pivot).filter(|c| c.doc.is_some_and(|d| d < doc)) {
                    cursor.advance(doc)?;
                }
            }
        }
    }

    /// A cursor for each dimension of `query` the index holds.
    fn cursors(&self, query: &SparseVector) -> Result<Vec<Cursor>> {
        let mut out = Vec::with_capacity(query.nnz());
        for (dimension, weight) in query.entries() {
            let Some(value) = self.inner.tree.get(&keys::summary_key(dimension))? else { continue };
            let cursor = Cursor::new(&self.inner.tree, dimension, f64::from(weight), keys::bounds_of(&value)?)?;
            if cursor.doc.is_some() {
                out.push(cursor);
            }
        }
        Ok(out)
    }
}

/// The pivot: the first cursor, in row order, at which the dimensions'
/// bounds summed pass the threshold, past any after it at its row; and
/// its row. None if no row can pass.
fn pivot(cursors: &[Cursor], collect: &dyn Collect) -> Option<(usize, u64)> {
    let mut sum = 0.0_f64;
    for (at, cursor) in cursors.iter().enumerate() {
        let doc = cursor.doc?;
        sum += cursor.bound;
        if passes(sum, collect) {
            let same = cursors.get(at.saturating_add(1)..).unwrap_or_default().iter().take_while(|c| c.doc == Some(doc)).count();
            return Some((at.saturating_add(same), doc));
        }
    }
    None
}

/// Skips the rows from `doc` to the nearest end of the pivot's and its
/// predecessors' blocks, or the next cursor's row if nearer: no row there
/// can pass, as their blocks' bounds do not.
fn skip(cursors: &mut [Cursor], pivot: usize, doc: u64) -> Result<()> {
    let ends = cursors.iter().take(pivot.saturating_add(1)).map(|c| c.block_last().map_or(u64::MAX, |l| l.saturating_add(1)));
    let after = cursors.get(pivot.saturating_add(1)).and_then(|c| c.doc).unwrap_or(u64::MAX);
    let next = ends.fold(after, u64::min).max(doc.saturating_add(1));
    for cursor in cursors.iter_mut().take(pivot.saturating_add(1)).filter(|c| c.doc.is_some_and(|d| d < next)) {
        cursor.advance(next)?;
    }
    Ok(())
}

/// Scores row `doc`, at which the first cursors are, and moves them past it.
fn evaluate(cursors: &mut [Cursor], doc: u64, collect: &mut dyn Collect, ask: &mut Ask<'_>) -> Result<()> {
    let at = cursors.iter().take_while(|c| c.doc == Some(doc)).count();
    let (mut score, mut error, mut several) = (0.0_f64, 0.0_f64, false);
    for cursor in cursors.iter().take(at) {
        let scored = cursor.score();
        score += scored.score;
        error += scored.error;
        several |= scored.several;
    }
    let mut kept = passes(score + error, collect);
    if let (true, Some(keep)) = (kept, ask.filter.as_mut()) {
        kept = keep(doc)?;
    }
    if kept {
        let hit = match ask.rescore.as_mut() {
            Some(rescore) => rescore(doc)?.map(|exact| Hit { row: doc, score: exact, exact: true }),
            None => Some(Hit { row: doc, score, exact: error <= 0.0_f64 && !several }),
        };
        if let Some(hit) = hit {
            collect.offer(hit)?;
        }
    }
    for cursor in cursors.iter_mut().take(at) {
        match doc.checked_add(1) {
            Some(next) => cursor.advance(next)?,
            None => cursor.doc = None,
        }
    }
    Ok(())
}
