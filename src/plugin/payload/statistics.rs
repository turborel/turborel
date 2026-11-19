//! \[DESIGN\] Statistics in protocol 1, both ways (RULES.md D278, D279):
//! what a plug-in's engine knows of a table's rows and its method of an
//! index's entries, given the planner, and what the database keeps of a
//! table, a column or an index, given a plug-in that asks.
//!
//! ```text
//! engine STATISTICS (op 8)  locator bytes   -> table
//! index STATISTICS (op 9)   locator bytes   -> index
//! table:   u8 0 (none known) | u8 1 | rows f64 | width f64 | refresh u64 | columns: count u32 | column...
//! column:  distinct f64 | nulls f64 | bounds: u8 0, or u8 1 | low value | high value
//!          | histogram: count u32 | value... | frequencies: u8 0, or u8 1 | most f64 | median f64 | p90 f64 | p99 f64
//! index:   u8 0 (none known) | u8 1 | entries f64 | distinct keys f64 | pages f64 | refresh u64
//! asked:   u8 1 table | name str;  u8 2 column | table name str | column name str;  u8 3 index | name str
//! ```
//!
//! A number not known is negative. A histogram's values are its bounds,
//! ascending, each bucket between two holding as many values. `refresh`
//! is how many rows or entries written make the answer stale, so the
//! database asks again; 0 is never, until ANALYZE or maintenance asks.

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::stats::{ColumnStats, Frequencies, Histogram, Statistics};
use crate::value::{CompOp, Value};

use super::super::values::{put_value, value};
use super::{count, put_f64, read_f64};

/// What a table's statistics hold, as a plug-in gives or is given them.
#[derive(Debug, Clone, Default)]
pub struct TableAnswer {
    /// Its rows; negative if not known.
    pub rows: f64,
    /// A row's width in bytes; negative if not known.
    pub width: f64,
    /// The rows written that make it stale; 0 never.
    pub refresh: u64,
    /// Each column's, in order.
    pub columns: Vec<ColumnAnswer>,
}

/// What a column's statistics hold.
#[derive(Debug, Clone, Default)]
pub struct ColumnAnswer {
    /// Its distinct non-null values; negative if not known.
    pub distinct: f64,
    /// The share of its values that are null; negative if not known.
    pub nulls: f64,
    /// Its least and greatest values.
    pub bounds: Option<(Value, Value)>,
    /// Its equi-depth histogram's bounds, ascending; empty if none.
    pub histogram: Vec<Value>,
    /// How often its values repeat.
    pub frequencies: Option<Frequencies>,
}

/// What an index's statistics hold.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct IndexAnswer {
    /// Its entries; negative if not known.
    pub entries: f64,
    /// Its distinct keys; negative if not known.
    pub distinct: f64,
    /// The pages a probe reads; negative if not known.
    pub pages: f64,
    /// The entries written that make it stale; 0 never.
    pub refresh: u64,
}

/// What a plug-in asks the statistics of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    /// A table, "SCHEMA.TABLE", and each of its columns.
    Table(String),
    /// A table's column alone.
    Column(String, String),
    /// An index, "SCHEMA.INDEX" or its name.
    Index(String),
}

/// `x`, if known.
pub fn known(x: f64) -> Option<f64> {
    (x.is_finite() && x >= 0.0_f64).then_some(x)
}

/// `x`, or a number not known.
pub fn or_unknown(x: Option<f64>) -> f64 {
    x.and_then(known).unwrap_or(-1.0_f64)
}

/// Writes `t`, none if not known.
///
/// # Errors
///
/// SQLSTATE 38T03 of a bound no protocol 1 carries.
pub fn put_table(out: &mut Vec<u8>, t: Option<&TableAnswer>) -> Result<()> {
    let Some(t) = t else {
        out.put_u8(0);
        return Ok(());
    };
    out.put_u8(1);
    put_f64(out, t.rows);
    put_f64(out, t.width);
    out.put_u64(t.refresh);
    out.put_u32(count(t.columns.len()));
    for c in &t.columns {
        put_f64(out, c.distinct);
        put_f64(out, c.nulls);
        out.put_u8(u8::from(c.bounds.is_some()));
        if let Some((low, high)) = &c.bounds {
            put_value(out, low)?;
            put_value(out, high)?;
        }
        out.put_u32(count(c.histogram.len()));
        c.histogram.iter().try_for_each(|v| put_value(out, v))?;
        out.put_u8(u8::from(c.frequencies.is_some()));
        if let Some(f) = c.frequencies {
            for x in [f.most, f.median, f.p90, f.p99] {
                put_f64(out, x);
            }
        }
    }
    Ok(())
}

