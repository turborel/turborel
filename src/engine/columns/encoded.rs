//! \[DESIGN\] How a column chunk's values are written (RULES.md D31): of the
//! encodings that can hold them, the one that takes fewest bytes, so a page
//! holds more of them. After the chunk's kind, count and nulls:
//!
//! ```text
//! FIXED:       width u32 | count × width bytes, a null's zeroed
//! VARYING:     (count + 1) × u32, where each value begins and the last ends
//!              | the bytes
//! CONSTANT:    width u32 | the one value not null, if any
//! FRAME:       bytes u8 | least i64 | count × bytes, each value of 8 bytes as
//!              a signed integer less the least, a null's zeroed
//! DICTIONARY:  code u8 | entries u32 | (entries + 1) × u32, where each
//!              entry begins and the last ends | the entries | count × code
//!              bytes, each value's entry, a null's 0
//! ```
//!
//! Integers are little-endian. Each value is as its type writes it in a
//! record, so every type is kept, and reading one encoded is as fast as
//! reading it plain, a value at a time: frame of reference needs only that
//! a field be 8 bytes, any 8 bytes being a signed integer. Split from
//! columns.rs (RULES.md R-SIZE-2).

use std::collections::HashMap;

use super::{HEADER, WORD, corrupt, nulls_len, read_word, too_large, word};
use crate::error::Result;
use crate::value::{DataType, Value};

pub(super) const FIXED: u8 = 1;
pub(super) const VARYING: u8 = 2;
pub(super) const CONSTANT: u8 = 3;
pub(super) const FRAME: u8 = 4;
pub(super) const DICTIONARY: u8 = 5;
/// The bytes of a field frame of reference takes.
const FRAMED: usize = 8;
/// The most entries of a dictionary: as many as a code of two bytes names.
const MOST_ENTRIES: usize = 1 << 16;
/// Entries a code of one byte names.
const SMALL_ENTRIES: usize = 1 << 8;

/// How a chunk's values are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Plan {
    Fixed(usize),
    Varying,
    Constant,
    /// Frame of reference, each value less the least in this many bytes.
    Frame(i64, usize),
    /// A dictionary, each value's entry in a code of this many bytes.
    Dictionary(usize),
}

/// The bytes the values of a chunk take, in each encoding that can hold
/// them, kept as values are added.
pub(super) struct Sizer<'f> {
    count: usize,
    compress: bool,
    /// The width of the type's fields, while every one is of it.
    width: Option<usize>,
    varying: usize,
    /// The first value not null, and whether every one since is it.
    first: Option<&'f [u8]>,
    same: bool,
    /// The least and greatest values of 8 bytes, while every one is so.
    range: Option<(i64, i64)>,
    framed: bool,
    entries: HashMap<&'f [u8], usize>,
    entry_bytes: usize,
}

/// The bytes a difference of `span` takes, of 1, 2, 4 or 8.
fn bytes_for(span: u128) -> usize {
    if span <= u128::from(u8::MAX) {
        1
    } else if span <= u128::from(u16::MAX) {
        2
    } else if span <= u128::from(u32::MAX) {
        4
    } else {
        FRAMED
    }
}

fn integer(field: &[u8]) -> Option<i64> {
    <[u8; FRAMED]>::try_from(field).ok().map(i64::from_le_bytes)
}

