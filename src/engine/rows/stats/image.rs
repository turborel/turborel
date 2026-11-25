//! \[DESIGN\] A table's statistics as the bytes its statistics page holds
//! (RULES.md D1093, `stats/saved.rs`): every number a pass found, and
//! every value, a bound, a histogram's bound, a common value or a sampled
//! row's, written as a record of the table's own format (`record.rs`), so
//! a value is read back of its column's type with nothing new to describe
//! it.
//!
//! ```text
//! page:     format u8 (STATISTICS_FORMAT) | rows u64 | width f64 |
//!           columns u32 | each column | least values, record |
//!           greatest values, record | histogram bounds u32, each a record |
//!           common values u32, each a record | sampled u8 0 or 1 |
//!           sampled rows u32, each a record
//! column:   distinct f64 | nulls f64 | rises u8 | flags u8 (UNBOUNDED,
//!           FREQUENT, ENVELOPED) | its histogram's bounds u32 | its common
//!           values u32, each one's share f64 | if FREQUENT, most f64,
//!           median f64, 90th f64, 99th f64 | if ENVELOPED, its envelopes
//!           (`stats/envelopes/kept.rs`)
//! record:   length u32 | a record of the table's columns
//! ```
//!
//! The record numbered n of the histogram's holds each column's bound n,
//! NULL of a column with fewer, and so of the common values; a column
//! with no least value is NULL in both records of bounds. Integers are
//! little-endian, and a number is its IEEE-754 bits.

use std::sync::Arc;

use crate::bytes::{Put, Reader};
use crate::engine::record::RecordFormat;
use crate::error::{Code, Result, SqlError};
use crate::stats::{ColumnStats, EnvelopeStats, Frequencies, Histogram, RowSample, TableStats};
use crate::storage::Row;
use crate::value::Value;

/// The format of a statistics page.
const STATISTICS_FORMAT: u8 = 1;
/// A column's flag: values may lie past its bounds.
const UNBOUNDED: u8 = 1;
/// A column's flag: how often its values repeat follows.
const FREQUENT: u8 = 2;
/// A column's flag: its envelopes' statistics follow.
const ENVELOPED: u8 = 4;

/// What is read of a column before the records: it, and how many of the
/// histogram's and the common values' records hold a value of it.
struct Read {
    column: ColumnStats,
    bounds: usize,
    shares: Vec<f64>,
}

fn put_f64(out: &mut Vec<u8>, x: f64) {
    out.put_u64(x.to_bits());
}

fn f64_of(r: &mut Reader<'_>) -> Result<f64> {
    r.u64().map(f64::from_bits)
}

fn put_count(out: &mut Vec<u8>, n: usize) {
    out.put_u32(u32::try_from(n).unwrap_or(u32::MAX));
}

fn count_of(r: &mut Reader<'_>) -> Result<usize> {
    let n = r.u32()?;
    usize::try_from(n).map_err(|_| r.corrupt("a count past what is held"))
}

/// Appends `row` to `out` as a record of `format`.
fn put_record(format: &RecordFormat, out: &mut Vec<u8>, row: &Row) -> Result<()> {
    let mut record = Vec::new();
    format.encode(row, &mut record)?;
    out.put_bytes(&record);
    Ok(())
}

/// Appends what `column` holds but its values.
fn put_column(out: &mut Vec<u8>, column: &ColumnStats) {
    put_f64(out, column.distinct);
    put_f64(out, column.nulls);
    out.put_u8(column.rises);
    let flag = |set: bool, bit: u8| if set { bit } else { 0 };
    out.put_u8(
        flag(column.unbounded, UNBOUNDED) | flag(column.frequencies.is_some(), FREQUENT) | flag(column.envelopes.is_some(), ENVELOPED),
    );
    put_count(out, column.histogram.bounds().len());
    put_count(out, column.common.len());
    for (_, share) in &column.common {
        put_f64(out, *share);
    }
    if let Some(Frequencies { most, median, p90, p99 }) = column.frequencies {
        for x in [most, median, p90, p99] {
            put_f64(out, x);
        }
    }
    if let Some(envelopes) = &column.envelopes {
        envelopes.put(out);
    }
}

/// What `put_column` wrote.
fn column_of(r: &mut Reader<'_>) -> Result<Read> {
    let (distinct, nulls, rises, flags) = (f64_of(r)?, f64_of(r)?, r.u8()?, r.u8()?);
    let bounds = count_of(r)?;
    let shares = (0..count_of(r)?).map(|_| f64_of(r)).collect::<Result<Vec<f64>>>()?;
    let frequencies = if flags & FREQUENT == 0 {
        None
    } else {
        Some(Frequencies { most: f64_of(r)?, median: f64_of(r)?, p90: f64_of(r)?, p99: f64_of(r)? })
    };
    let envelopes = if flags & ENVELOPED == 0 { None } else { Some(Box::new(EnvelopeStats::read(r)?)) };
    let column =
        ColumnStats { distinct, nulls, rises, unbounded: flags & UNBOUNDED != 0, frequencies, envelopes, ..ColumnStats::default() };
    Ok(Read { column, bounds, shares })
}

