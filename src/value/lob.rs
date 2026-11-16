//! [ISO/IEC 9075-2:2023 §4.3, §4.4] Large object strings, CHARACTER LARGE
//! OBJECT and BINARY LARGE OBJECT, as values (RULES.md D79).
//!
//! \[DESIGN\] A large object is never held whole in memory, sent whole or
//! logged whole. Its value is a list of parts: octets held inline, a slice
//! of a source that keeps its octets in chunks (a table's large object, a
//! temporary one in a scratch file), or, at a client, a locator naming one
//! a server holds [§4.37.5]. Concatenation joins the lists and SUBSTRING
//! slices them, so neither copies a value's octets; what reads it reads a
//! chunk at a time (`lob/reader.rs`), and what makes a new one writes a
//! chunk at a time (`lob/builder.rs`).
//!
//! A character large object's octets are UTF-8, every chunk and part cut
//! at a character's boundary, and each knows how many characters it holds,
//! so a position in characters is found without reading what is before it.

mod builder;
#[cfg(test)]
pub(crate) mod fixtures;
mod reader;
#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::Arc;

pub use builder::{LobBuilder, TempLobs, TempWriter};
pub use reader::{LobChars, LobReader};

use crate::error::{Code, Result, SqlError};

/// \[DESIGN\] The most octets a chunk of a large object holds (RULES.md
/// D79): one read, one log record, one page out of line.
pub const CHUNK_OCTETS: usize = 262_144;

/// The longest prefix of `bytes`, at most `most` octets, of whole UTF-8
/// characters: where a chunk of a character large object is cut. A
/// character that the end of `bytes` splits is left out too, its rest yet
/// to come; 0 if no character ends within `most`.
#[must_use]
pub fn char_cut(bytes: &[u8], most: usize) -> usize {
    let mut cut = most.min(bytes.len());
    if cut < bytes.len() {
        while cut > 0 && bytes.get(cut).is_some_and(|b| b & 0xC0 == 0x80) {
            cut = cut.saturating_sub(1);
        }
        return cut;
    }
    // At the end: the last character's lead octet, at most 3 back.
    let Some(lead) = (1..=4).filter_map(|back| cut.checked_sub(back)).find(|&i| bytes.get(i).is_some_and(|b| b & 0xC0 != 0x80)) else {
        return cut;
    };
    let width = bytes.get(lead).map_or(1, |&b| match b.leading_ones() {
        2 => 2,
        3 => 3,
        4 => 4,
        _ => 1,
    });
    if lead.saturating_add(width) > cut { lead } else { cut }
}

/// \[DESIGN\] The most octets a part held inline keeps before a builder
/// moves what it holds to a source of chunks: one chunk's.
pub const INLINE_OCTETS: usize = CHUNK_OCTETS;

/// [ISO/IEC 9075-2:2023 §6.1 SR17] The greatest `<large object length>`
/// (IL042), in octets of a BINARY LARGE OBJECT or characters of a
/// CHARACTER LARGE OBJECT: 4G.
pub const MAX_LOB_LENGTH: u64 = 4_294_967_296;

/// Which large object string type a value is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LobKind {
    /// [ISO/IEC 9075-2:2023 §4.3] CHARACTER LARGE OBJECT, its octets
    /// UTF-8.
    Character,
    /// [ISO/IEC 9075-2:2023 §4.4] BINARY LARGE OBJECT.
    Binary,
}

/// The size of one chunk of a source: its octets, and its characters,
/// which are its octets in a binary large object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    /// Its octets.
    pub octets: u32,
    /// Its characters.
    pub chars: u32,
}

/// Where a source's chunks begin, found once: each chunk's size, and the
/// octets and characters before it.
#[derive(Debug, Clone, Default)]
pub struct Directory {
    chunks: Box<[Chunk]>,
    starts: Box<[(u64, u64)]>,
}

impl Directory {
    /// The directory of chunks of `sizes`, in order.
    pub fn new(sizes: Vec<Chunk>) -> Directory {
        let mut starts = Vec::with_capacity(sizes.len());
        let (mut octets, mut chars) = (0_u64, 0_u64);
        for c in &sizes {
            starts.push((octets, chars));
            octets = octets.saturating_add(u64::from(c.octets));
            chars = chars.saturating_add(u64::from(c.chars));
        }
        Directory { chunks: sizes.into_boxed_slice(), starts: starts.into_boxed_slice() }
    }

