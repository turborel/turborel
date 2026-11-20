//! \[DESIGN\] The chunks of the row group a scan reads, each opened once
//! for the runs it reads of it (RULES.md D352): a run of a scan is at most
//! a vector of rows, and a chunk compressed holds many more, so each run
//! reopening, unsealing and parsing the chunks it touches read a chunk of
//! 65,536 rows about 32 times a column. A chunk read is kept here, by its
//! column and its first row, its bytes opened (owned, as the page it is on
//! is not held), its table of symbols read and, of a dictionary, its
//! entries made integers or text, so the runs after read it by parsing
//! its few header bytes alone.
//!
//! A scan reads a group's runs in order, so a chunk ended before the one
//! read is not read again: it is let go then, and a column keeps at most
//! `KEPT` chunks, so what is held is about a run's chunks a column, never
//! a whole group. The scan lets every chunk go as it takes up the next
//! group (`clear`). What is held is one run's metadata and bytes, so it is
//! charged to no work grant.

use std::collections::{HashMap, VecDeque};

use super::encoded::Entries;
use super::fsst::Symbols;
use super::group::{Pages, Seal, Zone};
use super::{Chunk, padding};
use crate::error::Result;
use crate::value::DataType;

/// \[DESIGN\] The most chunks of a column held: a run of 2,048 rows spans
/// one or two of a compressed column, and of a plain one, up to a few
/// dozen of 8,192-byte pages of wide values.
const KEPT: usize = 64;

/// A chunk opened, and what is made of it once.
struct Held {
    /// Its first row in the group, and the row past its last.
    first: u32,
    end: u32,
    bytes: Vec<u8>,
    table: Option<(Box<Symbols>, usize)>,
    made: Made,
}

/// Of a chunk, what its runs share: of a dictionary, its entries as
/// integers or as text, as its column's type reads them.
#[derive(Default)]
pub struct Made {
    pub(super) words: Option<Vec<i64>>,
    pub(super) entries: Option<Entries>,
}

/// \[DESIGN\] The chunks opened of the group being read.
#[derive(Default)]
pub struct Prepared {
    columns: HashMap<usize, VecDeque<Held>>,
    /// How many chunks were opened, as a test counts them.
    #[cfg(test)]
    pub(super) opened: usize,
}

impl Prepared {
    /// Lets every chunk go, as the next group is taken up.
    pub fn clear(&mut self) {
        self.columns.clear();
    }

    /// What `read` makes of the chunk `zone` of column `column` of type
    /// `t`, and what is made of it once: kept if read before, else read
    /// of `pages`, the chunks of the column ended before it let go.
    ///
    /// # Errors
    ///
    /// As `Pages::with_run`; SQLCODE -916 if the chunk is malformed;
    /// `read`'s.
    pub fn with<S: Seal, T>(
        &mut self,
        pages: &Pages<'_, S>,
        (column, zone, t): (usize, &Zone, &DataType),
        read: impl FnOnce(&Chunk<'_>, &Made) -> Result<T>,
    ) -> Result<T> {
        let first = zone.run.first;
        let kept = self.columns.entry(column).or_default();
        let at = match kept.iter().position(|h| h.first == first) {
            Some(at) => at,
            None => {
                while kept.front().is_some_and(|h| h.end <= first) || kept.len() >= KEPT {
                    kept.pop_front();
                }
                kept.push_back(open(pages, zone, t)?);
                #[cfg(test)]
                {
                    self.opened = self.opened.saturating_add(1);
                }
                kept.len().saturating_sub(1)
            }
        };
        let held = kept.get(at).ok_or_else(|| super::corrupt("a chunk held is gone"))?;
        let chunk = Chunk::parse_with(&held.bytes, held.table.as_ref().map(|(s, len)| (&**s, *len)))?;
        read(&chunk, &held.made)
    }
}

/// The chunk `zone`, of a column of type `t`, read of `pages`, and what
/// its runs share made of it.
fn open<S: Seal>(pages: &Pages<'_, S>, zone: &Zone, t: &DataType) -> Result<Held> {
    let bytes = pages.with_run(&zone.run, |b| Ok(b.to_vec()))?;
    let chunk = Chunk::parse(&bytes)?;
    let table = chunk.table();
    // Entries as text of a character string type, else as integers.
    let made = match padding(t) {
        Some(pad) => Made { words: None, entries: chunk.body.entry_texts(pad) },
        None => Made { words: chunk.body.entry_words(), entries: None },
    };
    Ok(Held { first: zone.run.first, end: zone.run.first.saturating_add(zone.run.rows), bytes, table, made })
}

impl Chunk<'_> {
    /// As `ints`, of what is made of it once, `made`.
    pub(super) fn ints_made(&self, (from, to): (usize, usize), outs: (&mut Vec<i64>, &mut Vec<bool>), made: &Made) -> Option<()> {
        self.ints_with((from, to), outs, made.words.as_deref())
    }

    /// As `coded`, of what is made of it once, `made`: none if it is not
    /// a dictionary of texts.
    pub(super) fn coded_made(
        &self,
        positions: &[usize],
        outs: (&mut String, &mut Vec<usize>, &mut Vec<u32>, &mut Vec<bool>),
        made: &Made,
    ) -> Option<()> {
        self.coded_with(positions, outs, made.entries.as_ref()?)
    }
}

#[cfg(test)]
mod tests;
