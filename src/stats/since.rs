//! \[DESIGN\] Statistics read after the table changed (RULES.md D381): a
//! pass's answers, told how many rows were written since and how many
//! the table holds now. A value past the greatest the pass saw may have
//! been written since, so once anything was, the least and greatest are
//! no longer bounds (`ColumnStats::unbounded`); a column whose greatest
//! value rose at each of the last two passes is taken to grow with the
//! rows inserted, and a range past it keeps the share written since; and
//! a column whose distinct values were within `KEY_SHARE` of its rows,
//! a key, has as many more distinct values as the table has rows more.

use std::sync::Arc;

use super::{ColumnStats, EnvelopeStats, Frequencies, RowSample, Statistics, TableStats, parts::count};
use crate::value::{CompOp, Value};

/// \[DESIGN\] The share of its non-null rows a column's distinct values
/// must reach for it to be taken as a key, its distinct values growing
/// with the rows: within 5%, as a sampled count of a key's is.
pub const KEY_SHARE: f64 = 0.95;
/// \[DESIGN\] The passes in a row at which a column's greatest value must
/// have risen for it to be taken to grow with insertion: two, so one
/// batch of high values is not taken for a trend.
pub const GROWING: u8 = 2;

/// What has changed since a pass: rows written since, rows it found, and
/// rows the table holds now.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Since {
    /// Rows inserted, updated or deleted since.
    pub changes: f64,
    /// Rows the pass found.
    pub rows: f64,
    /// Rows the table holds now.
    pub live: f64,
}

impl Since {
    /// Of no change.
    pub const NONE: Since = Since { changes: 0.0, rows: 0.0, live: 0.0 };

    /// Whether anything was written since.
    pub fn changed(self) -> bool {
        self.changes > 0.0_f64
    }
}

impl ColumnStats {
    /// Its distinct non-null values now, at least one: a key's scaled to
    /// the rows the table holds now, any other's as the pass found
    /// (`KEY_SHARE`).
    pub fn distinct_now(&self, since: Since) -> f64 {
        let at_pass = self.distinct_at_least_one();
        let key = at_pass >= KEY_SHARE * since.rows * (1.0_f64 - self.nulls);
        if key && since.live > since.rows && since.rows > 0.0_f64 { at_pass * since.live / since.rows } else { at_pass }
    }

    /// Whether its greatest value has risen at each of the last `GROWING`
    /// passes.
    pub fn grows(&self) -> bool {
        self.rises >= GROWING
    }
}

/// A table's statistics, as a pass found them, read after `since`.
#[derive(Debug)]
pub struct Changed {
    stats: Arc<TableStats>,
    since: Since,
}

impl Changed {
    /// `stats` read after `changes` rows were written, the table holding
    /// `live`: `stats` itself if nothing was and it holds what they found.
    pub fn of(stats: Arc<TableStats>, changes: u64, live: u64) -> Arc<dyn Statistics> {
        if changes == 0 && live == stats.rows {
            return stats;
        }
        let since = Since { changes: count(changes), rows: count(stats.rows), live: count(live) };
        Arc::new(Changed { stats, since })
    }

    fn column(&self, column: usize) -> Option<&ColumnStats> {
        self.stats.columns.get(column)
    }
}

impl Statistics for Changed {
    fn rows(&self) -> Option<f64> {
        self.stats.rows()
    }
    fn width(&self, columns: &[usize]) -> Option<f64> {
        self.stats.width(columns)
    }
    fn distinct(&self, column: usize) -> Option<f64> {
        Some(self.column(column)?.distinct_now(self.since))
    }
    fn nulls(&self, column: usize) -> Option<f64> {
        self.stats.nulls(column)
    }
    fn selectivity(&self, column: usize, op: CompOp, value: &Value) -> Option<f64> {
        Some(self.column(column)?.selectivity_since(op, value, self.since))
    }
    /// The pass's, its most rows of one value raised by the rows written
    /// since, each of which may hold it, so it stays a bound (RULES.md
    /// D385).
    fn frequencies(&self, column: usize) -> Option<Frequencies> {
        self.stats.frequencies(column).map(|f| Frequencies { most: f.most + self.since.changes, ..f })
    }
    fn bounds(&self, column: usize) -> Option<(Value, Value)> {
        self.stats.bounds(column)
    }
    fn envelopes(&self, column: usize) -> Option<&EnvelopeStats> {
        self.stats.envelopes(column)
    }
    fn histogram(&self, column: usize) -> Option<Vec<Value>> {
        self.stats.histogram(column)
    }
    fn common(&self, column: usize) -> Option<&[(Value, f64)]> {
        self.stats.common(column)
    }
    fn sample(&self) -> Option<&RowSample> {
        self.stats.sample()
    }
}

// Unit tests: bounds after change, a growing column's range past its
// greatest value, and a key's distinct values scaled (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