    /// Its chunks' sizes, in order.
    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }

    /// The octets and characters of all its chunks.
    pub fn totals(&self) -> (u64, u64) {
        match (self.starts.last(), self.chunks.last()) {
            (Some(&(o, c)), Some(last)) => (o.saturating_add(u64::from(last.octets)), c.saturating_add(u64::from(last.chars))),
            (None, _) | (_, None) => (0, 0),
        }
    }

    /// The chunk holding octet `at`, from 0, and the octet and character
    /// it begins at; none past the last.
    pub fn by_octet(&self, at: u64) -> Option<(usize, u64, u64)> {
        let k = self.starts.partition_point(|&(o, _)| o <= at).checked_sub(1)?;
        let (o, c) = *self.starts.get(k)?;
        let size = self.chunks.get(k)?;
        (at < o.saturating_add(u64::from(size.octets))).then_some((k, o, c))
    }

    /// The chunk holding character `at`, from 0, and the octet and
    /// character it begins at; none past the last.
    pub fn by_char(&self, at: u64) -> Option<(usize, u64, u64)> {
        let k = self.starts.partition_point(|&(_, c)| c <= at).checked_sub(1)?;
        let (o, c) = *self.starts.get(k)?;
        let size = self.chunks.get(k)?;
        (at < c.saturating_add(u64::from(size.chars))).then_some((k, o, c))
    }
}

/// Where a source keeps its large object, if a table does: the table, as
/// its engine names it while open, and the object's id in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Kept {
    /// The table, of all tables open.
    pub table: u64,
    /// The large object's id in the table.
    pub id: u64,
}

/// \[DESIGN\] A large object kept in chunks (RULES.md D79): a table's,
/// handed out by its storage engine (an extension point, R-SAFE-3), or a
/// temporary one of a scratch file. Its chunks never change once made.
pub trait LobSource: Send + Sync + fmt::Debug {
    /// Its octets and characters, known without reading its chunks.
    fn totals(&self) -> (u64, u64);
    /// Its chunks, read when first asked for.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if they cannot be read, or have been freed.
    fn directory(&self) -> Result<&Directory>;
    /// The octets of chunk `k`, from 0.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if they cannot be read, or have been freed (a
    /// transaction that made it was rolled back).
    fn chunk(&self, k: usize) -> Result<Arc<[u8]>>;
    /// The table that keeps it, and its id there, if a table does.
    fn kept(&self) -> Option<Kept>;
}

/// A slice of a source: its octets and characters from a chunk boundary
/// or within one.
#[derive(Debug, Clone)]
pub struct Slice {
    /// What keeps the octets.
    pub source: Arc<dyn LobSource>,
    /// The octet of the source it begins at, from 0.
    pub octet: u64,
    /// The character of the source it begins at, from 0.
    pub char: u64,
    /// Its octets.
    pub octets: u64,
    /// Its characters.
    pub chars: u64,
}

impl Slice {
    /// Whether it is the whole of its source.
    pub fn whole(&self) -> bool {
        self.octet == 0 && (self.octets, self.chars) == self.source.totals()
    }
}

/// One part of a large object's value.
#[derive(Debug, Clone)]
pub enum Part {
    /// Octets held in memory, and their characters.
    Inline(Arc<[u8]>, u64),
    /// A slice of a source of chunks.
    Slice(Slice),
    /// \[DESIGN\] At a client, a locator a server gave [ISO/IEC 9075-2:2023
    /// §4.37.5]: the whole value is the server's (RULES.md D45).
    Locator(u32),
    /// \[DESIGN\] The whole of the large object of this id the table of the
    /// row holding it keeps, as a record and the log name it, before the
    /// table makes it a slice it reads (RULES.md D79).
    Reference(u64),
}

impl Part {
    /// Its octets and characters.
    fn size(&self, kind: LobKind) -> (u64, u64) {
        match self {
            Part::Inline(bytes, chars) => (len(bytes), if kind == LobKind::Binary { len(bytes) } else { *chars }),
            Part::Slice(s) => (s.octets, s.chars),
            Part::Locator(_) | Part::Reference(_) => (0, 0),
        }
    }
}

/// `bytes`' length as a count.
fn len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

/// The characters of `bytes`, UTF-8 cut at characters' boundaries.
pub(crate) fn count_chars(bytes: &[u8]) -> u64 {
    len(bytes).saturating_sub(bytes.iter().filter(|&&b| b & 0xC0 == 0x80).count().try_into().unwrap_or(0))
}

