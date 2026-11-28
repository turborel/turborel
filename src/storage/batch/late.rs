//! \[DESIGN\] (RULES.md D361) A column of a column store's frozen rows not
//! yet decoded, carried through the operators that drop rows, a filter
//! and an inner join, and decoded once they are done, of the rows left
//! alone (Abadi, Myers, DeWitt and Madden, "Materialization Strategies in a
//! Column-Oriented DBMS", ICDE 2007, on late materialization). It holds where
//! its rows are in their group and how to decode them (`LateSource`), and,
//! once a join gathered it, which of those each row is (`picks`), composed
//! without a value decoded.
//!
//! An operator that reads a column has it decoded first (`Batch::resolve`),
//! of the rows a filter kept alone, the others left null, as no one reads
//! them. A reader that reads it before, which the executor never plans
//! (`exec/batch/late.rs`), still reads the values the scan would have
//! given, the column then decoded whole (`made`).
//!
//! \[DESIGN\] (RULES.md D1267) A late column may hold the octet length
//! of each of its values, taken by its scan with no text made
//! (`late/measured.rs`). It is given so where every reader takes the
//! length of the column and none reads its text, and it is then left
//! late past the stream's end, never decoded.

use std::sync::{Arc, OnceLock};

use super::mask::index;
use super::{Batch, Column, Value};
use crate::error::Result;

/// \[DESIGN\] (RULES.md D361) Where a late column's values are decoded
/// from. It is a storage foundation trait (RULES.md D30), so a `dyn`
/// boundary (R-SAFE-3). Its implementation keeps what it reads readable while it
/// lives, as the scan that made it keeps it.
pub trait LateSource: Send + Sync {
    /// Column `column` of the rows at `positions`, ascending, decoded as
    /// the scan would have decoded them, one value each.
    ///
    /// # Errors
    ///
    /// As reading what it reads fails: a damaged page.
    fn decode(&self, column: usize, positions: &[u32]) -> Result<Column>;
}

/// \[DESIGN\] (RULES.md D361) A column not yet decoded: of each row, its
/// place in its source.
#[derive(Clone)]
pub struct Late {
    source: Arc<dyn LateSource>,
    column: usize,
    /// The places in the source of the rows it was made of, ascending.
    positions: Arc<[u32]>,
    /// Of each row, which of `positions` it is, or none for a null: as a
    /// join gathered it. Without them row `n` is the `n`th.
    picks: Option<Arc<[Option<u32>]>>,
    /// Its values decoded whole, made the first time a reader that did
    /// not resolve it asks for them; none if decoding failed.
    made: OnceLock<Option<Box<Column>>>,
    /// \[DESIGN\] (RULES.md D1267) The octet length of the value at each
    /// of `positions`, if its scan measured them (`measured.rs`).
    lengths: Option<Arc<super::Ints>>,
}

impl std::fmt::Debug for Late {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Late").field("column", &self.column).field("rows", &self.len()).finish_non_exhaustive()
    }
}

impl Late {
    /// Column `column` of `source`, of the rows at `positions`, ascending.
    pub fn new(source: Arc<dyn LateSource>, column: usize, positions: Arc<[u32]>) -> Late {
        Late { source, column, positions, picks: None, made: OnceLock::new(), lengths: None }
    }

    /// How many rows.
    pub fn len(&self) -> usize {
        self.picks.as_ref().map_or(self.positions.len(), |p| p.len())
    }

    /// Whether it has no rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Which of `positions` row `r` is, if it is one.
    fn place(&self, r: usize) -> Option<usize> {
        match &self.picks {
            Some(picks) => picks.get(r).copied().flatten().map(index),
            None => (r < self.positions.len()).then_some(r),
        }
    }

