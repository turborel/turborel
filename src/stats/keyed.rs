//! \[DESIGN\] A table's statistics with its indexes' counts of their keys
//! (RULES.md D383). An index's sketch counts the distinct keys it holds,
//! every one it was given, kept as it is written (`index/stats.rs`), so:
//!
//! * the distinct values of a column alone in an index's key are the
//!   index's, not a sample's scaled (`stats.rs`, Haas and Stokes), which
//!   a sample of a column of clustered values undercounts;
//! * the distinct combinations of columns that are exactly an index's
//!   key are its distinct keys, which count correlated columns once,
//!   where the product of each column's distinct values counts every
//!   pair the columns could make;
//! * columns that hold a unique key's are as distinct as the rows.
//!
//! The planner asks a conjunction of equalities, a grouping and a join
//! on several columns this through `Statistics::group_distinct`. A
//! sketch's count is an estimate made anew at each asking, so each
//! index's is counted once, when first asked, and kept as long as these
//! statistics, one planning's.

use std::sync::{Arc, OnceLock};

use super::{EnvelopeStats, Frequencies, RowSample, Statistics};
use crate::index::Index;
use crate::value::{CompOp, Value};

/// An index of the table, as these statistics read it.
pub struct KeyIndex {
    /// Its key's columns, sorted, each once.
    columns: Vec<usize>,
    /// Whether no two of its keys are equal.
    unique: bool,
    /// The index.
    index: Arc<dyn Index>,
    /// Its distinct keys, once counted.
    counted: OnceLock<Option<f64>>,
}

impl std::fmt::Debug for KeyIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyIndex").field("columns", &self.columns).field("unique", &self.unique).finish_non_exhaustive()
    }
}

impl KeyIndex {
    /// The index `index` of key `columns`, unique if `unique`.
    pub fn new(columns: &[usize], unique: bool, index: Arc<dyn Index>) -> KeyIndex {
        KeyIndex { columns: sorted(columns), unique, index, counted: OnceLock::new() }
    }

    /// Its distinct keys, counted once.
    fn keys(&self) -> Option<f64> {
        *self.counted.get_or_init(|| self.index.distinct_keys())
    }
}

/// `columns`, sorted, each once.
fn sorted(columns: &[usize]) -> Vec<usize> {
    let mut sorted = columns.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
}

/// A table's statistics and its indexes.
#[derive(Debug)]
pub struct Keyed {
    inner: Arc<dyn Statistics>,
    indexes: Vec<KeyIndex>,
}

impl Keyed {
    /// `inner`, answering from `indexes` too; `inner` itself if none.
    pub fn of(inner: Arc<dyn Statistics>, indexes: Vec<KeyIndex>) -> Arc<dyn Statistics> {
        if indexes.is_empty() {
            return inner;
        }
        Arc::new(Keyed { inner, indexes })
    }

    /// The distinct keys of an index whose key is exactly `columns`,
    /// sorted, if one counts them.
    fn exact(&self, columns: &[usize]) -> Option<f64> {
        self.indexes.iter().filter(|ix| ix.columns == columns).find_map(KeyIndex::keys)
    }
}

impl Statistics for Keyed {
    fn rows(&self) -> Option<f64> {
        self.inner.rows()
    }
    fn width(&self, columns: &[usize]) -> Option<f64> {
        self.inner.width(columns)
    }
    fn distinct(&self, column: usize) -> Option<f64> {
        self.exact(&[column]).or_else(|| self.inner.distinct(column))
    }
    fn nulls(&self, column: usize) -> Option<f64> {
        self.inner.nulls(column)
    }
    fn selectivity(&self, column: usize, op: CompOp, value: &Value) -> Option<f64> {
        self.inner.selectivity(column, op, value)
    }
    fn frequencies(&self, column: usize) -> Option<Frequencies> {
        self.inner.frequencies(column)
    }
    fn bounds(&self, column: usize) -> Option<(Value, Value)> {
        self.inner.bounds(column)
    }
    fn envelopes(&self, column: usize) -> Option<&EnvelopeStats> {
        self.inner.envelopes(column)
    }
    fn histogram(&self, column: usize) -> Option<Vec<Value>> {
        self.inner.histogram(column)
    }
    fn common(&self, column: usize) -> Option<&[(Value, f64)]> {
        self.inner.common(column)
    }
    fn sample(&self) -> Option<&RowSample> {
        self.inner.sample()
    }
    /// Of one column, its distinct values; of columns exactly an index's
    /// key, its distinct keys; of columns holding a unique key's, the
    /// rows; else not known.
    fn group_distinct(&self, columns: &[usize]) -> Option<f64> {
        let columns = sorted(columns);
        if let [one] = columns.as_slice() {
            return self.distinct(*one);
        }
        let unique = || self.indexes.iter().any(|ix| ix.unique && ix.columns.iter().all(|c| columns.binary_search(c).is_ok()));
        self.exact(&columns).or_else(|| unique().then(|| self.rows()).flatten())
    }
}