/// [ISO/IEC 9075-2:2023 §4.3, §4.4] A value of a large object string type.
#[derive(Debug, Clone)]
pub struct Lob {
    kind: LobKind,
    octets: u64,
    chars: u64,
    parts: Arc<[Part]>,
}

impl Lob {
    /// The empty large object of `kind`.
    pub fn empty(kind: LobKind) -> Lob {
        Lob { kind, octets: 0, chars: 0, parts: Arc::from(Vec::new()) }
    }

    /// The character large object of `text`, held inline.
    pub fn of_text(text: &str) -> Lob {
        let bytes = text.as_bytes();
        let chars = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
        Lob::of_parts(LobKind::Character, vec![Part::Inline(Arc::from(bytes), chars)])
    }

    /// The binary large object of `octets`, held inline.
    pub fn of_octets(octets: &[u8]) -> Lob {
        Lob::of_parts(LobKind::Binary, vec![Part::Inline(Arc::from(octets), len(octets))])
    }

    /// The whole of `source`, of `kind`.
    pub fn of_source(kind: LobKind, source: Arc<dyn LobSource>) -> Lob {
        let (octets, chars) = source.totals();
        Lob::of_parts(kind, vec![Part::Slice(Slice { source, octet: 0, char: 0, octets, chars })])
    }

    /// \[DESIGN\] At a client, the value a server's locator `locator` names,
    /// of `kind`, `octets` and `chars` (RULES.md D45).
    pub fn of_locator(kind: LobKind, locator: u32, (octets, chars): (u64, u64)) -> Lob {
        Lob { kind, octets, chars, parts: Arc::from(vec![Part::Locator(locator)]) }
    }

    /// \[DESIGN\] The whole of the large object of id `id` its row's table
    /// keeps, of `kind`, `octets` and `chars`, unread (RULES.md D79).
    pub fn of_reference(kind: LobKind, id: u64, (octets, chars): (u64, u64)) -> Lob {
        Lob { kind, octets, chars, parts: Arc::from(vec![Part::Reference(id)]) }
    }

    /// The value of `parts`, empty ones left out, of `kind`.
    pub fn of_parts(kind: LobKind, parts: Vec<Part>) -> Lob {
        let parts: Vec<Part> =
            parts.into_iter().filter(|p| p.size(kind).0 > 0 || matches!(p, Part::Locator(_) | Part::Reference(_))).collect();
        let (octets, chars) =
            parts.iter().map(|p| p.size(kind)).fold((0_u64, 0_u64), |(o, c), (po, pc)| (o.saturating_add(po), c.saturating_add(pc)));
        Lob { kind, octets, chars, parts: Arc::from(parts) }
    }

    /// Its kind.
    pub fn kind(&self) -> LobKind {
        self.kind
    }

    /// Its length in octets.
    pub fn octets(&self) -> u64 {
        self.octets
    }

    /// Its length in characters: its octets, of a binary large object.
    pub fn chars(&self) -> u64 {
        if self.kind == LobKind::Binary { self.octets } else { self.chars }
    }

    /// Its parts.
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// The locator naming it, if it is a client's value a server holds.
    pub fn locator(&self) -> Option<u32> {
        match self.parts.first() {
            Some(Part::Locator(l)) => Some(*l),
            Some(Part::Inline(..) | Part::Slice(_) | Part::Reference(_)) | None => None,
        }
    }

    /// Its octets, if all are held inline in one part, or it has none.
    pub fn inline(&self) -> Option<&[u8]> {
        match &*self.parts {
            [] => Some(&[]),
            [Part::Inline(bytes, _)] => Some(bytes),
            _ => None,
        }
    }

    /// The bytes it holds in memory: its parts, and the octets of those
    /// held inline, not those of a source, which it reads by chunks.
    pub fn held_bytes(&self) -> usize {
        self.parts.iter().fold(size_of_val::<[Part]>(&self.parts), |sum, p| match p {
            Part::Inline(bytes, _) => sum.saturating_add(bytes.len()),
            Part::Slice(_) | Part::Locator(_) | Part::Reference(_) => sum,
        })
    }