impl<'f> Sizer<'f> {
    /// No values yet, of fields of `width` if the type has one; encoded
    /// plain only, unless `compress`.
    pub(super) fn new(width: Option<usize>, compress: bool) -> Sizer<'f> {
        Sizer {
            count: 0,
            compress,
            width,
            varying: 0,
            first: None,
            same: true,
            range: None,
            framed: width == Some(FRAMED),
            entries: HashMap::new(),
            entry_bytes: 0,
        }
    }

    /// Adds a value, none for a null.
    pub(super) fn add(&mut self, field: Option<&'f [u8]>) {
        self.count = self.count.saturating_add(1);
        let Some(f) = field else { return };
        self.varying = self.varying.saturating_add(f.len());
        if self.width.is_some_and(|w| w != f.len()) {
            self.width = None;
        }
        match self.first {
            None => self.first = Some(f),
            Some(first) => self.same = self.same && first == f,
        }
        match (integer(f), self.framed) {
            (Some(n), true) => self.range = Some(self.range.map_or((n, n), |(lo, hi)| (lo.min(n), hi.max(n)))),
            (None, _) | (_, false) => self.framed = false,
        }
        if self.entries.len() < MOST_ENTRIES && !self.entries.contains_key(f) {
            let next = self.entries.len();
            self.entries.insert(f, next);
            self.entry_bytes = self.entry_bytes.saturating_add(f.len());
        }
    }

    /// The encoding that takes the fewest bytes, and how many.
    pub(super) fn best(&self) -> (Plan, usize) {
        let base = HEADER.saturating_add(nulls_len(self.count));
        let plain = match self.width {
            Some(w) => (Plan::Fixed(w), base.saturating_add(WORD).saturating_add(self.count.saturating_mul(w))),
            None => (Plan::Varying, base.saturating_add(self.count.saturating_add(1).saturating_mul(WORD)).saturating_add(self.varying)),
        };
        if !self.compress {
            return plain;
        }
        let mut found = vec![plain];
        if self.same {
            found.push((Plan::Constant, base.saturating_add(WORD).saturating_add(self.first.map_or(0, <[u8]>::len))));
        }
        if let (true, Some((lo, hi))) = (self.framed, self.range) {
            let bytes = bytes_for(u128::try_from(i128::from(hi).saturating_sub(i128::from(lo))).unwrap_or(u128::MAX));
            found.push((Plan::Frame(lo, bytes), base.saturating_add(1 + FRAMED).saturating_add(self.count.saturating_mul(bytes))));
        }
        if self.entries.len() < MOST_ENTRIES {
            let code = if self.entries.len() < SMALL_ENTRIES { 1 } else { 2 };
            let table = self.entries.len().saturating_add(1).saturating_mul(WORD).saturating_add(self.entry_bytes);
            found.push((
                Plan::Dictionary(code),
                base.saturating_add(1 + WORD).saturating_add(table).saturating_add(self.count.saturating_mul(code)),
            ));
        }
        found.into_iter().min_by_key(|&(_, bytes)| bytes).unwrap_or(plain)
    }
}

/// Appends `fields` to `out` as `plan` writes them.
pub(super) fn encode(plan: Plan, fields: &[Option<Vec<u8>>], out: &mut Vec<u8>) -> Result<()> {
    match plan {
        Plan::Fixed(width) => {
            out.extend_from_slice(&word(width)?);
            for field in fields {
                match field {
                    Some(f) => out.extend_from_slice(f),
                    None => out.resize(out.len().saturating_add(width), 0),
                }
            }
        }
        Plan::Varying => {
            let mut at = 0_usize;
            out.extend_from_slice(&word(at)?);
            for field in fields {
                at = at.checked_add(field.as_ref().map_or(0, Vec::len)).ok_or_else(too_large)?;
                out.extend_from_slice(&word(at)?);
            }
            fields.iter().flatten().for_each(|f| out.extend_from_slice(f));
        }
        Plan::Constant => {
            let one = fields.iter().flatten().next().map_or(&[][..], Vec::as_slice);
            out.extend_from_slice(&word(one.len())?);
            out.extend_from_slice(one);
        }
        Plan::Frame(least, bytes) => framed(fields, (least, bytes), out),
        Plan::Dictionary(code) => dictionary(fields, code, out)?,
    }
    Ok(())
}

/// Appends `fields`, of 8 bytes each, as frame of reference of `least`,
/// each difference in `bytes`.
fn framed(fields: &[Option<Vec<u8>>], (least, bytes): (i64, usize), out: &mut Vec<u8>) {
    out.push(u8::try_from(bytes).unwrap_or(u8::MAX));
    out.extend_from_slice(&least.to_le_bytes());
    for field in fields {
        let n = field.as_deref().and_then(integer).map_or(0, |n| i128::from(n).saturating_sub(i128::from(least)));
        let delta = u64::try_from(n).unwrap_or(0).to_le_bytes();
        out.extend_from_slice(delta.get(..bytes).unwrap_or(&delta));
    }
}

