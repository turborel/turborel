//! \[DESIGN\] Statistics for the optimizer (RULES.md D31): what a table's
//! rows are like, so the planner can estimate how many rows a condition
//! keeps and what reading them costs.
//!
//! What an engine knows about its rows depends on how it stores them, so
//! the planner asks through a trait, `Statistics`, every answer optional:
//! an engine answers what it knows and the planner falls back to defaults
//! for the rest. A row store samples whole rows in a pass, as the built-in
//! one does, its column store's frozen rows among them, every row of a
//! small table and a sample of a large one's (`StatsBuilder::counting`):
//! distinct values then scaled from the sample's, as Haas and Stokes
//! estimate them, and its least and greatest values not taken as bounds
//! (`ColumnStats::unbounded`); a columnar
//! engine may know distinct values exactly from its dictionaries, least and
//! greatest values from each segment, and the width of each column apart,
//! and answers from those.
//!
//! The parts statistics are built of (`parts`) each take one column's
//! values, so an engine feeds them a row at a time or a column at a time.
//! `TableStats` is one way to answer from them: per column, the distinct
//! values, the fraction that is NULL, the least and greatest values, and a
//! histogram of the values but its most common, which it keeps apart
//! with their shares (`common`), how often its values repeat
//! (`frequencies`), which a property graph's statistics are made of
//! (`graph`), and of a geometry column, its envelopes' (`envelopes`); and
//! of the table, a sample of whole rows (`sample`). `ColumnBuilder`
//! gathers a column's; `StatsBuilder` gathers a table's from its rows.
//! What changed since a pass is told by `since`, and what the table's
//! indexes count of their keys by `keyed`.

pub mod common;
pub mod envelopes;
pub mod frequencies;
pub mod graph;
pub mod keyed;
pub mod parts;
pub mod sample;
mod selectivity;
pub mod since;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::BuildHasher;
use std::sync::Arc;

pub use common::Common;
pub use envelopes::{EnvelopeBuilder, EnvelopeStats};
pub use frequencies::Frequencies;
pub use parts::{Histogram, Sampling};
use parts::{HyperLogLog, Reservoir, count};
pub use sample::{RowReservoir, RowSample};
pub use since::Since;

use crate::geometry::Geometry;
use crate::hash::murmur::MurmurBuildHasher;
use crate::storage::Row;
use crate::value::{CompOp, Value, compare};

/// \[DESIGN\] What the planner may ask about a table's rows. Every answer
/// is optional: `None` is "not known", and the planner then assumes a
/// default. Fractions are of all the table's rows.
pub trait Statistics: Send + Sync + Debug {
    /// The rows of the table when the statistics were gathered.
    fn rows(&self) -> Option<f64> {
        None
    }
    /// The average bytes of a row read for a query needing `columns`: a
    /// row store reads whole rows, a columnar engine only those columns.
    fn width(&self, _columns: &[usize]) -> Option<f64> {
        None
    }
    /// The distinct non-null values of `column`.
    fn distinct(&self, _column: usize) -> Option<f64> {
        None
    }
    /// The fraction of rows whose `column` is NULL.
    fn nulls(&self, _column: usize) -> Option<f64> {
        None
    }
    /// The fraction of rows for which "`column` `op` `value`" is true.
    fn selectivity(&self, _column: usize, _op: CompOp, _value: &Value) -> Option<f64> {
        None
    }
    /// How often `column`'s distinct values repeat (`stats/frequencies.rs`).
    fn frequencies(&self, _column: usize) -> Option<Frequencies> {
        None
    }
    /// The least and greatest non-null values of `column`.
    fn bounds(&self, _column: usize) -> Option<(Value, Value)> {
        None
    }
    /// Of a geometry `column`, its envelopes' statistics (RULES.md D137).
    fn envelopes(&self, _column: usize) -> Option<&EnvelopeStats> {
        None
    }
    /// The bounds of `column`'s equi-depth histogram, ascending, as a
    /// plug-in is told them (RULES.md D279).
    fn histogram(&self, _column: usize) -> Option<Vec<Value>> {
        None
    }
    /// The most common values of `column`, each with the share of all
    /// rows it holds, the most frequent first (RULES.md D381).
    fn common(&self, _column: usize) -> Option<&[(Value, f64)]> {
        None
    }
    /// A sample of the table's whole rows, its columns in order
    /// (RULES.md D382).
    fn sample(&self) -> Option<&RowSample> {
        None
    }
    /// The distinct combinations of the values of `columns`, together,
    /// rows with a null in any left out: of one column, its distinct
    /// values; of several, as an index whose key they are counts its
    /// keys, or the rows if they hold a unique key (RULES.md D383).
    fn group_distinct(&self, _columns: &[usize]) -> Option<f64> {
        None
    }
}