    /// \[DESIGN\] Its values of the rows `rows`, or of all, decoded, the
    /// others null: each place in the source decoded once, ascending, then
    /// gathered to the rows, a row taken as often as a join paired it.
    ///
    /// # Errors
    ///
    /// As `LateSource::decode`.
    pub fn resolved(&self, rows: Option<&[u32]>, kernels: &dyn crate::kernels::Kernels) -> Result<Column> {
        if self.picks.is_none() && rows.is_none() {
            return self.source.decode(self.column, &self.positions);
        }
        if let (None, Some(rows)) = (&self.picks, rows)
            && rows.is_sorted_by(|a, b| a < b)
        {
            return self.kept(rows, kernels);
        }
        let all: Vec<u32>;
        let rows = if let Some(rows) = rows {
            rows
        } else {
            all = super::sel::rows_of(0..self.len());
            &all
        };
        let mut wanted: Vec<u32> =
            rows.iter().filter_map(|&r| self.place(index(r))).filter_map(|p| self.positions.get(p).copied()).collect();
        if !wanted.is_sorted() {
            wanted.sort_unstable();
        }
        wanted.dedup();
        let decoded = self.source.decode(self.column, &wanted)?;
        let mut picks = vec![super::NO_ROW; self.len()];
        for &r in rows {
            let at = self.place(index(r)).and_then(|p| self.positions.get(p)).and_then(|q| wanted.binary_search(q).ok());
            if let Some(slot) = picks.get_mut(index(r)) {
                *slot = at.and_then(|a| u32::try_from(a).ok()).unwrap_or(super::NO_ROW);
            }
        }
        Ok(decoded.gathered_by(&super::Picks::new(&picks), kernels))
    }

    /// \[DESIGN\] (RULES.md D1162) Of a column no join gathered, its values
    /// of the rows `rows`, ascending and each once, the others null: their
    /// places are ascending and each once too, so the `k`th row kept is the
    /// `k`th place decoded, none searched for among them.
    ///
    /// # Errors
    ///
    /// As `LateSource::decode`.
    fn kept(&self, rows: &[u32], kernels: &dyn crate::kernels::Kernels) -> Result<Column> {
        let wanted: Vec<u32> = rows.iter().filter_map(|&r| self.positions.get(index(r)).copied()).collect();
        let decoded = self.source.decode(self.column, &wanted)?;
        let mut picks = vec![super::NO_ROW; self.len()];
        let mut next = 0_u32;
        for &r in rows {
            if let Some(slot) = picks.get_mut(index(r)) {
                *slot = next;
                next = next.saturating_add(1);
            }
        }
        Ok(decoded.gathered_by(&super::Picks::new(&picks), kernels))
    }

    /// Its values decoded whole, made once: for a reader that did not
    /// resolve it, none if decoding failed.
    pub fn made(&self) -> Option<&Column> {
        self.made.get_or_init(|| self.resolved(None, &crate::kernels::Portable).ok().map(Box::new)).as_deref()
    }

    /// The rows at `picks`, in order, a null for none: still late, its
    /// picks composed with these.
    #[must_use]
    pub fn gathered(&self, picks: &[Option<usize>]) -> Late {
        let composed: Vec<Option<u32>> =
            picks.iter().map(|p| p.and_then(|r| self.place(r)).and_then(|place| u32::try_from(place).ok())).collect();
        Late {
            source: Arc::clone(&self.source),
            column: self.column,
            positions: Arc::clone(&self.positions),
            picks: Some(composed.into()),
            made: OnceLock::new(),
            lengths: self.lengths.clone(),
        }
    }

    /// The bytes it holds, near enough to bound a work area by.
    pub fn bytes(&self) -> usize {
        let picks = self.picks.as_ref().map_or(0, |p| p.len().saturating_mul(size_of::<Option<u32>>()));
        let lengths = self.lengths.as_ref().map_or(0, |l| l.units.len().saturating_mul(size_of::<i64>()));
        self.positions.len().saturating_mul(size_of::<u32>()).saturating_add(picks).saturating_add(lengths)
    }

    /// Its values, made the first time they are asked for: none if
    /// decoding failed.
    pub fn values(&self) -> &[Value] {
        self.made().map_or(&[][..], Column::values)
    }
}

impl Batch {
    /// \[DESIGN\] (RULES.md D361) Decodes each late column of the rows
    /// `rows`, or of all, the others null, so it is read as the scan would
    /// have given it, its integers gathered by `kernels` (RULES.md D570).
    /// A column its scan measured is left late, as its reader takes its
    /// lengths and no text of it (RULES.md D1267).
    ///
    /// # Errors
    ///
    /// As `LateSource::decode`.
    pub fn resolve(&mut self, rows: Option<&[u32]>, kernels: &dyn crate::kernels::Kernels) -> Result<()> {
        for column in &mut self.columns {
            if let Column::Late(late) = column
                && !late.is_measured()
            {
                *column = late.resolved(rows, kernels)?;
            }
        }
        Ok(())
    }

    /// Whether a column of it is late.
    pub fn has_late(&self) -> bool {
        self.columns.iter().any(|c| matches!(c, Column::Late(_)))
    }
}

mod measured;

// Unit tests: a late column decoded of the rows kept, gathered and read
// before (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
