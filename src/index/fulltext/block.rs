//! \[DESIGN\] Blocks of postings (RULES.md D86): a term's postings, each a
//! document number, the term's frequency there and its places, kept in
//! blocks of at most `MOST_BLOCK_DOCUMENTS` documents, each block an entry
//! keyed by its first document number (`keys.rs`), so the block keys are
//! a skip list over the term's documents (`cursor.rs`). Block i holds the
//! documents numbered from its key up to the next block's.
//!
//! A block's value is
//!
//! * a header: its documents n, the greatest frequency among them, and the
//!   bytes of the section after it, variable-length integers each;
//! * that section: the n document numbers, each the difference from the
//!   one before, the first from the block's key; then the n frequencies,
//!   each shifted left one bit, the low bit set if its places are kept
//!   past the block (`Q` entries);
//! * the places of each document, each its length in bytes then its places
//!   (`postings.rs`), empty for one kept past the block; so a reader of
//!   document numbers stops at the header's length, and a reader of one
//!   document's places skips the others' by their lengths.
//!
//! A document's places stay in its block while they take at most a
//! quarter of the room a block has (`inline_most`), so a block always
//! holds at least four. A block is written whole by the writer holding its
//! term's stripe lock (`FullTextIndex::term_lock`): a document is put in
//! its place, and a block past `MOST_BLOCK_DOCUMENTS` or its room is split
//! in two, the new one keyed by its first document number, written before
//! the old is cut, so a reader may see a document twice, never not at all;
//! readers keep document numbers rising, so they give it once.

use std::ops::Bound;

use super::FullTextIndex;
use super::keys::{BLOCK_OVERHEAD, block_docno, block_key, block_range, corrupt, overflow_key, put_varint, take_varint};
use crate::engine::tree::{Direction, Update};
use crate::error::Result;

/// \[DESIGN\] The most documents in a block.
pub(super) const MOST_BLOCK_DOCUMENTS: usize = 128;
/// \[DESIGN\] The bytes a document takes in a block besides its places, at
/// most: its number's difference, frequency and places' length.
const MOST_POSTING_OVERHEAD: usize = 24;
/// The share of a block's room one document's places may take, as its
/// inverse: a quarter.
const INLINE_SHARE: usize = 4;
/// The low bit of a frequency's value: its places are kept past the block.
const OVERFLOW_BIT: u64 = 1;

/// Where a document's places are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Places {
    /// In the block: their bytes (`postings::positions_in`).
    Inline(Vec<u8>),
    /// Past it, in the term's `Q` entries of the document.
    Overflow,
}

/// One document of a term's block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Posting {
    /// The document's number.
    pub docno: u64,
    /// The term's frequency in it.
    pub frequency: u32,
    /// Its places.
    pub places: Places,
}

/// The bytes a block of `term` may take: an entry's, less its key's.
pub(super) fn block_room(max_entry: usize, term: usize) -> usize {
    max_entry.saturating_sub(term.saturating_add(BLOCK_OVERHEAD))
}

/// The most bytes of one document's places kept in a block of `room`.
pub(super) fn inline_most(room: usize) -> usize {
    room.checked_div(INLINE_SHARE).unwrap_or_default().saturating_sub(MOST_POSTING_OVERHEAD)
}

/// The value of a block keyed `first` holding `postings`, in order.
pub(super) fn encode(first: u64, postings: &[Posting]) -> Vec<u8> {
    let mut ids = Vec::new();
    let mut before = first;
    for posting in postings {
        put_varint(&mut ids, posting.docno.saturating_sub(before));
        before = posting.docno;
    }
    let mut most = 0_u32;
    for posting in postings {
        let overflow = u64::from(matches!(posting.places, Places::Overflow));
        put_varint(&mut ids, (u64::from(posting.frequency) << 1_u32) | overflow);
        most = most.max(posting.frequency);
    }
    let mut out = Vec::with_capacity(ids.len().saturating_add(MOST_POSTING_OVERHEAD));
    put_varint(&mut out, u64::try_from(postings.len()).unwrap_or(u64::MAX));
    put_varint(&mut out, u64::from(most));
    put_varint(&mut out, u64::try_from(ids.len()).unwrap_or(u64::MAX));
    out.extend_from_slice(&ids);
    for posting in postings {
        let places: &[u8] = match &posting.places {
            Places::Inline(bytes) => bytes,
            Places::Overflow => &[],
        };
        put_varint(&mut out, u64::try_from(places.len()).unwrap_or(u64::MAX));
        out.extend_from_slice(places);
    }
    out
}

/// A count read from a block, as a `usize`.
fn count(value: u64) -> Result<usize> {
    usize::try_from(value).map_err(|_| corrupt())
}

/// A block's header: its documents, its greatest frequency, its numbers'
/// and frequencies' section, and the places after it.
fn header(value: &[u8]) -> Result<(usize, u64, &[u8], &[u8])> {
    let (n, rest) = take_varint(value)?;
    let (most, rest) = take_varint(rest)?;
    let (length, rest) = take_varint(rest)?;
    let (ids, places) = rest.split_at_checked(count(length)?).ok_or_else(corrupt)?;
    Ok((count(n)?, most, ids, places))
}

