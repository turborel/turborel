//! \[DESIGN\] A character large object as a text the matchers read by
//! position (RULES.md D61, D79): its characters are read a chunk at a time,
//! the two chunks read last kept decoded, so a search holds at most two
//! chunks' characters whatever the value's length. The chunk of a
//! character is found from the source's directory of characters per
//! chunk, and of a part held inline from marks set a chunk's octets apart,
//! never by reading what is before it. Two chunks are kept so a search
//! going back across a chunk's end, a back-reference to a capture in the
//! chunk before or an assertion reading the character before, does not
//! read either again and again.

use std::cell::RefCell;

use super::text::Text;
use crate::error::Result;
use crate::value::lob::{CHUNK_OCTETS, Lob, LobKind, Part, Slice, broken};

/// Where a part of the value begins, and how long it is, in characters
/// and octets; of a part held inline, where in it each piece of about a
/// chunk's octets begins, its character and octet.
#[derive(Debug)]
struct Placed {
    char: u64,
    octet: u64,
    marks: Vec<(u64, usize)>,
}

/// Characters of the value decoded, from character `lo`.
#[derive(Debug)]
struct Held {
    lo: usize,
    chars: Vec<char>,
}

impl Held {
    /// The character at `at` of the value, if it is held.
    fn get(&self, at: usize) -> Option<char> {
        self.chars.get(at.checked_sub(self.lo)?).copied()
    }
}

/// A character large object read by position, a chunk at a time.
#[derive(Debug)]
pub struct LobText<'a> {
    lob: &'a Lob,
    len: usize,
    placed: Vec<Placed>,
    /// The two chunks read last.
    held: RefCell<Kept>,
}

/// The two chunks read last, so one is read again only after two others.
#[derive(Debug, Default)]
struct Kept {
    latest: Option<Held>,
    before: Option<Held>,
}

impl<'a> LobText<'a> {
    /// The text of `lob`, a character large object, its chunks unread.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 of a binary large object, or one a locator or a
    /// reference names, whose characters are not here to read.
    pub fn new(lob: &'a Lob) -> Result<LobText<'a>> {
        if lob.kind() == LobKind::Binary {
            return Err(broken("a binary large object is read as characters"));
        }
        let len = usize::try_from(lob.chars()).map_err(|_| broken("a character large object is longer than memory can count"))?;
        let (mut char, mut octet) = (0_u64, 0_u64);
        let mut placed = Vec::with_capacity(lob.parts().len());
        for p in lob.parts() {
            let (marks, (octets, chars)) = match p {
                Part::Inline(bytes, chars) => (marks(bytes), (u64::try_from(bytes.len()).unwrap_or(u64::MAX), *chars)),
                Part::Slice(s) => (Vec::new(), (s.octets, s.chars)),
                Part::Locator(_) | Part::Reference(_) => return Err(broken("a locator's or reference's value is read where it is kept")),
            };
            placed.push(Placed { char, octet, marks });
            char = char.saturating_add(chars);
            octet = octet.saturating_add(octets);
        }
        Ok(LobText { lob, len, placed, held: RefCell::new(Kept::default()) })
    }

    /// The part character or octet `at` is in, by `key`, and where it
    /// begins.
    fn part(&self, at: u64, key: fn(&Placed) -> u64) -> Result<(&Part, &Placed)> {
        let k = self.placed.partition_point(|p| key(p) <= at).checked_sub(1).ok_or_else(|| broken("a position before the first part"))?;
        match (self.lob.parts().get(k), self.placed.get(k)) {
            (Some(part), Some(placed)) => Ok((part, placed)),
            (None, _) | (_, None) => Err(broken("a position past the last part")),
        }
    }

    /// The characters of the chunk, or piece of a part held inline, that
    /// character `at` is in.
    fn load(&self, at: usize) -> Result<Held> {
        let at = u64::try_from(at).map_err(|_| broken("a position past what a large object holds"))?;
        let (part, placed) = self.part(at, |p| p.char)?;
        let within = at.saturating_sub(placed.char);
        let (first, chars) = match part {
            Part::Inline(bytes, _) => inline_piece(bytes, &placed.marks, within)?,
            Part::Slice(s) => slice_chunk(s, within)?,
            Part::Locator(_) | Part::Reference(_) => return Err(broken("a locator's or reference's value is read where it is kept")),
        };
        let lo = usize::try_from(placed.char.saturating_add(first)).map_err(|_| broken("a position past what memory can count"))?;
        Ok(Held { lo, chars })
    }

    /// The character, from 0, that begins at octet `at` of the value, or
    /// the first after it if `at` is within a character; the value's
    /// characters at or past its end.
    ///
    /// # Errors
    ///
    /// As reading the chunk it is in fails.
    pub fn char_of_octet(&self, at: u64) -> Result<usize> {
        if at >= self.lob.octets() {
            return Ok(self.len);
        }
        let (part, placed) = self.part(at, |p| p.octet)?;
        let within = at.saturating_sub(placed.octet);
        let before = match part {
            Part::Inline(bytes, _) => leads(bytes.get(..usize::try_from(within).unwrap_or(usize::MAX)).unwrap_or(bytes)),
            Part::Slice(s) => {
                let octet = s.octet.saturating_add(within);
                let (k, start, char) =
                    s.source.directory()?.by_octet(octet).ok_or_else(|| broken("a slice goes past its source's chunks"))?;
                let bytes = s.source.chunk(k)?;
                let cut = usize::try_from(octet.saturating_sub(start)).unwrap_or(usize::MAX);
                char.saturating_add(leads(bytes.get(..cut).unwrap_or(&bytes))).saturating_sub(s.char)
            }
            Part::Locator(_) | Part::Reference(_) => return Err(broken("a locator's or reference's value is read where it is kept")),
        };
        usize::try_from(placed.char.saturating_add(before)).map_err(|_| broken("a position past what memory can count"))
    }

    /// The octet character `at`, from 0, begins at; the value's octets at
    /// its end.
    ///
    /// # Errors
    ///
    /// As reading the chunk it is in fails.
    pub fn octet_of_char(&self, at: usize) -> Result<u64> {
        self.lob.reader().octet_of_char(u64::try_from(at).unwrap_or(u64::MAX))
    }

    /// The value read.
    pub fn lob(&self) -> &'a Lob {
        self.lob
    }
}

