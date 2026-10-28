//! \[DESIGN\] Statistics for the optimizer (RULES.md D31): what a table's
//! rows are like, so the planner can estimate how many rows a condition
//! keeps and what reading them costs.
//!
//! What an engine knows about its rows depends on how it stores them, so
//! the planner asks through a trait, `Statistics`, every answer optional:
//! an engine answers what it knows and the planner falls back to defaults
//! for the rest. A row store samples whole rows in a pass; a columnar
//! engine may know distinct values exactly from its dictionaries, least and
//! greatest values from each segment, and the width of each column apart,
//! and answers from those.
//!
//! The parts statistics are built of (`parts`) each take one column's
//! values, so an engine feeds them a row at a time or a column at a time.
//! `TableStats` is one way to answer from them: per column, the distinct
//! values, the fraction that is NULL, the least and greatest values, and a
//! histogram. `ColumnBuilder` gathers a column's; `StatsBuilder` gathers a
//! table's from its rows.

pub mod parts;

use std::cmp::Ordering;
use std::fmt::Debug;

pub use parts::Sampling;
use parts::{Histogram, HyperLogLog, Reservoir, count};

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
    /// Its non-null values' histogram.
    pub histogram: Histogram,
}

impl ColumnStats {
    /// The fraction of rows for which "column `op` `value`" is true, a
    /// comparison with NULL being true for none [X3.135-1986 §5.11 GR2]:
    /// for equality, the non-null rows shared among the distinct values,
    /// none outside the least and greatest; for an order, the histogram.
    pub fn selectivity(&self, op: CompOp, value: &Value) -> f64 {
        if value.is_null() {
            return 0.0;
        }
        let present = (1.0_f64 - self.nulls).clamp(0.0, 1.0);
        let outside = self.min.as_ref().is_some_and(|min| compare(value, min) == Some(Ordering::Less))
            || self.max.as_ref().is_some_and(|max| compare(value, max) == Some(Ordering::Greater));
        let equal = if outside || self.min.is_none() { 0.0_f64 } else { present / self.distinct.max(1.0) };
        let below = present * self.histogram.below(value).unwrap_or(0.5_f64);
        let fraction = match op {
            CompOp::Eq => equal,
            CompOp::Ne => present - equal,
            CompOp::Lt => below,
            CompOp::Le => below + equal,
            CompOp::Gt => present - below - equal,
            CompOp::Ge => present - below,
        };
        fraction.clamp(0.0, 1.0)
    }
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
        self.sample.add(value);
        if self.min.as_ref().is_none_or(|min| compare(value, min) == Some(Ordering::Less)) {
            self.min = Some(value.clone());
        }
        if self.max.as_ref().is_none_or(|max| compare(value, max) == Some(Ordering::Greater)) {
            self.max = Some(value.clone());
        }
    }

    /// The statistics of the values added.
    pub fn finish(self) -> ColumnStats {
        let present = self.values.saturating_sub(self.nulls);
        ColumnStats {
            distinct: self.sketch.estimate().min(count(present)),
            nulls: if self.values == 0 { 0.0 } else { count(self.nulls) / count(self.values) },
            min: self.min,
            max: self.max,
            histogram: Histogram::of_sorted(&self.sample.into_sorted(), self.sampling.buckets),
        }
    }
}

/// \[DESIGN\] Gathers a table's statistics from its rows, in one pass.
pub struct StatsBuilder {
    rows: u64,
    bytes: u64,
    columns: Vec<ColumnBuilder>,
}

impl StatsBuilder {
    /// A builder for rows of `columns` columns.
    pub fn new(columns: usize) -> StatsBuilder {
        StatsBuilder::sampling(columns, Sampling::default())
    }

    /// A builder for rows of `columns` columns, sampling as `sampling` says.
    pub fn sampling(columns: usize, sampling: Sampling) -> StatsBuilder {
        StatsBuilder { rows: 0, bytes: 0, columns: vec![ColumnBuilder::sampling(sampling); columns] }
    }

    /// Adds `row`, which takes `bytes` bytes as stored.
    pub fn add(&mut self, row: &Row, bytes: usize) {
        for (column, value) in self.columns.iter_mut().zip(row) {
            column.add(value);
        }
        self.rows = self.rows.saturating_add(1);
        self.bytes = self.bytes.saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    }

    /// The statistics of the rows added.
    pub fn finish(self) -> TableStats {
        let width = if self.rows == 0 { 0.0_f64 } else { count(self.bytes) / count(self.rows) };
        TableStats { rows: self.rows, width, columns: self.columns.into_iter().map(ColumnBuilder::finish).collect() }
    }
}

// Unit tests: the sketch's accuracy, the histogram's fractions, and the
// selectivities they give (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