/// What a column's values are like.
#[derive(Debug, Clone, Default)]
pub struct ColumnStats {
    /// Its distinct non-null values.
    pub distinct: f64,
    /// The fraction of its values that are NULL.
    pub nulls: f64,
    /// Its least value, if it has any but NULL.
    pub min: Option<Value>,
    /// Its greatest value, if it has any but NULL.
    pub max: Option<Value>,
    /// The histogram of its non-null values but its most common.
    pub histogram: Histogram,
    /// Its most common values, each with the share of all rows it holds,
    /// the most frequent first (`stats/common.rs`).
    pub common: Common,
    /// At how many passes in a row, up to `since::GROWING`, its greatest
    /// value had risen past the pass before's.
    pub rises: u8,
    /// Whether values may lie past `min` and `max`, they being a sample's,
    /// so a value outside them is not estimated to match no row.
    pub unbounded: bool,
    /// How often its distinct non-null values repeat, if it has any.
    pub frequencies: Option<Frequencies>,
    /// Of a column of geometries, their envelopes' statistics.
    pub envelopes: Option<Box<EnvelopeStats>>,
}

/// \[DESIGN\] Statistics of a table's rows, column by column, as
/// `ColumnBuilder`s gathered them.
#[derive(Debug, Clone, Default)]
pub struct TableStats {
    /// Its rows.
    pub rows: u64,
    /// Their average width in bytes, as stored.
    pub width: f64,
    /// Its columns, in order.
    pub columns: Vec<ColumnStats>,
    /// A sample of its rows, if it has any.
    pub sample: Option<Arc<RowSample>>,
}

impl Statistics for TableStats {
    fn rows(&self) -> Option<f64> {
        Some(count(self.rows))
    }
    /// Whole rows, whatever columns are needed.
    fn width(&self, _columns: &[usize]) -> Option<f64> {
        Some(self.width)
    }
    fn distinct(&self, column: usize) -> Option<f64> {
        Some(self.columns.get(column)?.distinct)
    }
    fn nulls(&self, column: usize) -> Option<f64> {
        Some(self.columns.get(column)?.nulls)
    }
    fn selectivity(&self, column: usize, op: CompOp, value: &Value) -> Option<f64> {
        Some(self.columns.get(column)?.selectivity(op, value))
    }
    fn frequencies(&self, column: usize) -> Option<Frequencies> {
        self.columns.get(column)?.frequencies
    }
    fn bounds(&self, column: usize) -> Option<(Value, Value)> {
        let c = self.columns.get(column)?;
        Some((c.min.clone()?, c.max.clone()?))
    }
    fn envelopes(&self, column: usize) -> Option<&EnvelopeStats> {
        self.columns.get(column)?.envelopes.as_deref()
    }
    fn histogram(&self, column: usize) -> Option<Vec<Value>> {
        Some(self.columns.get(column)?.histogram.bounds().to_vec())
    }
    fn common(&self, column: usize) -> Option<&[(Value, f64)]> {
        Some(&self.columns.get(column)?.common)
    }
    fn sample(&self) -> Option<&RowSample> {
        self.sample.as_deref()
    }
    /// Of one column, its distinct values; of several, not known: no
    /// index tells them here (`stats/keyed.rs`).
    fn group_distinct(&self, columns: &[usize]) -> Option<f64> {
        match columns {
            [one] => self.distinct(*one),
            [] | [_, _, ..] => None,
        }
    }
}

/// \[DESIGN\] Gathers one column's statistics from its values, in one pass.
#[derive(Debug, Clone, Default)]
pub struct ColumnBuilder {
    sampling: Sampling,
    values: u64,
    nulls: u64,
    sketch: HyperLogLog,
    sample: Reservoir,
    min: Option<Value>,
    max: Option<Value>,
    /// Of a sample of the rows, in how many of the runs read each value
    /// was seen, by the hash of its normalized form, for the distinct
    /// values to be scaled.
    seen: Option<HashMap<u64, Runs>>,
    /// The run of rows the values added now are of, if told: if not,
    /// each value is of a run of its own.
    run: Option<u64>,
    /// Of a column of geometries, their envelopes, begun at the first.
    envelopes: Option<EnvelopeBuilder>,
}

