//! \[DESIGN\] A load's rows by column, as text (RULES.md D1148): of each
//! column, its cells' text end to end, where each ends, and which are
//! null. A bulk copy of the `columns` capability sends them so; the
//! server parses each column into its type, so no row of values is made
//! for a load that goes straight to groups.

use crate::error::{Code, Result, SqlError};
use crate::storage::Row;
use crate::value::{CharString, Value};

mod parse;
pub use parse::parse;

/// The cells of one column, as text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lane {
    /// A bit set for each null cell, the first cell's the low bit of the
    /// first byte.
    pub nulls: Vec<u8>,
    /// Where each cell's text ends in `bytes`, a null's where the one
    /// before it does.
    pub ends: Vec<u32>,
    /// The cells' text, end to end, UTF-8.
    pub bytes: Vec<u8>,
}

/// The rows of a load, by column.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lanes {
    /// How many rows.
    pub rows: usize,
    /// Each column's cells, in the copy's order of columns.
    pub columns: Vec<Lane>,
}

/// \[DESIGN\] The values of one column of a table, each as its type
/// writes it in a record (`SqlType::encode_record`), end to end: a lane
/// parsed into its type (RULES.md D1148).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Records {
    /// Each value's record, end to end; a null has none.
    pub bytes: Vec<u8>,
    /// Where each value's record ends in `bytes`, a null's where the one
    /// before it does.
    pub ends: Vec<u32>,
    /// A bit set for each null, as `Lane::nulls`.
    pub nulls: Vec<u8>,
}

impl Records {
    /// How many values.
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// Whether value `n` is null.
    pub fn is_null(&self, n: usize) -> bool {
        is_null_in(&self.nulls, n)
    }

    /// The record of value `n`, none if it is null or out of range.
    pub fn record(&self, n: usize) -> Option<&[u8]> {
        if self.is_null(n) {
            return None;
        }
        let start = n.checked_sub(1).map_or(Some(0), |p| self.ends.get(p).copied())?;
        let end = *self.ends.get(n)?;
        self.bytes.get(usize::try_from(start).ok()?..usize::try_from(end).ok()?)
    }
}

/// Whether the bit of value `n` is set in the null bitmap `nulls`.
pub fn is_null_in(nulls: &[u8], n: usize) -> bool {
    nulls.get(n >> 3_u32).is_some_and(|b| b & (1_u8 << (n & 7)) != 0)
}

/// Sets the bit of value `n` in the null bitmap `nulls`, if it has one.
pub fn set_null(nulls: &mut [u8], n: usize) {
    if let Some(b) = nulls.get_mut(n >> 3_u32) {
        *b |= 1_u8 << (n & 7);
    }
}

fn malformed(why: &str) -> SqlError {
    SqlError::new(Code::Dynamic, format!("[DESIGN] a copy's columns are malformed: {why}"))
}

impl Lane {
    /// Whether cell `n` is null.
    pub fn is_null(&self, n: usize) -> bool {
        is_null_in(&self.nulls, n)
    }

    /// The text of cell `n`, none if it is null.
    ///
    /// # Errors
    ///
    /// SQLCODE -804 if its bounds are out of order or out of range, or it
    /// is not UTF-8.
    pub fn cell(&self, n: usize) -> Result<Option<&str>> {
        if self.is_null(n) {
            return Ok(None);
        }
        let start = n.checked_sub(1).map_or(Some(0), |p| self.ends.get(p).copied()).ok_or_else(|| malformed("a cell has no end"))?;
        let end = self.ends.get(n).copied().ok_or_else(|| malformed("a cell has no end"))?;
        let (start, end) = (usize::try_from(start).unwrap_or(usize::MAX), usize::try_from(end).unwrap_or(usize::MAX));
        let bytes = self.bytes.get(start..end).ok_or_else(|| malformed("a cell ends out of range"))?;
        std::str::from_utf8(bytes).map(Some).map_err(|_| malformed("a cell is not UTF-8"))
    }

