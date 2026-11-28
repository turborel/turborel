//! \[DESIGN\] The chunks of the row group a scan reads, each opened once
//! for the runs it reads of it (RULES.md D352): a run of a scan is at most
//! a vector of rows, and a chunk compressed holds many more, so each run
//! reopening, unsealing and parsing the chunks it touches read a chunk of
//! 65,536 rows about 32 times a column. A chunk read is kept here, by its
//! column and its first row, its table of symbols read and, of a
//! dictionary of a type not of character strings, its entries made
//! integers, so the runs after read it by parsing its few header bytes
//! alone.
//!
//! \[DESIGN\] (RULES.md D1298, D1252) Nothing is made of a dictionary of
//! texts as it is opened, and nothing of it is kept. A run that asks for
//! its rows as text copies the entries it reads from the chunk's own
//! octets into the run's vector, where they are judged UTF-8 with the
//! rest of the run (`texts.rs`). A conjunct judged of the chunk where its
//! octets lie, a value read alone and a length taken of its ends open the
//! chunk and ask for no text, so they pay for no check and no copy.
//!
//! The bytes of a chunk whose page passed through no encoding are not
//! kept (RULES.md D1094): each run reads them in the pool, its page held
//! while the run is decoded of it and no longer, so a chunk is never
//! copied to be read. A page of frozen rows is not written again while a
//! reader holds its group, and one found in the pool is found by a hash
//! and a latch, which costs less than copying it. Only the bytes of a
//! chunk whose page had to be opened by its encodings are kept, owned,
//! as opening it once for all its runs is what D352 saves.
//!
//! A scan reads a group's runs in order, so a chunk ended before the one
//! read is not read again: it is let go then, and a column keeps at most
//! `KEPT` chunks, so what is held is about a run's chunks a column, never
//! a whole group. The scan lets every chunk go as it takes up the next
//! group (`clear`). What is held is one run's metadata and bytes, so it is
//! charged to no work grant. The bytes of a chunk let go are kept to hold
//! the next one's, so a scan allocates them about once a column, not once
//! a chunk. The codes of the group's sketches a scan reads are kept here
//! too, with what each code says of each conjunct, until the next group.

use std::collections::VecDeque;

use super::fsst::Symbols;
use super::group::{Pages, Seal, Zone};
use super::{Chunk, padding};
use crate::error::Result;
use crate::storage::Mask;
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
    /// Its bytes, opened; none kept of a chunk read in the pool.
    bytes: Vec<u8>,
    /// Whether its bytes are read in the pool by each run, as its page
    /// passed through no encoding (RULES.md D1094).
    in_place: bool,
    table: Option<(Box<Symbols>, usize)>,
    made: Made,
}

/// Of a chunk, what its runs share: of a dictionary of a type not of
/// character strings, its entries as integers.
#[derive(Default)]
pub struct Made {
    pub(super) words: Option<Vec<i64>>,
}

/// \[DESIGN\] The chunks opened of the group being read.
#[derive(Default)]
pub struct Prepared {
    /// Each column's chunks held, by its number.
    columns: Vec<VecDeque<Held>>,
    /// The bytes of chunks let go, to hold the next ones'.
    spare: Vec<Vec<u8>>,
    /// The columns given measured, and the lengths held of a run
    /// (`prepared/lengths.rs`, RULES.md D1267).
    lengths: lengths::Lengths,
    /// What is read and made of the group's sketches (`sketch.rs`).
    pub(super) sketches: super::sketch::Held,
    /// The steps of the scan's LIKEs by the tables of symbols its chunks
    /// are coded by (`fsst/stepped.rs`, RULES.md D1251), kept from group
    /// to group, as each is found again by its table.
    pub(super) stepped: super::fsst::Stepped,
    /// \[DESIGN\] (RULES.md D1318) By column, of the run of texts that
    /// gave the most octets a stored octet so far, its octets and the
    /// octets its chunks stored for its rows (`room`).
    rooms: Vec<(usize, usize)>,
    /// How many chunks were opened, as a test counts them.
    #[cfg(test)]
    pub(super) opened: usize,
}

/// `n` as a number that holds the product of two counts.
fn wide(n: usize) -> u128 {
    u128::try_from(n).unwrap_or(u128::MAX)
}

impl Prepared {
    /// Lets every chunk go, as the next group is taken up.
    pub fn clear(&mut self) {
        let Prepared { columns, spare, sketches, lengths, .. } = self;
        sketches.clear();
        lengths.clear();
        for kept in columns {
            spare.extend(kept.drain(..).map(|h| h.bytes));
        }
        spare.truncate(KEPT);
    }