impl ColumnBuilder {
    /// A builder no value has been added to.
    pub fn new() -> ColumnBuilder {
        ColumnBuilder::default()
    }

    /// A builder no value has been added to, sampling as `sampling` says.
    pub fn sampling(sampling: Sampling) -> ColumnBuilder {
        ColumnBuilder { sampling, sample: Reservoir::keeping(sampling.values), ..ColumnBuilder::default() }
    }

    /// Adds `value`, NULL or not.
    pub fn add(&mut self, value: &Value) {
        self.values = self.values.saturating_add(1);
        if value.is_null() {
            self.nulls = self.nulls.saturating_add(1);
            return;
        }
        self.sketch.add(value);
        if let Some(seen) = self.seen.as_mut() {
            seen.entry(MurmurBuildHasher::default().hash_one(value.norm())).or_insert(Runs { runs: 0, last: None }).saw(self.run);
        }
        if Geometry::is_geometry(value) {
            self.add_geometry(value);
        } else {
            self.sample.add(value);
        }
        if self.min.as_ref().is_none_or(|min| compare(value, min) == Some(Ordering::Less)) {
            self.min = Some(value.clone());
        }
        if self.max.as_ref().is_none_or(|max| compare(value, max) == Some(Ordering::Greater)) {
            self.max = Some(value.clone());
        }
    }

    /// Adds the geometry `value` to its envelopes' statistics, not to the
    /// sample: a geometry has no order a histogram could place it by, and
    /// one of many vertices would take a sample's memory (RULES.md D137).
    /// A value that cannot be read is left out, as a scan reading it would
    /// raise its error.
    fn add_geometry(&mut self, value: &Value) {
        if let Ok(geometry) = Geometry::from_value(value) {
            let most = self.sampling.values;
            self.envelopes.get_or_insert_with(|| EnvelopeBuilder::keeping(most)).add(&geometry);
        }
    }

    /// The statistics of the values added.
    pub fn finish(self) -> ColumnStats {
        let distinct = self.sketch.estimate().min(count(self.values.saturating_sub(self.nulls)));
        self.finish_with(distinct)
    }

    /// The statistics of the values added, of `distinct` distinct values:
    /// the most common of the sample kept apart, the histogram of the
    /// rest (`stats/common.rs`).
    fn finish_with(self, distinct: f64) -> ColumnStats {
        let present = self.values.saturating_sub(self.nulls);
        let nulls = if self.values == 0 { 0.0_f64 } else { count(self.nulls) / count(self.values) };
        let sorted = self.sample.into_sorted();
        let frequencies = Frequencies::of_sorted(&sorted, count(present));
        let (common, rest) = common::common_of(sorted, 1.0_f64 - nulls, distinct);
        ColumnStats {
            distinct,
            nulls,
            min: self.min,
            max: self.max,
            histogram: Histogram::of_sorted(&rest, self.sampling.buckets),
            common,
            rises: 0,
            unbounded: false,
            frequencies,
            envelopes: self.envelopes.and_then(EnvelopeBuilder::finish).map(Box::new),
        }
    }

    /// The statistics of a table of `rows` rows, the values added a sample
    /// of them: the share NULL and the histogram the sample's, the distinct
    /// values scaled from its (`scaled_distinct`), and not bounded by its
    /// least and greatest.
    fn finish_scaled(mut self, rows: f64) -> ColumnStats {
        let seen = self.seen.take().unwrap_or_default();
        let present = count(self.values.saturating_sub(self.nulls));
        // Each run's repeats of a value are collapsed to one, and the
        // sample and the table taken as the values so collapsed
        // (Chaudhuri, Das and Srivastava, "Effective use of block-level
        // sampling in statistics estimation", SIGMOD 2004): rows near in a
        // table often share a value, which a sample of runs would
        // otherwise take for a value repeated across the table.
        let collapsed = count(seen.values().map(|r| u64::from(r.runs)).sum());
        let once = count(seen.values().map(|r| u64::from(r.runs == 1)).sum());
        let nulls = if self.values == 0 { 0.0_f64 } else { count(self.nulls) / count(self.values) };
        let found = count(u64::try_from(seen.len()).unwrap_or(u64::MAX));
        let of = rows * (1.0_f64 - nulls) * collapsed / present.max(1.0_f64);
        let distinct = scaled_distinct((found, once), collapsed, of).min(rows * (1.0_f64 - nulls));
        let mut stats = self.finish_with(distinct);
        stats.unbounded = true;
        // Each value's rows of the sample, scaled to the table's.
        let factor = rows * (1.0_f64 - stats.nulls) / present.max(1.0_f64);
        stats.frequencies = stats.frequencies.map(|f| f.scaled(factor.max(1.0_f64)));
        stats
    }
}