/// Appends `fields` as a dictionary of their values, each one's entry in a
/// code of `code` bytes.
fn dictionary(fields: &[Option<Vec<u8>>], code: usize, out: &mut Vec<u8>) -> Result<()> {
    let mut places: HashMap<&[u8], usize> = HashMap::new();
    let mut entries: Vec<&[u8]> = Vec::new();
    let codes: Vec<usize> = fields
        .iter()
        .map(|f| {
            f.as_deref().map_or(0, |f| {
                *places.entry(f).or_insert_with(|| {
                    entries.push(f);
                    entries.len().saturating_sub(1)
                })
            })
        })
        .collect();
    out.push(u8::try_from(code).unwrap_or(u8::MAX));
    out.extend_from_slice(&word(entries.len())?);
    let mut at = 0_usize;
    out.extend_from_slice(&word(at)?);
    for entry in &entries {
        at = at.checked_add(entry.len()).ok_or_else(too_large)?;
        out.extend_from_slice(&word(at)?);
    }
    for e in &entries {
        out.extend_from_slice(e);
    }
    for c in codes {
        let bytes = u16::try_from(c).map_err(|_| too_large())?.to_le_bytes();
        out.extend_from_slice(bytes.get(..code).unwrap_or(&bytes));
    }
    Ok(())
}

