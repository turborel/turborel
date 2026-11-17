//! \[DESIGN\] A document's postings (RULES.md D86): for each term, its
//! frequency and where its words are, each place (word ordinal, sentence,
//! paragraph and character offset) [ISO/IEC 13249-2:2002 (CD, SC32
//! N0640) §5.1.5] kept as its difference from the place before, a
//! variable-length integer each. A term's places in one document are
//! kept in its block (`block.rs`) while they are few; past that, in chunks
//! of their own (`Q` entries), so no entry is longer than the tree takes,
//! each chunk's differences beginning from nothing, so it reads alone.
//!
//! Stop words are not posted, but count among the ordinals, so the places
//! of the words around them stay right [§4.4.2].
//!
//! A document is never held whole, as it may be a large object of
//! gigabytes (RULES.md D38): its tokens are read once, each place a record
//! of its term and place sorted in a spill area (`spill::sorted_records`),
//! which holds what fits its bytes in memory and spills the rest. The
//! sorted records give each term's places in order, a run of the term,
//! read as it is taken (`Runs`): a run holds its places while they would
//! stay in a block, then gives them a chunk at a time.

use std::collections::VecDeque;
use std::iter::Peekable;

use super::block::{block_room, inline_most};
use super::document::{Hasher, Reading, chars_of, term_of};
use super::keys::{DocHash, OVERFLOW_OVERHEAD, VARINT_MOST, corrupt, put_varint, take_varint};
use crate::error::{Code, Result, SqlError};
use crate::spill::{Area, Records, WorkAreas, sorted_records};
use crate::value::Value;

/// Places in a position.
const PLACES: usize = 4;
/// Bytes of a place in a sorted record.
const PLACE_BYTES: usize = 4;
/// Bytes of a sorted record after its term: the 0 ending it, then four
/// places.
const RECORD_SUFFIX: usize = 1 + PLACES * PLACE_BYTES;
/// The most bytes one position takes.
pub(super) const MOST_POSITION: usize = PLACES * VARINT_MOST;

/// Where a word is: its ordinal among the document's words, stop words
/// counted, its sentence, its paragraph, and its first character's offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Position {
    /// Its ordinal among the document's words, from 1.
    pub ordinal: u32,
    /// Its sentence's ordinal, from 1.
    pub sentence: u32,
    /// Its paragraph's ordinal, from 1.
    pub paragraph: u32,
    /// Its first character's offset, from 0.
    pub offset: u32,
}

impl Position {
    /// Its places, in order.
    fn places(self) -> [u32; PLACES] {
        [self.ordinal, self.sentence, self.paragraph, self.offset]
    }
}

/// A document read: its hash, its length in words that are not stop
/// words, and its places, sorted by term, then place.
pub(super) struct Read {
    /// The hash of its tokens (`document.rs`).
    pub hash: DocHash,
    /// Its words that are not stop words.
    pub length: u32,
    /// Its places' records, sorted.
    pub places: Records,
}

/// The sorted record of `position` of `term`: the term, its end, then its
/// places, big-endian, so a term's sort in document order.
fn record(term: &[u8], position: Position) -> Vec<u8> {
    let mut out = Vec::with_capacity(term.len().saturating_add(RECORD_SUFFIX));
    out.extend_from_slice(term);
    out.push(0);
    for place in position.places() {
        out.extend_from_slice(&place.to_be_bytes());
    }
    out
}

/// The term and position of a sorted record.
///
/// # Errors
///
/// SQLCODE -916 if it is malformed.
fn parse(record: &[u8]) -> Result<(&[u8], Position)> {
    let at = record.len().checked_sub(RECORD_SUFFIX).ok_or_else(corrupt)?;
    let (term, rest) = record.split_at_checked(at).ok_or_else(corrupt)?;
    let mut places = [0_u32; PLACES];
    let (each, _) = rest.get(1..).unwrap_or_default().as_chunks::<PLACE_BYTES>();
    for (place, bytes) in places.iter_mut().zip(each) {
        *place = u32::from_be_bytes(*bytes);
    }
    let [ordinal, sentence, paragraph, offset] = places;
    Ok((term, Position { ordinal, sentence, paragraph, offset }))
}

/// The document `value` read as `reading` says, its terms cut to `most`
/// bytes, its places sorted in an area of `work`; none if it is null.
///
/// # Errors
///
/// As reading it; the tokenizer's; the spill area's.
pub(super) fn read(value: &Value, reading: &Reading, most: usize, (work, area): (&WorkAreas, Area)) -> Result<Option<Read>> {
    let Some(chars) = chars_of(value)? else { return Ok(None) };
    let mut hasher = Hasher::new();
    let mut length = 0_u32;
    let mut tokens = reading.tokens(chars);
    let mut records = std::iter::from_fn(|| {
        loop {
            let token = match tokens.next()? {
                Ok(token) => token,
                Err(e) => return Some(Err(e)),
            };
            hasher.add(&token);
            if !token.stop {
                length = length.saturating_add(1);
                let position =
                    Position { ordinal: token.ordinal, sentence: token.sentence, paragraph: token.paragraph, offset: token.offset };
                return Some(Ok(record(term_of(&token.word, most), position)));
            }
        }
    });
    let places = sorted_records(work, area, &mut records)?;
    Ok(Some(Read { hash: hasher.finish(), length, places }))
}