/// The document numbers of the block keyed `first` of value `value`.
///
/// # Errors
///
/// SQLCODE -916 if it is malformed.
pub(super) fn docnos(first: u64, value: &[u8]) -> Result<Vec<u64>> {
    let (n, _, mut ids, _) = header(value)?;
    let mut out = Vec::with_capacity(n.min(MOST_BLOCK_DOCUMENTS));
    let mut at = first;
    for _ in 0..n {
        let (delta, rest) = take_varint(ids)?;
        at = at.checked_add(delta).ok_or_else(corrupt)?;
        out.push(at);
        ids = rest;
    }
    Ok(out)
}

/// The document numbers of the block keyed `first` of value `value`, and
/// the term's frequency in each.
///
/// # Errors
///
/// SQLCODE -916 if it is malformed.
pub(super) fn frequencies(first: u64, value: &[u8]) -> Result<(Vec<u64>, Vec<u32>)> {
    let numbers = docnos(first, value)?;
    let (_, _, mut ids, _) = header(value)?;
    for _ in &numbers {
        ids = take_varint(ids)?.1;
    }
    let mut out = Vec::with_capacity(numbers.len());
    for _ in &numbers {
        let (flagged, rest) = take_varint(ids)?;
        ids = rest;
        out.push(u32::try_from(flagged >> 1_u32).map_err(|_| corrupt())?);
    }
    Ok((numbers, out))
}

/// The postings of the block keyed `first` of value `value`.
///
/// # Errors
///
/// SQLCODE -916 if it is malformed.
pub(super) fn decode(first: u64, value: &[u8]) -> Result<Vec<Posting>> {
    let numbers = docnos(first, value)?;
    let (_, _, ids, mut places) = header(value)?;
    let mut ids = ids;
    for _ in &numbers {
        ids = take_varint(ids)?.1;
    }
    let mut out = Vec::with_capacity(numbers.len());
    for docno in numbers {
        let (flagged, after_ids) = take_varint(ids)?;
        ids = after_ids;
        let (length, sized) = take_varint(places)?;
        let (bytes, after_places) = sized.split_at_checked(count(length)?).ok_or_else(corrupt)?;
        places = after_places;
        let frequency = u32::try_from(flagged >> 1_u32).map_err(|_| corrupt())?;
        let kept = if flagged & OVERFLOW_BIT == 0 { Places::Inline(bytes.to_vec()) } else { Places::Overflow };
        out.push(Posting { docno, frequency, places: kept });
    }
    Ok(out)
}

/// Whether a block keyed `first` of `postings` fits a block of `room`.
pub(super) fn fits(first: u64, postings: &[Posting], room: usize) -> bool {
    postings.len() <= MOST_BLOCK_DOCUMENTS && encode(first, postings).len() <= room
}

/// Where to split `postings`, too many for one block: past the last if it
/// was just put there, so blocks filled in order stay full; else where
/// the bytes before reach half.
fn split_at(postings: &[Posting], appended: bool) -> usize {
    let last = postings.len().saturating_sub(1);
    if appended {
        return last;
    }
    let size = |p: &Posting| match &p.places {
        Places::Inline(bytes) => bytes.len().saturating_add(MOST_POSTING_OVERHEAD),
        Places::Overflow => MOST_POSTING_OVERHEAD,
    };
    let half = postings.iter().map(size).sum::<usize>().checked_div(2).unwrap_or_default();
    let mut bytes = 0_usize;
    for (at, posting) in postings.iter().enumerate() {
        bytes = bytes.saturating_add(size(posting));
        if bytes >= half {
            return at.saturating_add(1).clamp(1, last);
        }
    }
    last
}

/// The bytes `value` takes as a variable-length integer.
fn varint_len(value: u64) -> usize {
    let bits = u64::BITS.saturating_sub(value.leading_zeros()).max(1);
    usize::try_from(bits.div_ceil(7)).unwrap_or(usize::MAX)
}

/// A block filled in order, as a load fills it, its size kept as each
/// posting is added, so it is never encoded to learn whether one fits.
pub(super) struct Builder {
    /// Its key's document number, and its last's.
    first: u64,
    last: u64,
    most: u32,
    /// The bytes of its numbers' and frequencies' section, and of its
    /// places.
    ids: usize,
    places: usize,
    pub postings: Vec<Posting>,
}

impl Builder {
    /// An empty block keyed `first`.
    pub fn new(first: u64) -> Builder {
        Builder { first, last: first, most: 0, ids: 0, places: 0, postings: Vec::new() }
    }

    /// Its key's document number.
    pub fn first(&self) -> u64 {
        self.first
    }