/// A chunk's values, read in place, as their encoding wrote them.
pub(super) enum Body<'a> {
    Fixed { width: usize, values: &'a [u8] },
    Varying { offsets: &'a [u8], values: &'a [u8] },
    Constant { field: &'a [u8] },
    Frame { least: i64, bytes: usize, deltas: &'a [u8] },
    Dictionary { code: usize, offsets: &'a [u8], entries: &'a [u8], codes: &'a [u8] },
}

/// `bytes` from `at` on, `len` of them.
fn slice(bytes: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    at.checked_add(len).and_then(|end| bytes.get(at..end)).ok_or_else(|| corrupt("it is too short"))
}

/// The body of a chunk of `kind`, of `count` values, that `bytes` hold
/// from `at`.
pub(super) fn body(kind: u8, count: usize, (bytes, at): (&[u8], usize)) -> Result<Body<'_>> {
    let rest = bytes.get(at..).unwrap_or_default();
    match kind {
        FIXED => {
            let width = read_word(rest, 0)?;
            Ok(Body::Fixed {
                width,
                values: slice(rest, WORD, count.checked_mul(width).ok_or_else(|| corrupt("its values are too long"))?)?,
            })
        }
        VARYING => {
            let len = count.checked_add(1).and_then(|n| n.checked_mul(WORD)).ok_or_else(|| corrupt("its count is too large"))?;
            Ok(Body::Varying { offsets: slice(rest, 0, len)?, values: rest.get(len..).unwrap_or_default() })
        }
        CONSTANT => Ok(Body::Constant { field: slice(rest, WORD, read_word(rest, 0)?)? }),
        FRAME => {
            let bytes_each = usize::from(*rest.first().ok_or_else(|| corrupt("it is too short"))?);
            let least = integer(slice(rest, 1, FRAMED)?).ok_or_else(|| corrupt("it is too short"))?;
            let deltas = slice(rest, 1 + FRAMED, count.checked_mul(bytes_each).ok_or_else(|| corrupt("its values are too long"))?)?;
            Ok(Body::Frame { least, bytes: bytes_each, deltas })
        }
        DICTIONARY => {
            let code = usize::from(*rest.first().ok_or_else(|| corrupt("it is too short"))?);
            let entries = read_word(rest, 1)?;
            let offsets_len =
                entries.checked_add(1).and_then(|n| n.checked_mul(WORD)).ok_or_else(|| corrupt("its entries are too many"))?;
            let offsets = slice(rest, 1 + WORD, offsets_len)?;
            let entries_len = read_word(offsets, entries.saturating_mul(WORD))?;
            let entries_at = (1 + WORD).saturating_add(offsets_len);
            let held = slice(rest, entries_at, entries_len)?;
            let codes = slice(
                rest,
                entries_at.saturating_add(entries_len),
                count.checked_mul(code).ok_or_else(|| corrupt("its codes are too long"))?,
            )?;
            Ok(Body::Dictionary { code, offsets, entries: held, codes })
        }
        _ => Err(corrupt("it is of another format")),
    }
}

impl Body<'_> {
    /// Its `n`th value, not null, of type `t`.
    pub(super) fn value(&self, n: usize, t: &DataType) -> Result<Value> {
        let decoded =
            |field: Option<&[u8]>| field.and_then(|f| t.decode_record(f)).ok_or_else(|| corrupt("a value is not one of its column's type"));
        match self {
            Body::Fixed { width, values } => decoded(n.checked_mul(*width).and_then(|at| values.get(at..at.checked_add(*width)?))),
            Body::Varying { offsets, values } => {
                let (from, to) =
                    (read_word(offsets, n.saturating_mul(WORD))?, read_word(offsets, n.saturating_add(1).saturating_mul(WORD))?);
                decoded(values.get(from..to))
            }
            Body::Constant { field } => decoded(Some(field)),
            Body::Frame { least, bytes, deltas } => {
                let at = n.saturating_mul(*bytes);
                let mut delta = [0_u8; FRAMED];
                let given = deltas.get(at..at.saturating_add(*bytes)).ok_or_else(|| corrupt("a value past its end is asked for"))?;
                delta.get_mut(..given.len()).ok_or_else(|| corrupt("a difference is too long"))?.copy_from_slice(given);
                let sum = i128::from(*least).saturating_add(i128::from(u64::from_le_bytes(delta)));
                let field = i64::try_from(sum).map_err(|_| corrupt("a value is out of its range"))?.to_le_bytes();
                decoded(Some(&field))
            }
            Body::Dictionary { code, offsets, entries, codes } => {
                let at = n.saturating_mul(*code);
                let mut entry = [0_u8; 2];
                let given = codes.get(at..at.saturating_add(*code)).ok_or_else(|| corrupt("a value past its end is asked for"))?;
                entry.get_mut(..given.len()).ok_or_else(|| corrupt("a code is too long"))?.copy_from_slice(given);
                let e = usize::from(u16::from_le_bytes(entry));
                let (from, to) =
                    (read_word(offsets, e.saturating_mul(WORD))?, read_word(offsets, e.saturating_add(1).saturating_mul(WORD))?);
                decoded(entries.get(from..to))
            }
        }
    }
}

impl Body<'_> {
    /// \[DESIGN\] The values at positions `from` to `to`, each an exact
    /// number's units written as 8 bytes, as integers pushed to `out`, a
    /// null's 0 (`Ints`): none if the encoding is of varying fields or a
    /// dictionary. Each frame's width read with a loop of its own, so its
    /// differences are read as fast as plain integers.
    pub(super) fn ints(&self, (from, to): (usize, usize), out: &mut Vec<i64>) -> Option<()> {
        let count = to.checked_sub(from)?;
        match self {
            Body::Frame { least, bytes, deltas } => {
                let given = deltas.get(from.checked_mul(*bytes)?..to.checked_mul(*bytes)?)?;
                unframed(*least, (*bytes, given), out)
            }
            Body::Fixed { width: FRAMED, values } => {
                let given = values.get(from.checked_mul(FRAMED)?..to.checked_mul(FRAMED)?)?;
                out.extend(given.as_chunks::<FRAMED>().0.iter().map(|&w| i64::from_le_bytes(w)));
                Some(())
            }
            Body::Constant { field } => {
                // None if every value is null, its field empty.
                out.extend(std::iter::repeat_n(integer(field).unwrap_or(0), count));
                Some(())
            }
            Body::Fixed { .. } | Body::Varying { .. } | Body::Dictionary { .. } => None,
        }
    }
}

