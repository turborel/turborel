//! \[DESIGN\] A row table's batches read in place (RULES.md D350): each
//! row's newest version read where it lies in its leaf, under the leaf's
//! latch (`tree/scan/visit.rs`), and the columns asked for gathered
//! straight from its record into their vectors (`gathered.rs`), so a row
//! the reader sees costs no copy of its key, version or record, and a
//! count of rows reads no column at all.
//!
//! What cannot be read under a latch is copied out and read after it, as
//! a scan of copies reads every row: a version the reader does not see,
//! whose older ones are in the undo store, or whose maker is to be looked
//! up in the table of late stamps, whose lock is not taken under a latch;
//! a sealed record, opened by its encodings, a plug-in's call (RULES.md
//! R-ERR-4); and a record with a column asked for kept out of line, read
//! from other pages. Rows come in row id order either way.

use std::iter;
use std::sync::Arc;

use super::gathered::Gathering;
use super::mvcc::Reader;
use super::scan::Ranged;
use super::sealing::sealed;
use super::version::Version;
use super::{RowTable, row_id};
use crate::engine::record::not_fetched;
use crate::engine::tree::{Visit, Visited};
use crate::error::Result;
use crate::storage::{Batches, Ramp, RowId};

impl RowTable {
    /// The batches of the rows `reader` sees in `ranges` of row ids, each
    /// from its first id up to, not including, its end if it has one, of
    /// `columns`: each record read in place where it can be.
    pub(super) fn gathered_in(self: Arc<Self>, reader: Reader, columns: &[usize], ranges: Vec<(RowId, Option<RowId>)>) -> Batches {
        let mut entries = Self::ranged(&self, ranges);
        let mut gathering = Gathering::new(self.format.types(), columns);
        let mut ramp = Ramp::default();
        Box::new(iter::from_fn(move || {
            let rows = ramp.next_rows();
            if let Err(e) = self.fill(&mut entries, reader, (&mut gathering, rows)) {
                // The rows gathered are given up with the scan.
                gathering.take();
                return Some(Err(e));
            }
            (gathering.rows() > 0).then(|| Ok(gathering.take()))
        }))
    }

    /// Gathers into `gathering` the rows `reader` sees of `entries`, until
    /// it holds `rows` or none is left.
    fn fill(&self, entries: &mut Ranged, reader: Reader, (gathering, rows): (&mut Gathering, usize)) -> Result<()> {
        while gathering.rows() < rows {
            let visited = entries.visit(|newest| self.in_place(newest, reader, (gathering, rows)))?;
            match visited {
                Visited::More => {}
                Visited::Later((key, newest)) => {
                    if let Some(record) = self.visible_from(newest, reader)? {
                        self.gather_record((row_id(&key)?, &record), gathering)?;
                    }
                }
                Visited::Done => return Ok(()),
            }
        }
        Ok(())
    }

    /// What a scan does with a row's newest version, `newest`, under its
    /// leaf's latch: gathers the row `reader` sees into `gathering`, unless
    /// it holds `rows` already, or the row is read after the latch (see the
    /// module's documentation).
    fn in_place(&self, newest: &[u8], reader: Reader, (gathering, rows): (&mut Gathering, usize)) -> Result<Visit> {
        if gathering.rows() >= rows {
            return Ok(Visit::Stop);
        }
        let version = Version::parse(newest)?;
        // Its maker as stamped: a commit stamped late is read after the latch.
        if !reader.sees(version.maker) {
            return Ok(Visit::Later);
        }
        if version.deleted {
            return Ok(Visit::Taken);
        }
        if gathering.reads_any() && (sealed(version.record) || !gathering.inline(&self.format, version.record)?) {
            return Ok(Visit::Later);
        }
        gathering.push((&self.format, version.record), &mut not_fetched, Some(&self.lobs))?;
        Ok(Visit::Taken)
    }
}

// Unit tests: batches read in place are those of the rows the reader sees,
// as a scan of copies gives them (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