    /// Adds `posting`, numbered past its last, if the block then still
    /// fits `room`; gives it back if not.
    pub fn add(&mut self, posting: Posting, room: usize) -> Option<Posting> {
        let flag = u64::from(matches!(posting.places, Places::Overflow));
        let place_bytes = match &posting.places {
            Places::Inline(bytes) => bytes.len(),
            Places::Overflow => 0,
        };
        let ids = self
            .ids
            .saturating_add(varint_len(posting.docno.saturating_sub(self.last)))
            .saturating_add(varint_len((u64::from(posting.frequency) << 1_u32) | flag));
        let places = self.places.saturating_add(varint_len(u64::try_from(place_bytes).unwrap_or(u64::MAX))).saturating_add(place_bytes);
        let most = self.most.max(posting.frequency);
        let n = self.postings.len().saturating_add(1);
        let head = varint_len(u64::try_from(n).unwrap_or(u64::MAX))
            .saturating_add(varint_len(u64::from(most)))
            .saturating_add(varint_len(u64::try_from(ids).unwrap_or(u64::MAX)));
        if n > MOST_BLOCK_DOCUMENTS || head.saturating_add(ids).saturating_add(places) > room {
            return Some(posting);
        }
        (self.ids, self.places, self.most, self.last) = (ids, places, most, posting.docno);
        self.postings.push(posting);
        None
    }
}

impl FullTextIndex {
    /// The bytes a block of `term` may take.
    pub(super) fn room(&self, term: &[u8]) -> usize {
        block_room(self.tree.max_entry(), term.len())
    }

    /// The block of `term` covering document number `docno`: the last whose
    /// key is at most `docno`, else the first; its first number and value.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if a key is malformed.
    pub(super) fn covering(&self, term: &[u8], docno: u64) -> Result<Option<(u64, Vec<u8>)>> {
        let (lower, upper) = block_range(term);
        let bound = block_key(term, docno);
        let before = self.tree.scan(Bound::Included(&lower), Bound::Included(&bound), Direction::Backward).next().transpose()?;
        let found = match before {
            Some(found) => Some(found),
            None => self.tree.scan(Bound::Included(&lower), Bound::Excluded(&upper), Direction::Forward).next().transpose()?,
        };
        found.map(|(key, value)| Ok((block_docno(&key, term)?, value))).transpose()
    }

    /// The posting of `term` in document `docno`, if any.
    ///
    /// # Errors
    ///
    /// As `covering`.
    pub(super) fn posting(&self, term: &[u8], docno: u64) -> Result<Option<Posting>> {
        let Some((first, value)) = self.covering(term, docno)? else { return Ok(None) };
        Ok(decode(first, &value)?.into_iter().find(|p| p.docno == docno))
    }

    /// Puts `posting` in `term`'s blocks, the caller holding its stripe
    /// lock: in its place in the block covering it, split if past its room
    /// or count; a block of its own for a term's first.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if a block is malformed.
    pub(super) fn put_posting(&self, term: &[u8], posting: Posting) -> Result<()> {
        let room = self.room(term);
        let Some((first, value)) = self.covering(term, posting.docno)? else {
            return self.write_block(term, posting.docno, &[posting]);
        };
        let mut postings = decode(first, &value)?;
        let at = postings.partition_point(|p| p.docno < posting.docno);
        let start = first.min(posting.docno);
        if postings.get(at).is_some_and(|p| p.docno == posting.docno) {
            postings.remove(at);
        }
        postings.insert(at, posting);
        let mut kept = start == first;
        if !fits(start, &postings, room) {
            let right = postings.split_off(split_at(&postings, at.saturating_add(1) == postings.len()));
            let right_first = right.first().map_or(start, |p| p.docno);
            self.write_block(term, right_first, &right)?;
            kept |= right_first == first;
        }
        self.write_block(term, start, &postings)?;
        if !kept {
            self.tree.delete(&block_key(term, first))?;
        }
        Ok(())
    }

    /// Writes the block of `term` keyed `first` holding `postings`.
    fn write_block(&self, term: &[u8], first: u64, postings: &[Posting]) -> Result<()> {
        let value = encode(first, postings);
        self.tree.update(&block_key(term, first), |_| Ok(Update::Put(value)))
    }

    /// Takes the posting of `term` in document `docno` out of its block,
    /// the caller holding its stripe lock; the block deleted if emptied.
    /// The posting taken, if there was one.
    ///
    /// # Errors
    ///
    /// As `put_posting`.
    pub(super) fn take_posting(&self, term: &[u8], docno: u64) -> Result<Option<Posting>> {
        let Some((first, value)) = self.covering(term, docno)? else { return Ok(None) };
        let mut postings = decode(first, &value)?;
        let Some(at) = postings.iter().position(|p| p.docno == docno) else { return Ok(None) };
        let taken = postings.remove(at);
        if postings.is_empty() {
            self.tree.delete(&block_key(term, first))?;
        } else {
            self.write_block(term, first, &postings)?;
        }
        Ok(Some(taken))
    }

    /// Deletes every chunk of `term`'s places past its block in document
    /// `docno`: numbered from 0 without a gap, the first missing ends them.
    ///
    /// # Errors
    ///
    /// The tree's.
    pub(super) fn delete_overflow(&self, term: &[u8], docno: u64) -> Result<()> {
        for chunk in 0..=u16::MAX {
            if self.tree.delete(&overflow_key(term, docno, chunk))?.is_none() {
                break;
            }
        }
        Ok(())
    }
}