/// What `put_table` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn table(r: &mut Reader<'_>) -> Result<Option<TableAnswer>> {
    if r.u8()? == 0 {
        return Ok(None);
    }
    let (rows, width, refresh) = (read_f64(r)?, read_f64(r)?, r.u64()?);
    let columns = (0..r.u32()?).map(|_| column(r)).collect::<Result<_>>()?;
    Ok(Some(TableAnswer { rows, width, refresh, columns }))
}

/// A column's statistics.
fn column(r: &mut Reader<'_>) -> Result<ColumnAnswer> {
    let (distinct, nulls) = (read_f64(r)?, read_f64(r)?);
    let bounds = if r.u8()? == 0 { None } else { Some((value(r)?, value(r)?)) };
    let histogram = (0..r.u32()?).map(|_| value(r)).collect::<Result<_>>()?;
    let frequencies =
        if r.u8()? == 0 { None } else { Some(Frequencies { most: read_f64(r)?, median: read_f64(r)?, p90: read_f64(r)?, p99: read_f64(r)? }) };
    Ok(ColumnAnswer { distinct, nulls, bounds, histogram, frequencies })
}

/// Writes `i`, none if not known.
pub fn put_index(out: &mut Vec<u8>, i: Option<&IndexAnswer>) {
    let Some(i) = i else {
        out.put_u8(0);
        return;
    };
    out.put_u8(1);
    for x in [i.entries, i.distinct, i.pages] {
        put_f64(out, x);
    }
    out.put_u64(i.refresh);
}

/// What `put_index` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn index(r: &mut Reader<'_>) -> Result<Option<IndexAnswer>> {
    if r.u8()? == 0 {
        return Ok(None);
    }
    Ok(Some(IndexAnswer { entries: read_f64(r)?, distinct: read_f64(r)?, pages: read_f64(r)?, refresh: r.u64()? }))
}

/// Writes `a`.
pub fn put_asked(out: &mut Vec<u8>, a: &Asked) {
    match a {
        Asked::Table(t) => {
            out.put_u8(1);
            out.put_str(t);
        }
        Asked::Column(t, c) => {
            out.put_u8(2);
            out.put_str(t);
            out.put_str(c);
        }
        Asked::Index(i) => {
            out.put_u8(3);
            out.put_str(i);
        }
    }
}

/// What `put_asked` wrote.
///
/// # Errors
///
/// `Reader`'s, or SQLSTATE 38T03 of a kind of question there is not.
pub fn asked(r: &mut Reader<'_>) -> Result<Asked> {
    Ok(match r.u8()? {
        1 => Asked::Table(r.str()?),
        2 => Asked::Column(r.str()?, r.str()?),
        3 => Asked::Index(r.str()?),
        other => return Err(super::super::errors::violation(&format!("no statistics are asked of kind {other}"))),
    })
}

/// What `stats` keeps of its table of `columns` columns, as a plug-in is
/// told it.
pub fn answer_of(stats: &dyn Statistics, columns: usize) -> TableAnswer {
    let all: Vec<usize> = (0..columns).collect();
    let columns = (0..columns)
        .map(|c| ColumnAnswer {
            distinct: or_unknown(stats.distinct(c)),
            nulls: or_unknown(stats.nulls(c)),
            bounds: stats.bounds(c),
            histogram: stats.histogram(c).unwrap_or_default(),
            frequencies: stats.frequencies(c),
        })
        .collect();
    TableAnswer { rows: or_unknown(stats.rows()), width: or_unknown(stats.width(&all)), refresh: 0, columns }
}

impl Statistics for TableAnswer {
    fn rows(&self) -> Option<f64> {
        known(self.rows)
    }
    fn width(&self, _columns: &[usize]) -> Option<f64> {
        known(self.width)
    }
    fn distinct(&self, column: usize) -> Option<f64> {
        known(self.columns.get(column)?.distinct)
    }
    fn nulls(&self, column: usize) -> Option<f64> {
        known(self.columns.get(column)?.nulls)
    }
    fn selectivity(&self, column: usize, op: CompOp, value: &Value) -> Option<f64> {
        let c = self.columns.get(column)?;
        let (min, max) = c.bounds.clone().unzip();
        let stats = ColumnStats {
            distinct: known(c.distinct)?,
            nulls: known(c.nulls).unwrap_or(0.0_f64),
            min,
            max,
            histogram: Histogram::of_bounds(c.histogram.clone()),
            unbounded: true,
            frequencies: c.frequencies,
            envelopes: None,
        };
        Some(stats.selectivity(op, value))
    }
    fn frequencies(&self, column: usize) -> Option<Frequencies> {
        self.columns.get(column)?.frequencies
    }
    fn bounds(&self, column: usize) -> Option<(Value, Value)> {
        self.columns.get(column)?.bounds.clone()
    }
    fn histogram(&self, column: usize) -> Option<Vec<Value>> {
        Some(self.columns.get(column)?.histogram.clone()).filter(|h| !h.is_empty())
    }
}

#[cfg(test)]
mod tests;