    /// [ISO/IEC 9075-2:2023 §8.2 GR3, GR5] How it compares with `other`, of
    /// its kind, read a chunk at a time: equal exactly when they are, two
    /// character large objects after the shorter is padded with spaces
    /// \[GR3a\], two binary ones octet by octet, of one length \[GR5\]; if
    /// not, by the first octet that differs, an order the equality
    /// operations alone read, as no large object is ordered [§9.14 SR3].
    ///
    /// # Errors
    ///
    /// As reading a chunk fails.
    pub fn compare_to(&self, other: &Lob) -> Result<std::cmp::Ordering> {
        use std::cmp::Ordering;
        if let (Some(a), Some(b)) = (self.inline(), other.inline())
            && self.kind == LobKind::Binary
        {
            return Ok(a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
        }
        if self.kind == LobKind::Binary && self.octets != other.octets {
            return Ok(self.octets.cmp(&other.octets));
        }
        let common = self.octets.min(other.octets);
        let (mut at, mut order, mut theirs) = (0_u64, Ordering::Equal, other.reader());
        self.reader().each(0, common, &mut |piece| {
            let them = theirs.read(at, len(piece))?;
            order = piece.cmp(them.as_slice());
            at = at.saturating_add(len(piece));
            Ok(order == Ordering::Equal)
        })?;
        if order != Ordering::Equal || self.octets == other.octets {
            return Ok(order);
        }
        // [GR3a] The rest of the longer, against spaces.
        let (longer, sign) = if self.octets > other.octets { (self, Ordering::Greater) } else { (other, Ordering::Less) };
        let mut rest = Ordering::Equal;
        longer.reader().each(common, longer.octets, &mut |piece| {
            rest = piece.iter().find(|&&b| b != b' ').map_or(Ordering::Equal, |&b| b.cmp(&b' '));
            Ok(rest == Ordering::Equal)
        })?;
        Ok(if sign == Ordering::Greater { rest } else { rest.reverse() })
    }

    /// A reader of it.
    pub fn reader(&self) -> LobReader<'_> {
        LobReader::new(self)
    }

    /// [ISO/IEC 9075-2:2023 §6.32 GR2, GR3] `self` followed by `other`,
    /// copying no octets but those of short parts held inline, which are
    /// joined.
    #[must_use]
    pub fn concat(&self, other: &Lob) -> Lob {
        let mut parts: Vec<Part> = self.parts.to_vec();
        for p in other.parts.iter() {
            match (parts.last_mut(), p) {
                (Some(Part::Inline(a, ac)), Part::Inline(b, bc)) if a.len().saturating_add(b.len()) <= INLINE_OCTETS => {
                    let mut joined = a.to_vec();
                    joined.extend_from_slice(b);
                    *a = Arc::from(joined);
                    *ac = ac.saturating_add(*bc);
                }
                (Some(Part::Inline(..) | Part::Slice(_) | Part::Locator(_) | Part::Reference(_)) | None, _) => parts.push(p.clone()),
            }
        }
        Lob::of_parts(self.kind, parts)
    }

    /// Its octets from octet `from`, from 0, `count` of them, or those
    /// there are: of a binary large object, whose characters are its
    /// octets.
    #[must_use]
    pub fn slice_octets(&self, from: u64, count: u64) -> Lob {
        let mut parts = Vec::new();
        let mut at = 0_u64;
        let end = from.saturating_add(count);
        for p in self.parts.iter() {
            let (size, _) = p.size(LobKind::Binary);
            let (lo, hi) = (from.max(at), end.min(at.saturating_add(size)));
            if lo < hi {
                let (off, n) = (lo.saturating_sub(at), hi.saturating_sub(lo));
                parts.push(match p {
                    Part::Inline(bytes, _) => {
                        let cut = cut_bytes(bytes, off, n);
                        Part::Inline(Arc::from(cut), len(cut))
                    }
                    Part::Slice(s) => Part::Slice(Slice {
                        octet: s.octet.saturating_add(off),
                        char: s.char.saturating_add(off),
                        octets: n,
                        chars: n,
                        ..s.clone()
                    }),
                    Part::Locator(l) => Part::Locator(*l),
                    Part::Reference(id) => Part::Reference(*id),
                });
            }
            at = at.saturating_add(size);
        }
        Lob::of_parts(self.kind, parts)
    }

    /// Its characters from character `from`, from 0, `count` of them, or
    /// those there are; of a character large object, the positions in
    /// octets of its ends found by reading the chunks they are in.
    ///
    /// # Errors
    ///
    /// As reading a chunk fails.
    pub fn slice_chars(&self, from: u64, count: u64) -> Result<Lob> {
        if self.kind == LobKind::Binary {
            return Ok(self.slice_octets(from, count));
        }
        let mut parts = Vec::new();
        let mut at = 0_u64;
        let end = from.saturating_add(count);
        for p in self.parts.iter() {
            let (_, size) = p.size(self.kind);
            let (lo, hi) = (from.max(at), end.min(at.saturating_add(size)));
            if lo < hi {
                parts.push(reader::cut_chars(p, lo.saturating_sub(at), hi.saturating_sub(lo))?);
            }
            at = at.saturating_add(size);
        }
        Ok(Lob::of_parts(self.kind, parts))
    }