    /// \[DESIGN\] (RULES.md D1318) The room to make for the texts of a
    /// run of column `column` whose chunks store `stored` octets for its
    /// rows: `stored` by the most octets a stored octet has given in a
    /// run of the column so far (`gave`), and `stored` until a run has
    /// told. A dictionary's entries and varying fields are no more than
    /// their chunk stores, and texts coded by symbols are about twice
    /// it, so a column that mixes them is given room by the run that
    /// needed most, and its vector grows no more once such a run is read.
    pub(super) fn room(&self, column: usize, stored: usize) -> usize {
        let Some(&(octets, of)) = self.rooms.get(column) else { return stored };
        let scaled = wide(stored).saturating_mul(wide(octets)).checked_div(wide(of)).unwrap_or(0);
        usize::try_from(scaled).unwrap_or(stored).max(stored)
    }

    /// \[DESIGN\] (RULES.md D1318) Notes that a run of column `column`
    /// whose chunks stored `stored` octets for its rows gave `octets`, if
    /// no run of the column before gave more a stored octet. A stored
    /// octet is taken to give no more than a symbol's most
    /// (`fsst::LONGEST`), as no code does, so a run that read a few long
    /// rows of a chunk does not set the room of all that follow.
    pub(super) fn gave(&mut self, column: usize, (octets, stored): (usize, usize)) {
        if self.rooms.len() <= column {
            self.rooms.resize(column.saturating_add(1), (0, 0));
        }
        let Some(best) = self.rooms.get_mut(column) else { return };
        let told = octets.min(stored.saturating_mul(super::fsst::LONGEST));
        if wide(told).saturating_mul(wide(best.1)) > wide(best.0).saturating_mul(wide(stored)) || best.1 == 0 {
            *best = (told, stored);
        }
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
        if self.columns.len() <= column {
            self.columns.resize_with(column.saturating_add(1), VecDeque::new);
        }
        let Prepared {
            columns,
            spare,
            #[cfg(test)]
            opened,
            ..
        } = &mut *self;
        let kept = columns.get_mut(column).ok_or_else(|| super::corrupt("a column held is gone"))?;
        let at = if let Some(at) = kept.iter().position(|h| h.first == first) {
            at
        } else {
            while kept.front().is_some_and(|h| h.end <= first) || kept.len() >= KEPT {
                spare.extend(kept.pop_front().map(|h| h.bytes));
            }
            kept.push_back(open(pages, (zone, t), spare.pop().unwrap_or_default())?);
            #[cfg(test)]
            {
                *opened = opened.saturating_add(1);
            }
            kept.len().saturating_sub(1)
        };
        let held = kept.get(at).ok_or_else(|| super::corrupt("a chunk held is gone"))?;
        let table = held.table.as_ref().map(|(s, len)| (&**s, *len));
        if held.in_place {
            // Its page is held while the run is decoded of it.
            return pages.with_run(&zone.run, |bytes| read(&Chunk::parse_with(bytes, table)?, &held.made));
        }
        read(&Chunk::parse_with(&held.bytes, table)?, &held.made)
    }
}

/// The chunk `zone`, of a column of type `t`, read of `pages`, and what
/// its runs share made of it: its bytes kept in `bytes` only if its page
/// had to be opened by its encodings.
fn open<S: Seal>(pages: &Pages<'_, S>, (zone, t): (&Zone, &DataType), mut bytes: Vec<u8>) -> Result<Held> {
    bytes.clear();
    let (table, made, in_place) = pages.with_run_told(&zone.run, |read, in_place| {
        if !in_place {
            bytes.extend_from_slice(read);
        }
        let chunk = Chunk::parse(read)?;
        // Entries as integers of a type not of character strings. Those
        // of texts are read of the chunk by each run (RULES.md D1252).
        let words = if padding(t).is_some() { None } else { chunk.body.entry_words() };
        Ok((chunk.table(), Made { words }, in_place))
    })?;
    Ok(Held { first: zone.run.first, end: zone.run.first.saturating_add(zone.run.rows), bytes, in_place, table, made })
}

impl Chunk<'_> {
    /// As `ints`, of what is made of it once, `made`.
    pub(super) fn ints_made(
        &self,
        (from, to): (usize, usize),
        outs: (&mut Vec<i64>, &mut Mask),
        (made, kernels): (&Made, &dyn crate::kernels::Kernels),
    ) -> Option<()> {
        self.ints_with((from, to), outs, (made.words.as_deref(), kernels))
    }

    /// \[DESIGN\] (RULES.md D360) Its value at position `n`, of 8 bytes, as
    /// an integer, read alone (`encoded::Body::int_at`), of what is made of
    /// it once, `made`: none if its encoding does not hold it.
    pub(super) fn int_made(&self, n: usize, made: &Made) -> Option<i64> {
        if n >= self.count {
            return None;
        }
        self.body.int_at(n, made.words.as_deref())
    }
}

mod lengths;

#[cfg(test)]
mod tests;