/// What reading a document's runs gives.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Event {
    /// A chunk of a term's places kept past its block: the term, the
    /// chunk's number and its bytes; before the run's end.
    Overflow(Vec<u8>, u16, Vec<u8>),
    /// The end of a term's run: the term, its frequency, and its places'
    /// bytes, or none if they were given in chunks.
    End(Vec<u8>, u32, Option<Vec<u8>>),
}

/// The run of one term being read.
struct Run {
    term: Vec<u8>,
    /// The places not yet given, from `last` on.
    current: Vec<u8>,
    /// The number of the chunk being filled, once the run overflows.
    chunk: u16,
    /// Whether its places are past what a block keeps.
    overflow: bool,
    /// The most bytes of places a block keeps, and of a chunk.
    inline: usize,
    room: usize,
    last: Position,
    frequency: u32,
}

/// Each term's run of sorted places, in term order.
pub(super) struct Runs {
    places: Peekable<Records>,
    /// The most bytes of an entry of the tree.
    max_entry: usize,
    run: Option<Run>,
    ready: VecDeque<Event>,
}

impl Runs {
    /// The runs of `places`, for a tree whose entries take at most
    /// `max_entry` bytes.
    pub fn new(places: Records, max_entry: usize) -> Runs {
        Runs { places: places.peekable(), max_entry, run: None, ready: VecDeque::new() }
    }

    /// Adds `position` of `term` to its run, ending the run before if it
    /// is another term's.
    fn add(&mut self, term: &[u8], position: Position) -> Result<()> {
        if self.run.as_ref().is_some_and(|run| run.term != term) {
            self.end();
        }
        let max_entry = self.max_entry;
        let run = self.run.get_or_insert_with(|| Run {
            term: term.to_vec(),
            current: Vec::new(),
            chunk: 0,
            overflow: false,
            inline: inline_most(block_room(max_entry, term.len())),
            room: max_entry.saturating_sub(term.len().saturating_add(OVERFLOW_OVERHEAD)),
            last: Position::default(),
            frequency: 0,
        });
        let wanted = run.current.len().saturating_add(MOST_POSITION);
        run.overflow |= wanted > run.inline;
        if run.overflow && wanted > run.room && !run.current.is_empty() {
            self.ready.push_back(Event::Overflow(run.term.clone(), run.chunk, std::mem::take(&mut run.current)));
            run.chunk = run.chunk.checked_add(1).ok_or_else(|| {
                SqlError::new(Code::Unsupported, "[DESIGN] a term occurs in one document more often than a full-text index's chunks hold")
            })?;
            run.last = Position::default();
        }
        for (place, before) in position.places().into_iter().zip(run.last.places()) {
            put_varint(&mut run.current, u64::from(place.saturating_sub(before)));
        }
        run.last = position;
        run.frequency = run.frequency.saturating_add(1);
        Ok(())
    }

    /// Ends the run under way: its last chunk, if it overflowed, then its
    /// end.
    fn end(&mut self) {
        let Some(run) = self.run.take() else { return };
        let inline = if run.overflow {
            self.ready.push_back(Event::Overflow(run.term.clone(), run.chunk, run.current));
            None
        } else {
            Some(run.current)
        };
        self.ready.push_back(Event::End(run.term, run.frequency, inline));
    }
}

impl Iterator for Runs {
    type Item = Result<Event>;

    fn next(&mut self) -> Option<Result<Event>> {
        loop {
            if let Some(event) = self.ready.pop_front() {
                return Some(Ok(event));
            }
            let Some(record) = self.places.next() else {
                self.run.as_ref()?;
                self.end();
                continue;
            };
            let added = record.and_then(|record| parse(&record).and_then(|(term, position)| self.add(term, position)));
            if let Err(e) = added {
                self.run = None;
                return Some(Err(e));
            }
        }
    }
}

/// The places in `value`: a document's in its block, or one chunk's.
///
/// # Errors
///
/// SQLCODE -916 if it is malformed.
pub(super) fn positions_in(mut value: &[u8]) -> Result<Vec<Position>> {
    let mut out = Vec::new();
    let mut last = Position::default();
    while !value.is_empty() {
        let mut places = [0_u32; PLACES];
        for (place, before) in places.iter_mut().zip(last.places()) {
            let (delta, rest) = take_varint(value)?;
            *place = u32::try_from(delta).ok().and_then(|d| before.checked_add(d)).ok_or_else(corrupt)?;
            value = rest;
        }
        let [ordinal, sentence, paragraph, offset] = places;
        last = Position { ordinal, sentence, paragraph, offset };
        out.push(last);
    }
    Ok(out)
}