impl Body<'_> {
    /// \[DESIGN\] The text of each value at positions `from` to `to`, a
    /// character string written as it is, pushed to `text`, and where each
    /// ends to `ends`, of those `null` says are not nulls (`Texts`): none if
    /// the encoding is of fixed fields or a frame, or a value is not UTF-8.
    pub(super) fn texts(
        &self,
        (from, to): (usize, usize),
        (text, ends): (&mut String, &mut Vec<usize>),
        null: impl Fn(usize) -> bool,
    ) -> Option<()> {
        for n in from..to {
            if !null(n) {
                text.push_str(std::str::from_utf8(self.field(n)?).ok()?);
            }
            ends.push(text.len());
        }
        Some(())
    }

    /// \[DESIGN\] Of a dictionary, its entries' text pushed to `text` and
    /// where each ends to `ends`, then of each of `positions` its entry's
    /// place among all those `ends` holds pushed to `codes`, 0 of those
    /// `null` says are nulls (`Coded`): none if the encoding is not a
    /// dictionary, or an entry is not UTF-8.
    pub(super) fn coded(
        &self,
        positions: impl Iterator<Item = usize>,
        (text, ends, codes): (&mut String, &mut Vec<usize>, &mut Vec<u32>),
        null: impl Fn(usize) -> bool,
    ) -> Option<()> {
        let Body::Dictionary { offsets, entries, .. } = self else { return None };
        let base = u32::try_from(ends.len()).ok()?;
        for e in 0..(offsets.len().checked_div(WORD)?.checked_sub(1)?) {
            let (from, to) = between(offsets, e)?;
            text.push_str(std::str::from_utf8(entries.get(from..to)?).ok()?);
            ends.push(text.len());
        }
        for n in positions {
            codes.push(if null(n) { 0 } else { base.checked_add(u32::from(self.code(n)?))? });
        }
        Some(())
    }

    /// The entry of the `n`th value of a dictionary.
    fn code(&self, n: usize) -> Option<u16> {
        let Body::Dictionary { code, codes, .. } = self else { return None };
        let at = n.checked_mul(*code)?;
        let mut entry = [0_u8; 2];
        entry.get_mut(..*code)?.copy_from_slice(codes.get(at..at.checked_add(*code)?)?);
        Some(u16::from_le_bytes(entry))
    }

    /// The field of the `n`th value of a varying, constant or dictionary
    /// encoding, as written.
    fn field(&self, n: usize) -> Option<&[u8]> {
        match self {
            Body::Varying { offsets, values } => {
                let (from, to) = between(offsets, n)?;
                values.get(from..to)
            }
            Body::Constant { field } => Some(field),
            Body::Dictionary { offsets, entries, .. } => {
                let (from, to) = between(offsets, usize::from(self.code(n)?))?;
                entries.get(from..to)
            }
            Body::Fixed { .. } | Body::Frame { .. } => None,
        }
    }
}

/// Where the `e`th of the fields `offsets` gives begins and ends.
fn between(offsets: &[u8], e: usize) -> Option<(usize, usize)> {
    read_word(offsets, e.checked_mul(WORD)?).ok().zip(read_word(offsets, e.checked_add(1)?.checked_mul(WORD)?).ok())
}

/// The values of frame of reference `least` whose differences, of `bytes`
/// each, are `given`, pushed to `out`. A difference is less than 2^64, and
/// the value it gives was an integer of 64 bits, so adding it wrapping
/// gives that value.
fn unframed(least: i64, (bytes, given): (usize, &[u8]), out: &mut Vec<i64>) -> Option<()> {
    let plus = |d: u64| least.wrapping_add(i64::from_le_bytes(d.to_le_bytes()));
    match bytes {
        1 => out.extend(given.iter().map(|&b| plus(u64::from(b)))),
        2 => out.extend(given.as_chunks::<2>().0.iter().map(|&d| plus(u64::from(u16::from_le_bytes(d))))),
        3 => out.extend(given.as_chunks::<3>().0.iter().map(|&[a, b, c]| plus(u64::from(u32::from_le_bytes([a, b, c, 0]))))),
        4 => out.extend(given.as_chunks::<4>().0.iter().map(|&d| plus(u64::from(u32::from_le_bytes(d))))),
        FRAMED => out.extend(given.as_chunks::<FRAMED>().0.iter().map(|&d| plus(u64::from_le_bytes(d)))),
        _ => {
            for d in given.chunks_exact(bytes) {
                let mut word = [0_u8; FRAMED];
                word.get_mut(..d.len())?.copy_from_slice(d);
                out.push(plus(u64::from_le_bytes(word)));
            }
        }
    }
    Some(())
}

/// The kind byte of `plan`.
pub(super) fn kind_of(plan: Plan) -> u8 {
    match plan {
        Plan::Fixed(_) => FIXED,
        Plan::Varying => VARYING,
        Plan::Constant => CONSTANT,
        Plan::Frame(..) => FRAME,
        Plan::Dictionary(_) => DICTIONARY,
    }
}