/// \[DESIGN\] The distinct values of `of` values, from a sample of `sampled`
/// of them holding `distinct` distinct ones, `once` of them seen once: Haas
/// and Stokes's estimator Duj1 ("Estimating the number of classes in a
/// finite population", JASA 93(444), 1998), n d / (n - f1 + f1 n / N); at
/// least those of the sample, at most `of`.
pub fn scaled_distinct((distinct, once): (f64, f64), sampled: f64, of: f64) -> f64 {
    let below = sampled - once + once * sampled / of.max(1.0_f64);
    let scaled = if below > 0.0_f64 { sampled * distinct / below } else { of };
    scaled.clamp(distinct.min(of), of.max(distinct))
}

/// In how many runs of rows a value was seen, and the last.
#[derive(Debug, Clone, Copy)]
struct Runs {
    runs: u32,
    last: Option<u64>,
}

impl Runs {
    /// Counts the value seen in run `run`, once for each run; of no run
    /// told, once each time.
    fn saw(&mut self, run: Option<u64>) {
        if run.is_none() || self.last != run {
            self.runs = self.runs.saturating_add(1);
            self.last = run;
        }
    }
}

/// \[DESIGN\] Gathers a table's statistics from its rows, in one pass.
pub struct StatsBuilder {
    rows: u64,
    bytes: u64,
    columns: Vec<ColumnBuilder>,
    sample: RowReservoir,
}

impl StatsBuilder {
    /// A builder for rows of `columns` columns.
    pub fn new(columns: usize) -> StatsBuilder {
        StatsBuilder::sampling(columns, Sampling::default())
    }

    /// A builder for rows of `columns` columns, sampling as `sampling` says.
    pub fn sampling(columns: usize, sampling: Sampling) -> StatsBuilder {
        StatsBuilder { rows: 0, bytes: 0, columns: vec![ColumnBuilder::sampling(sampling); columns], sample: RowReservoir::default() }
    }

    /// As `sampling`, for rows that are a sample of a table's, to be
    /// finished by `finish_sampled`: each value counted as it is seen.
    pub fn counting(columns: usize, sampling: Sampling) -> StatsBuilder {
        let column = ColumnBuilder { seen: Some(HashMap::new()), ..ColumnBuilder::sampling(sampling) };
        StatsBuilder { rows: 0, bytes: 0, columns: vec![column; columns], sample: RowReservoir::default() }
    }

    /// Tells it the rows added from now on are of the run of rows `run`,
    /// a sampled pass reading the table a run at a time.
    pub fn at_run(&mut self, run: u64) {
        for column in &mut self.columns {
            column.run = Some(run);
        }
    }

    /// Adds `row`, which takes `bytes` bytes as stored.
    pub fn add(&mut self, row: &Row, bytes: usize) {
        for (column, value) in self.columns.iter_mut().zip(row) {
            column.add(value);
        }
        self.sample.add(row);
        self.rows = self.rows.saturating_add(1);
        self.bytes = self.bytes.saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    }

    /// The statistics of the rows added.
    pub fn finish(self) -> TableStats {
        let width = if self.rows == 0 { 0.0_f64 } else { count(self.bytes) / count(self.rows) };
        let sample = self.sample.finish().map(Arc::new);
        TableStats { rows: self.rows, width, columns: self.columns.into_iter().map(ColumnBuilder::finish).collect(), sample }
    }

    /// The statistics of a table of `rows` rows, those added a sample of
    /// them (`counting`): their width the sample's, each column's as
    /// `ColumnBuilder::finish_scaled` gives.
    pub fn finish_sampled(self, rows: u64) -> TableStats {
        let width = if self.rows == 0 { 0.0_f64 } else { count(self.bytes) / count(self.rows) };
        let columns = self.columns.into_iter().map(|c| c.finish_scaled(count(rows))).collect();
        TableStats { rows, width, columns, sample: self.sample.finish().map(Arc::new) }
    }
}

// Unit tests: the sketch's accuracy, the histogram's fractions, and the
// selectivities they give (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
