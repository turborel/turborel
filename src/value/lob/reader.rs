//! \[DESIGN\] Reading a large object (RULES.md D79): a range of its octets
//! a piece at a time, each piece a part held inline or a chunk of a source,
//! the last chunk read kept for the next read; its characters one by one
//! from any character; and a part cut by characters, whose ends in octets
//! are found by reading only the chunks they fall in.

use std::sync::Arc;

use super::{Lob, LobKind, LobSource, Part, Slice, broken, count_chars, cut_bytes, len};
use crate::error::Result;

/// A reader of a large object.
#[derive(Debug)]
pub struct LobReader<'a> {
    lob: &'a Lob,
    cached: Option<Cached>,
}

/// The chunk a reader read last: its source, its number and its octets.
type Cached = (Arc<dyn LobSource>, usize, Arc<[u8]>);

impl<'a> LobReader<'a> {
    /// A reader of `lob`.
    pub fn new(lob: &'a Lob) -> LobReader<'a> {
        LobReader { lob, cached: None }
    }

    /// Chunk `k` of `source`, read or kept from the last read.
    fn chunk(&mut self, source: &Arc<dyn LobSource>, k: usize) -> Result<Arc<[u8]>> {
        if let Some((s, at, bytes)) = &self.cached
            && Arc::ptr_eq(s, source)
            && *at == k
        {
            return Ok(Arc::clone(bytes));
        }
        let bytes = source.chunk(k)?;
        self.cached = Some((Arc::clone(source), k, Arc::clone(&bytes)));
        Ok(bytes)
    }

    /// Gives `f` the octets of the value from octet `from`, from 0, `count`
    /// of them or those there are, a piece at a time, in order, until it
    /// returns false; whether it never did.
    ///
    /// # Errors
    ///
    /// As reading a chunk fails, or `f` does; SQLCODE -910 of a locator,
    /// whose value a client reads from its server.
    pub fn each(&mut self, from: u64, count: u64, f: &mut dyn FnMut(&[u8]) -> Result<bool>) -> Result<bool> {
        let end = from.saturating_add(count).min(self.lob.octets());
        let mut at = 0_u64;
        let parts = Arc::clone(&self.lob.parts);
        for p in parts.iter() {
            let size = part_octets(p);
            let (lo, hi) = (from.max(at), end.min(at.saturating_add(size)));
            if lo < hi && !self.each_in_part(p, lo.saturating_sub(at), hi.saturating_sub(lo), f)? {
                return Ok(false);
            }
            at = at.saturating_add(size);
            if at >= end {
                break;
            }
        }
        Ok(true)
    }

    /// `each` within part `p`, its octets from `from`, `count` of them.
    fn each_in_part(&mut self, p: &Part, from: u64, count: u64, f: &mut dyn FnMut(&[u8]) -> Result<bool>) -> Result<bool> {
        match p {
            Part::Inline(bytes, _) => f(cut_bytes(bytes, from, count)),
            Part::Slice(s) => {
                let (mut at, end) = (s.octet.saturating_add(from), s.octet.saturating_add(from).saturating_add(count));
                while at < end {
                    let (k, start, _) =
                        s.source.directory()?.by_octet(at).ok_or_else(|| broken("a slice goes past its source's chunks"))?;
                    let bytes = self.chunk(&s.source, k)?;
                    let piece = cut_bytes(&bytes, at.saturating_sub(start), end.saturating_sub(at));
                    if piece.is_empty() {
                        return Err(broken("a chunk is shorter than its directory says"));
                    }
                    if !f(piece)? {
                        return Ok(false);
                    }
                    at = at.saturating_add(len(piece));
                }
                Ok(true)
            }
            Part::Locator(_) => Err(broken("a locator's value is its server's, read by the client from it")),
            Part::Reference(_) => Err(broken("a reference is read once its table makes it a slice")),
        }
    }

    /// The value's octets from octet `from`, `count` of them or those there
    /// are, in memory: for a range known to be short.
    ///
    /// # Errors
    ///
    /// As `each`.
    pub fn read(&mut self, from: u64, count: u64) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(usize::try_from(count.min(self.lob.octets())).unwrap_or(0));
        self.each(from, count, &mut |piece| {
            out.extend_from_slice(piece);
            Ok(true)
        })?;
        Ok(out)
    }

    /// The value's text from character `from`, `count` characters or those
    /// there are, in memory: of a character large object, for a range
    /// known to be short.
    ///
    /// # Errors
    ///
    /// As `each`; SQLCODE -910 if the octets are not UTF-8.
    pub fn text(&mut self, from: u64, count: u64) -> Result<String> {
        let sliced = self.lob.slice_chars(from, count)?;
        let bytes = LobReader::new(&sliced).read(0, sliced.octets())?;
        String::from_utf8(bytes).map_err(|_| broken("a character large object's octets are not UTF-8"))
    }

    /// The octet character `at`, from 0, begins at; the value's octets at
    /// its end.
    ///
    /// # Errors
    ///
    /// As reading the chunk it is in fails.
    pub fn octet_of_char(&mut self, at: u64) -> Result<u64> {
        if self.lob.kind() == LobKind::Binary {
            return Ok(at.min(self.lob.octets()));
        }
        let (mut octets, mut chars) = (0_u64, 0_u64);
        for p in self.lob.parts.iter() {
            let (po, pc) = (part_octets(p), part_chars(p));
            if at < chars.saturating_add(pc) {
                return Ok(octets.saturating_add(octet_in_part(p, at.saturating_sub(chars))?));
            }
            octets = octets.saturating_add(po);
            chars = chars.saturating_add(pc);
        }
        Ok(octets)
    }
}