impl Text for LobText<'_> {
    fn len(&self) -> usize {
        self.len
    }

    fn at(&self, at: usize) -> Result<Option<char>> {
        if at >= self.len {
            return Ok(None);
        }
        let mut held = self.held.try_borrow_mut().map_err(|_| broken("a large object's text was read within its own read"))?;
        let kept = &mut *held;
        if let Some(c) = kept.latest.as_ref().and_then(|h| h.get(at)) {
            return Ok(Some(c));
        }
        if let Some(c) = kept.before.as_ref().and_then(|h| h.get(at)) {
            std::mem::swap(&mut kept.latest, &mut kept.before);
            return Ok(Some(c));
        }
        let loaded = self.load(at)?;
        let c = loaded.get(at);
        kept.before = kept.latest.replace(loaded);
        if c.is_none() {
            return Err(broken("a chunk holds fewer characters than its directory says"));
        }
        Ok(c)
    }
}

/// Where in UTF-8 `bytes` each piece of about a chunk's octets begins, at
/// a character's boundary: its character and octet.
fn marks(bytes: &[u8]) -> Vec<(u64, usize)> {
    let mut marks = vec![(0_u64, 0_usize)];
    let (mut char, mut last) = (0_u64, 0_usize);
    for (i, b) in bytes.iter().enumerate() {
        if b & 0xC0 == 0x80 {
            continue;
        }
        if i.saturating_sub(last) >= CHUNK_OCTETS {
            marks.push((char, i));
            last = i;
        }
        char = char.saturating_add(1);
    }
    marks
}

/// How many characters begin in UTF-8 `bytes`.
fn leads(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.iter().filter(|&&b| b & 0xC0 != 0x80).count()).unwrap_or(u64::MAX)
}

/// `bytes` decoded.
fn decoded(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|_| broken("a character large object's octets are not UTF-8"))
}

/// Of a part held inline in `bytes`, marked by `marks`, the piece its
/// character `at` is in: the character it begins at, and its characters.
fn inline_piece(bytes: &[u8], marks: &[(u64, usize)], at: u64) -> Result<(u64, Vec<char>)> {
    let m = marks.partition_point(|(c, _)| *c <= at).checked_sub(1).ok_or_else(|| broken("an inline part has no first mark"))?;
    let (Some(&(char, lo)), hi) = (marks.get(m), marks.get(m.saturating_add(1)).map_or(bytes.len(), |next| next.1)) else {
        return Err(broken("an inline part's mark is missing"));
    };
    let piece = bytes.get(lo..hi).ok_or_else(|| broken("an inline part's mark is past its octets"))?;
    Ok((char, decoded(piece)?.chars().collect()))
}

/// Of slice `s`, the part of the chunk of its source its character `at`
/// is in that is within it: the character of the slice it begins at, and
/// its characters.
fn slice_chunk(s: &Slice, at: u64) -> Result<(u64, Vec<char>)> {
    let directory = s.source.directory()?;
    let (k, _, start) = directory.by_char(s.char.saturating_add(at)).ok_or_else(|| broken("a slice goes past its source's chunks"))?;
    let size = directory.chunks().get(k).map_or(0, |c| u64::from(c.chars));
    let first = start.max(s.char);
    let last = start.saturating_add(size).min(s.char.saturating_add(s.chars));
    let bytes = s.source.chunk(k)?;
    let skip = usize::try_from(first.saturating_sub(start)).unwrap_or(usize::MAX);
    let take = usize::try_from(last.saturating_sub(first)).unwrap_or(usize::MAX);
    Ok((first.saturating_sub(s.char), decoded(&bytes)?.chars().skip(skip).take(take).collect()))
}

#[cfg(test)]
mod tests;