    /// Its whole value in memory, if it has no more than `most` octets:
    /// its text, of a character large object, or its octets.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a chunk cannot be read or is not UTF-8; none if it
    /// is longer than `most`.
    pub fn bounded(&self, most: u64) -> Result<Option<Vec<u8>>> {
        if self.octets > most {
            return Ok(None);
        }
        let mut out = Vec::with_capacity(usize::try_from(self.octets).unwrap_or(0));
        self.reader().each(0, self.octets, &mut |piece| {
            out.extend_from_slice(piece);
            Ok(true)
        })?;
        Ok(Some(out))
    }
}

/// \[DESIGN\] A large object as a key of identity (`Value::norm`): two are
/// equal if both are held inline with the same octets, or are the same
/// value, its parts shared. Equal values not held inline may be unequal
/// keys, which only makes a cache keyed by them miss; unequal values are
/// never equal keys, as the key holds its parts, so their address is not
/// given to another value while it lives. No large object is grouped
/// [ISO/IEC 9075-2:2023 §9.12 SR3], so nothing groups by this key.
#[derive(Debug, Clone)]
pub struct LobIdentity(pub Lob);

impl LobIdentity {
    /// The address of its parts, of a value not held inline.
    fn address(&self) -> usize {
        Arc::as_ptr(&self.0.parts).cast::<u8>().addr()
    }
}

impl PartialEq for LobIdentity {
    fn eq(&self, other: &LobIdentity) -> bool {
        if self.0.kind != other.0.kind {
            return false;
        }
        match (self.0.inline(), other.0.inline()) {
            (Some(a), Some(b)) => a == b,
            (None, None) => Arc::ptr_eq(&self.0.parts, &other.0.parts),
            (Some(_), None) | (None, Some(_)) => false,
        }
    }
}

impl Eq for LobIdentity {}

impl std::hash::Hash for LobIdentity {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.kind.hash(state);
        match self.0.inline() {
            Some(bytes) => bytes.hash(state),
            None => self.address().hash(state),
        }
    }
}

impl LobIdentity {
    /// Its bytes as a key: its kind, then its octets if held inline, else
    /// the address of its parts, which the value spilled with it keeps.
    pub fn put_key(&self, out: &mut Vec<u8>) {
        out.push(match self.0.kind {
            LobKind::Character => 0,
            LobKind::Binary => 1,
        });
        if let Some(bytes) = self.0.inline() {
            out.push(0);
            out.extend_from_slice(&len(bytes).to_le_bytes());
            out.extend_from_slice(bytes);
        } else {
            out.push(1);
            out.extend_from_slice(&u64::try_from(self.address()).unwrap_or(u64::MAX).to_le_bytes());
        }
    }
}

/// The `count` octets of `bytes` from octet `from`, from 0, or those
/// there are.
pub(crate) fn cut_bytes(bytes: &[u8], from: u64, count: u64) -> &[u8] {
    let lo = usize::try_from(from).unwrap_or(usize::MAX).min(bytes.len());
    let hi = usize::try_from(from.saturating_add(count)).unwrap_or(usize::MAX).min(bytes.len());
    bytes.get(lo..hi).unwrap_or_default()
}

impl fmt::Display for Lob {
    /// Its text if held inline, as a short value is; else what it is,
    /// which a client reads by its locator.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.inline(), self.kind) {
            (Some(bytes), LobKind::Character) => f.write_str(&String::from_utf8_lossy(bytes)),
            (Some(bytes), LobKind::Binary) => bytes.iter().try_for_each(|b| write!(f, "{b:02X}")),
            (None, LobKind::Character) => write!(f, "<CHARACTER LARGE OBJECT of {} characters>", self.chars),
            (None, LobKind::Binary) => write!(f, "<BINARY LARGE OBJECT of {} octets>", self.octets),
        }
    }
}

/// \[DESIGN\] A source broken: what it keeps is not what its directory
/// says, or not UTF-8 of a character large object (SQLCODE -910).
pub(crate) fn broken(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] a large object is broken: {what}"))
}