/// The octets of `p`.
fn part_octets(p: &Part) -> u64 {
    match p {
        Part::Inline(bytes, _) => len(bytes),
        Part::Slice(s) => s.octets,
        Part::Locator(_) | Part::Reference(_) => 0,
    }
}

/// The characters of `p`.
fn part_chars(p: &Part) -> u64 {
    match p {
        Part::Inline(_, chars) => *chars,
        Part::Slice(s) => s.chars,
        Part::Locator(_) | Part::Reference(_) => 0,
    }
}

/// The octet of part `p` its character `at`, from 0, begins at.
fn octet_in_part(p: &Part, at: u64) -> Result<u64> {
    match p {
        Part::Inline(bytes, _) => Ok(octet_in(bytes, at)),
        Part::Slice(s) => Ok(source_octet(s, s.char.saturating_add(at))?.saturating_sub(s.octet)),
        Part::Locator(_) | Part::Reference(_) => Err(broken("a locator's or reference's value is read where it is kept")),
    }
}

/// The octet of UTF-8 `bytes` its character `at` begins at, or its length.
fn octet_in(bytes: &[u8], at: u64) -> u64 {
    let mut seen = 0_u64;
    for (i, b) in bytes.iter().enumerate() {
        if b & 0xC0 != 0x80 {
            if seen == at {
                return u64::try_from(i).unwrap_or(u64::MAX);
            }
            seen = seen.saturating_add(1);
        }
    }
    len(bytes)
}

/// The octet of `s`'s source its character `at` begins at, from 0; the
/// source's octets at its end.
fn source_octet(s: &Slice, at: u64) -> Result<u64> {
    let directory = s.source.directory()?;
    match directory.by_char(at) {
        Some((k, octet, char)) => {
            let bytes = s.source.chunk(k)?;
            Ok(octet.saturating_add(octet_in(&bytes, at.saturating_sub(char))))
        }
        None => Ok(directory.totals().0),
    }
}

/// The part of `p` of `count` characters from character `from` of it, from
/// 0, of a character large object.
///
/// # Errors
///
/// As reading a chunk an end falls in fails.
pub(super) fn cut_chars(p: &Part, from: u64, count: u64) -> Result<Part> {
    match p {
        Part::Inline(bytes, _) => {
            let lo = octet_in(bytes, from);
            let hi = octet_in(bytes, from.saturating_add(count));
            let cut = cut_bytes(bytes, lo, hi.saturating_sub(lo));
            Ok(Part::Inline(Arc::from(cut), count_chars(cut)))
        }
        Part::Slice(s) => {
            let (lo_char, hi_char) = (s.char.saturating_add(from), s.char.saturating_add(from).saturating_add(count));
            let lo = source_octet(s, lo_char)?;
            let hi = if hi_char >= s.char.saturating_add(s.chars) { s.octet.saturating_add(s.octets) } else { source_octet(s, hi_char)? };
            Ok(Part::Slice(Slice { octet: lo, char: lo_char, octets: hi.saturating_sub(lo), chars: count, source: Arc::clone(&s.source) }))
        }
        Part::Locator(l) => Ok(Part::Locator(*l)),
        Part::Reference(id) => Ok(Part::Reference(*id)),
    }
}

/// The characters of a character large object from one of them, read a
/// chunk at a time.
#[derive(Debug)]
pub struct LobChars {
    lob: Lob,
    /// The octet of the value read next.
    at: u64,
    /// The octets read and not yet given, and where in them.
    piece: Vec<u8>,
    used: usize,
}

impl LobChars {
    /// The characters of `lob` from character `from`, from 0.
    ///
    /// # Errors
    ///
    /// As finding where it begins fails.
    pub fn new(lob: &Lob, from: u64) -> Result<LobChars> {
        let at = lob.reader().octet_of_char(from)?;
        Ok(LobChars { lob: lob.clone(), at, piece: Vec::new(), used: 0 })
    }

    /// Reads the next chunk's worth into the piece; false at the end.
    fn fill(&mut self) -> Result<bool> {
        if self.at >= self.lob.octets() {
            return Ok(false);
        }
        let want = u64::try_from(super::CHUNK_OCTETS).unwrap_or(u64::MAX);
        let mut got = Vec::new();
        let mut reader = self.lob.reader();
        reader.each(self.at, want, &mut |piece| {
            got.extend_from_slice(piece);
            Ok(false)
        })?;
        if got.is_empty() {
            return Ok(false);
        }
        self.at = self.at.saturating_add(len(&got));
        self.piece = got;
        self.used = 0;
        Ok(true)
    }

    /// The next character, if any.
    ///
    /// # Errors
    ///
    /// As reading a chunk fails; SQLCODE -910 if the octets are not UTF-8.
    pub fn next_char(&mut self) -> Result<Option<char>> {
        if self.used >= self.piece.len() && !self.fill()? {
            return Ok(None);
        }
        let rest = self.piece.get(self.used..).unwrap_or_default();
        let width = rest.first().map_or(1, |&b| utf8_width(b)).min(rest.len());
        let text = std::str::from_utf8(rest.get(..width).unwrap_or_default())
            .map_err(|_| broken("a character large object's octets are not UTF-8"))?;
        let c = text.chars().next().ok_or_else(|| broken("a character large object's octets are not UTF-8"))?;
        self.used = self.used.saturating_add(width);
        Ok(Some(c))
    }
}

/// The octets of the UTF-8 character `lead` begins.
fn utf8_width(lead: u8) -> usize {
    match lead.leading_ones() {
        2 => 2,
        3 => 3,
        4 => 4,
        _ => 1,
    }
}