/// `stats`, of a table whose records are of `format`, as its statistics
/// page holds them.
///
/// # Errors
///
/// SQLCODE -910 if they are of other columns than the table's; the
/// record format's, of a value it does not write.
pub(super) fn encoded(stats: &TableStats, format: &RecordFormat) -> Result<Vec<u8>> {
    if stats.columns.len() != format.columns() {
        return Err(SqlError::new(Code::Api, "[DESIGN] statistics are kept of the table's own columns"));
    }
    let mut out = vec![STATISTICS_FORMAT];
    out.put_u64(stats.rows);
    put_f64(&mut out, stats.width);
    put_count(&mut out, stats.columns.len());
    for column in &stats.columns {
        put_column(&mut out, column);
    }
    let row_of =
        |of: &dyn Fn(&ColumnStats) -> Option<Value>| -> Row { stats.columns.iter().map(|c| of(c).unwrap_or(Value::Null)).collect() };
    put_record(format, &mut out, &row_of(&|c| c.min.clone()))?;
    put_record(format, &mut out, &row_of(&|c| c.max.clone()))?;
    let bounds = stats.columns.iter().map(|c| c.histogram.bounds().len()).max().unwrap_or(0);
    put_count(&mut out, bounds);
    for at in 0..bounds {
        put_record(format, &mut out, &row_of(&|c| c.histogram.bounds().get(at).cloned()))?;
    }
    let common = stats.columns.iter().map(|c| c.common.len()).max().unwrap_or(0);
    put_count(&mut out, common);
    for at in 0..common {
        put_record(format, &mut out, &row_of(&|c| c.common.get(at).map(|(value, _)| value.clone())))?;
    }
    out.put_u8(u8::from(stats.sample.is_some()));
    let sampled = stats.sample.as_deref().map(RowSample::rows).unwrap_or_default();
    put_count(&mut out, sampled.len());
    sampled.iter().try_for_each(|row| put_record(format, &mut out, row))?;
    Ok(out)
}

/// The rows of the records a count in `r` is followed by.
fn records_of(r: &mut Reader<'_>, format: &RecordFormat) -> Result<Vec<Row>> {
    (0..count_of(r)?).map(|_| format.decode(r.bytes()?)).collect()
}

/// Column `at`'s values of `rows`, of the first `count` of them.
fn values_of(rows: &[Row], at: usize, count: usize, r: &Reader<'_>) -> Result<Vec<Value>> {
    let held = rows.get(..count).ok_or_else(|| r.corrupt("a column of more values than records"))?;
    held.iter().map(|row| row.get(at).cloned().ok_or_else(|| r.corrupt("a record of fewer columns"))).collect()
}

/// The statistics `encoded` wrote as `content`, of a table whose records
/// are of `format`.
///
/// # Errors
///
/// SQLCODE -916 if `content` is not what `encoded` writes of such a
/// table; the record format's.
pub(super) fn decoded(content: &[u8], format: &RecordFormat) -> Result<TableStats> {
    let mut r = Reader::new(content, "statistics of a table");
    if r.u8()? != STATISTICS_FORMAT {
        return Err(r.corrupt("of another format"));
    }
    let (rows, width) = (r.u64()?, f64_of(&mut r)?);
    if count_of(&mut r)? != format.columns() {
        return Err(r.corrupt("of other columns than the table's"));
    }
    let heads = (0..format.columns()).map(|_| column_of(&mut r)).collect::<Result<Vec<Read>>>()?;
    let (least, greatest) = (format.decode(r.bytes()?)?, format.decode(r.bytes()?)?);
    let (bounds, common) = (records_of(&mut r, format)?, records_of(&mut r, format)?);
    let mut columns = Vec::new();
    let ends = least.into_iter().zip(greatest);
    for (at, (head, (min, max))) in heads.into_iter().zip(ends).enumerate() {
        let Read { mut column, bounds: held, shares } = head;
        column.min = (!min.is_null()).then_some(min);
        column.max = (!max.is_null()).then_some(max);
        column.histogram = Histogram::of_bounds(values_of(&bounds, at, held, &r)?);
        column.common = values_of(&common, at, shares.len(), &r)?.into_iter().zip(shares).collect();
        columns.push(column);
    }
    let sampled = r.u8()? != 0;
    let sample = records_of(&mut r, format)?;
    r.finish()?;
    Ok(TableStats { rows, width, columns, sample: sampled.then(|| Arc::new(RowSample::of_rows(sample))) })
}