    /// Checks it holds `rows` cells.
    ///
    /// # Errors
    ///
    /// SQLCODE -804 if it holds another number of ends or of null bits.
    pub fn check(&self, rows: usize) -> Result<()> {
        if self.ends.len() != rows || self.nulls.len() != rows.div_ceil(8) {
            return Err(malformed("a column has another number of cells than the copy has rows"));
        }
        Ok(())
    }
}

impl Lane {
    /// Its cells from `from`, `count` of them, as a lane of their own.
    fn part(&self, from: usize, count: usize) -> Lane {
        let start = from.checked_sub(1).and_then(|p| self.ends.get(p)).copied().unwrap_or(0);
        let mut part = Lane { nulls: vec![0; count.div_ceil(8)], ..Lane::default() };
        for n in 0..count {
            let end = self.ends.get(from.saturating_add(n)).copied().unwrap_or(start);
            part.ends.push(end.saturating_sub(start));
            if self.is_null(from.saturating_add(n)) {
                set_null(&mut part.nulls, n);
            }
        }
        let last = part.ends.last().copied().unwrap_or(0);
        let (start, last) = (usize::try_from(start).unwrap_or(0), usize::try_from(last).unwrap_or(0));
        part.bytes = self.bytes.get(start..start.saturating_add(last)).unwrap_or_default().to_vec();
        part
    }

    /// Appends the `rows` cells of `other` after its own `held`.
    fn append(&mut self, held: usize, other: &Lane, rows: usize) -> Result<()> {
        let base = u32::try_from(self.bytes.len()).map_err(|_| malformed("a column is too long"))?;
        self.bytes.extend_from_slice(&other.bytes);
        self.nulls.resize(held.saturating_add(rows).div_ceil(8), 0);
        for n in 0..rows {
            let end = other.ends.get(n).copied().unwrap_or(0).checked_add(base).ok_or_else(|| malformed("a column is too long"))?;
            self.ends.push(end);
            let at = held.saturating_add(n);
            if other.is_null(n) {
                set_null(&mut self.nulls, at);
            }
        }
        Ok(())
    }
}

impl Lanes {
    /// Appends the rows of `other` after its own, column by column.
    ///
    /// # Errors
    ///
    /// SQLCODE -804 if `other` has another number of columns, or is
    /// malformed, or a column grows too long.
    pub fn append(&mut self, other: &Lanes) -> Result<()> {
        if self.rows == 0 && self.columns.is_empty() {
            self.columns = vec![Lane::default(); other.columns.len()];
        }
        if other.columns.len() != self.columns.len() {
            return Err(malformed("a part of a copy has another number of columns"));
        }
        for (lane, more) in self.columns.iter_mut().zip(&other.columns) {
            more.check(other.rows)?;
            lane.append(self.rows, more, other.rows)?;
        }
        self.rows = self.rows.saturating_add(other.rows);
        Ok(())
    }

    /// Its first `at` rows, the rest left in it.
    #[must_use]
    pub fn split_front(&mut self, at: usize) -> Lanes {
        let at = at.min(self.rows);
        let rest = self.rows.saturating_sub(at);
        let front = Lanes { rows: at, columns: self.columns.iter().map(|l| l.part(0, at)).collect() };
        self.columns = self.columns.iter().map(|l| l.part(at, rest)).collect();
        self.rows = rest;
        front
    }

    /// Its rows, each a row of values: a null, or the cell's text as a
    /// character string, as a copy's rows of text are sent.
    ///
    /// # Errors
    ///
    /// As `Lane::check` and `Lane::cell`.
    pub fn to_rows(&self) -> Result<Vec<Row>> {
        for lane in &self.columns {
            lane.check(self.rows)?;
        }
        (0..self.rows)
            .map(|n| {
                self.columns
                    .iter()
                    .map(|lane| Ok(lane.cell(n)?.map_or(Value::Null, |s| Value::Char(CharString::from(s.to_owned())))))
                    .collect()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
