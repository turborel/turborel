//! \[DESIGN\] How a column chunk's values are written (RULES.md D31): of the
//! encodings that can hold them, the one that takes fewest bytes, so a page
//! holds more of them. After the chunk's kind, count and nulls:
//!
//! ```text
//! FIXED:       width u32 | count × width bytes, a null's zeroed
//! VARYING:     (count + 1) × u32, where each value begins and the last ends
//!              | the bytes
//! CONSTANT:    width u32 | the one value not null, if any
//! FRAME:       bits u8 | least i64 | count × bits, bit-packed, each value of
//!              8 bytes as a signed integer less the least, a null's zeroed
//!              (`packed.rs`)
//! RUNS:        runs of one value, of 8-byte fields as a frame's
//!              (`runs.rs`)
//! ALP:         approximate numbers as decimals, a frame of reference of
//!              each one's digits and exceptions (`alp.rs`)
//! DICTIONARY:  bits u8 | entries u32 | (entries + 1) × u32, where each
//!              entry begins and the last ends | the entries | count × bits,
//!              bit-packed, each value's entry, a null's 0
//! ```
//!
//! Integers are little-endian. Each value is as its type writes it in a
//! record, so every type is kept, and reading one encoded is as fast as
//! reading it plain, a value at a time: frame of reference needs only that
//! a field be 8 bytes, any 8 bytes being a signed integer. Split from
//! columns.rs (RULES.md R-SIZE-2).

use std::collections::HashMap;

use super::alp::{Alp, Trials};
use super::packed::{bits_for, get, pack, packed_len, unpack};
use super::runs::{Runs, runs_len};
use super::{HEADER, WORD, corrupt, nulls_len, read_word, too_large, word};
use crate::error::Result;
use crate::value::{DataType, Value};

pub(super) const FIXED: u8 = 1;
pub(super) const VARYING: u8 = 2;
pub(super) const CONSTANT: u8 = 3;
pub(super) const FRAME: u8 = 4;
pub(super) const DICTIONARY: u8 = 5;
pub(super) const RUNS: u8 = 6;
pub(super) const ALP: u8 = 7;
/// The bytes of a field frame of reference takes.
const FRAMED: usize = 8;
/// The most entries of a dictionary: as many as a code of 16 bits names.
const MOST_ENTRIES: usize = 1 << 16;
/// The bits of a code of the most entries.
const MOST_CODE_BITS: u32 = 16;

/// How a chunk's values are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Plan {
    Fixed(usize),
    Varying,
    Constant,
    /// Frame of reference, each value less the least in this many bits.
    Frame(i64, u32),
    /// A dictionary, each value's entry in a code of this many bits.
    Dictionary(u32),
    /// Runs of one value, each less the least in this many bits.
    Runs(i64, u32),
    /// Approximate numbers as decimals of an exponent, each one's digits
    /// less the least in this many bits.
    Alp(u32, i64, u32),
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
    /// The runs of values alike among them, and the last value.
    runs: usize,
    last: Option<i64>,
    framed: bool,
    entries: HashMap<&'f [u8], usize>,
    entry_bytes: usize,
    /// The exponents tried, of approximate numbers.
    alp: Option<Trials>,
}

fn integer(field: &[u8]) -> Option<i64> {
    <[u8; FRAMED]>::try_from(field).ok().map(i64::from_le_bytes)
}

impl<'f> Sizer<'f> {
    /// No values yet, of fields of `width` if the type has one, of
    /// approximate numbers if `floats`; encoded plain only, unless
    /// `compress`.
    pub(super) fn new(width: Option<usize>, (compress, floats): (bool, bool)) -> Sizer<'f> {
        Sizer {
            count: 0,
            compress,
            width,
            varying: 0,
            first: None,
            same: true,
            range: None,
            runs: 0,
            last: None,
            framed: width == Some(FRAMED),
            entries: HashMap::new(),
            entry_bytes: 0,
            alp: (floats && compress && width == Some(FRAMED)).then(Trials::default),
        }
    }

    /// Adds a value, none for a null.
    pub(super) fn add(&mut self, field: Option<&'f [u8]>) {
        self.count = self.count.saturating_add(1);
        let Some(f) = field else { return };
        if let Some(alp) = &mut self.alp {
            alp.add(f);
        }
        self.varying = self.varying.saturating_add(f.len());
        if self.width.is_some_and(|w| w != f.len()) {
            self.width = None;
        }
        match self.first {
            None => self.first = Some(f),
            Some(first) => self.same = self.same && first == f,
        }
        match (integer(f), self.framed) {
            (Some(n), true) => {
                self.range = Some(self.range.map_or((n, n), |(lo, hi)| (lo.min(n), hi.max(n))));
                if self.last != Some(n) {
                    self.runs = self.runs.saturating_add(1);
                    self.last = Some(n);
                }
            }
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
        // One value throughout is a constant, though a frame of no bits
        // takes a few bytes fewer.
        if let (true, false, Some((lo, hi))) = (self.framed, self.same, self.range) {
            let bits = bits_for(u64::try_from(i128::from(hi).saturating_sub(i128::from(lo))).unwrap_or(u64::MAX));
            let packed = packed_len(self.count, bits).unwrap_or(usize::MAX);
            found.push((Plan::Frame(lo, bits), base.saturating_add(1 + FRAMED).saturating_add(packed)));
            let runs = runs_len((self.count, self.runs, bits)).unwrap_or(usize::MAX);
            found.push((Plan::Runs(lo, bits), base.saturating_add(runs)));
        }
        if let Some(((e, least, bits), bytes)) = self.alp.as_ref().and_then(|a| a.best(self.count)) {
            found.push((Plan::Alp(e, least, bits), base.saturating_add(bytes)));
        }
        if self.entries.len() < MOST_ENTRIES {
            let bits = bits_for(u64::try_from(self.entries.len().saturating_sub(1)).unwrap_or(u64::MAX));
            let table = self.entries.len().saturating_add(1).saturating_mul(WORD).saturating_add(self.entry_bytes);
            found.push((
                Plan::Dictionary(bits),
                base.saturating_add(1 + WORD).saturating_add(table).saturating_add(packed_len(self.count, bits).unwrap_or(usize::MAX)),
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
        Plan::Frame(least, bits) => framed(fields, (least, bits), out),
        Plan::Dictionary(bits) => dictionary(fields, bits, out)?,
        Plan::Runs(least, bits) => super::runs::encode(fields, (least, bits), out)?,
        Plan::Alp(e, least, bits) => super::alp::encode(fields, (e, least, bits), out)?,
    }
    Ok(())
}

/// Appends `fields`, of 8 bytes each, as frame of reference of `least`,
/// each difference in `bits`, bit-packed.
fn framed(fields: &[Option<Vec<u8>>], (least, bits): (i64, u32), out: &mut Vec<u8>) {
    out.push(u8::try_from(bits).unwrap_or(u8::MAX));
    out.extend_from_slice(&least.to_le_bytes());
    let deltas = fields.iter().map(|field| {
        let n = field.as_deref().and_then(integer).map_or(0, |n| i128::from(n).saturating_sub(i128::from(least)));
        u64::try_from(n).unwrap_or(0)
    });
    pack(deltas, bits, out);
}

/// Appends `fields` as a dictionary of their values, each one's entry in a
/// code of `bits`, bit-packed.
fn dictionary(fields: &[Option<Vec<u8>>], bits: u32, out: &mut Vec<u8>) -> Result<()> {
    let mut places: HashMap<&[u8], usize> = HashMap::new();
    let mut entries: Vec<&[u8]> = Vec::new();
    let codes: Vec<u64> = fields
        .iter()
        .map(|f| {
            f.as_deref().map_or(0, |f| {
                *places.entry(f).or_insert_with(|| {
                    entries.push(f);
                    entries.len().saturating_sub(1)
                })
            })
        })
        .map(|c| u64::try_from(c).unwrap_or(u64::MAX))
        .collect();
    if entries.len() > MOST_ENTRIES {
        return Err(too_large());
    }
    out.push(u8::try_from(bits).unwrap_or(u8::MAX));
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
    pack(codes.into_iter(), bits, out);
    Ok(())
}

/// A chunk's values, read in place, as their encoding wrote them.
pub(super) enum Body<'a> {
    Fixed { width: usize, values: &'a [u8] },
    Varying { offsets: &'a [u8], values: &'a [u8] },
    Constant { field: &'a [u8] },
    Frame { least: i64, bits: u32, deltas: &'a [u8] },
    Dictionary { bits: u32, offsets: &'a [u8], entries: &'a [u8], codes: &'a [u8] },
    Runs(Runs<'a>),
    Alp(Alp<'a>),
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
            let bits = u32::from(*rest.first().ok_or_else(|| corrupt("it is too short"))?);
            if bits > u64::BITS {
                return Err(corrupt("its differences are wider than 64 bits"));
            }
            let least = integer(slice(rest, 1, FRAMED)?).ok_or_else(|| corrupt("it is too short"))?;
            let deltas = slice(rest, 1 + FRAMED, packed_len(count, bits).ok_or_else(|| corrupt("its values are too long"))?)?;
            Ok(Body::Frame { least, bits, deltas })
        }
        DICTIONARY => {
            let bits = u32::from(*rest.first().ok_or_else(|| corrupt("it is too short"))?);
            if bits > MOST_CODE_BITS {
                return Err(corrupt("its codes are wider than 16 bits"));
            }
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
                packed_len(count, bits).ok_or_else(|| corrupt("its codes are too long"))?,
            )?;
            Ok(Body::Dictionary { bits, offsets, entries: held, codes })
        }
        RUNS => Ok(Body::Runs(Runs::parse(rest, count)?)),
        ALP => Ok(Body::Alp(Alp::parse(rest, count)?)),
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
            Body::Runs(runs) => decoded(runs.field(n).as_ref().map(<[u8; FRAMED]>::as_slice)),
            Body::Alp(alp) => decoded(alp.field(n).as_ref().map(<[u8; FRAMED]>::as_slice)),
            Body::Frame { least, bits, deltas } => {
                let delta = get(deltas, *bits, n).ok_or_else(|| corrupt("a value past its end is asked for"))?;
                let sum = i128::from(*least).saturating_add(i128::from(delta));
                let field = i64::try_from(sum).map_err(|_| corrupt("a value is out of its range"))?.to_le_bytes();
                decoded(Some(&field))
            }
            Body::Dictionary { offsets, entries, .. } => {
                let e = usize::from(self.code(n).ok_or_else(|| corrupt("a value past its end is asked for"))?);
                let (from, to) =
                    (read_word(offsets, e.saturating_mul(WORD))?, read_word(offsets, e.saturating_add(1).saturating_mul(WORD))?);
                decoded(entries.get(from..to))
            }
        }
    }
}

impl Body<'_> {
    /// \[DESIGN\] The values at positions `from` to `to`, each 8 bytes, an
    /// exact number's units or an approximate number's bits, as integers
    /// pushed to `out`, a null's 0 (`Ints`, `Floats`): none if the
    /// encoding is of varying fields or a dictionary of them. Each frame's width read with a loop of its own, so its
    /// differences are read as fast as plain integers.
    pub(super) fn ints(&self, (from, to): (usize, usize), out: &mut Vec<i64>) -> Option<()> {
        let count = to.checked_sub(from)?;
        match self {
            Body::Frame { least, bits, deltas } => unframed(*least, (*bits, deltas), (from, to), out),
            Body::Runs(runs) => runs.ints((from, to), out),
            Body::Alp(alp) => alp.words((from, to), out),
            Body::Dictionary { bits, offsets, entries, codes } => dictionary_words((*bits, offsets, entries, codes), (from, to), out),
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
            Body::Fixed { .. } | Body::Varying { .. } => None,
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
    /// dictionary, or an entry is not UTF-8. The codes from the first of
    /// `positions`, in order, to the last are unpacked at once.
    pub(super) fn coded(
        &self,
        positions: &[usize],
        (text, ends, codes): (&mut String, &mut Vec<usize>, &mut Vec<u32>),
        null: impl Fn(usize) -> bool,
    ) -> Option<()> {
        let Body::Dictionary { bits, offsets, entries, codes: packed } = self else { return None };
        let base = u32::try_from(ends.len()).ok()?;
        for e in 0..(offsets.len().checked_div(WORD)?.checked_sub(1)?) {
            let (from, to) = between(offsets, e)?;
            text.push_str(std::str::from_utf8(entries.get(from..to)?).ok()?);
            ends.push(text.len());
        }
        let (Some(&first), Some(&last)) = (positions.first(), positions.last()) else { return Some(()) };
        let mut held = Vec::with_capacity(last.checked_sub(first)?.saturating_add(1));
        unpack(packed, *bits, (first, last.checked_add(1)?), |c| held.push(c))?;
        for &n in positions {
            let code = u32::try_from(*held.get(n.checked_sub(first)?)?).ok()?;
            codes.push(if null(n) { 0 } else { base.checked_add(code)? });
        }
        Some(())
    }

    /// The entry of the `n`th value of a dictionary.
    fn code(&self, n: usize) -> Option<u16> {
        let Body::Dictionary { bits, codes, .. } = self else { return None };
        u16::try_from(get(codes, *bits, n)?).ok()
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
            Body::Fixed { .. } | Body::Frame { .. } | Body::Runs(_) | Body::Alp(_) => None,
        }
    }
}

/// The values at positions `from` to `to` of a dictionary whose codes of
/// `bits` are `codes`, each its entry's 8 bytes as an integer, pushed to
/// `out`: the entries read once, then the codes unpacked a run at a time;
/// none if an entry is not of 8 bytes.
fn dictionary_words(
    (bits, offsets, entries, codes): (u32, &[u8], &[u8], &[u8]),
    (from, to): (usize, usize),
    out: &mut Vec<i64>,
) -> Option<()> {
    let count = offsets.len().checked_div(WORD)?.checked_sub(1)?;
    let words: Vec<i64> =
        (0..count).map(|e| between(offsets, e).and_then(|(a, b)| entries.get(a..b)).and_then(integer)).collect::<Option<_>>()?;
    out.reserve(to.checked_sub(from)?);
    let mut ok = true;
    unpack(codes, bits, (from, to), |c| {
        let word = usize::try_from(c).ok().and_then(|c| words.get(c)).copied();
        ok = ok && word.is_some();
        out.push(word.unwrap_or(0));
    })?;
    ok.then_some(())
}

/// Where the `e`th of the fields `offsets` gives begins and ends.
fn between(offsets: &[u8], e: usize) -> Option<(usize, usize)> {
    read_word(offsets, e.checked_mul(WORD)?).ok().zip(read_word(offsets, e.checked_add(1)?.checked_mul(WORD)?).ok())
}

/// The values of frame of reference `least` whose differences, of `bytes`
/// each, are `given`, pushed to `out`. A difference is less than 2^64, and
/// the value it gives was an integer of 64 bits, so adding it wrapping
/// gives that value.
fn unframed(least: i64, (bits, deltas): (u32, &[u8]), (from, to): (usize, usize), out: &mut Vec<i64>) -> Option<()> {
    let plus = |d: u64| least.wrapping_add(i64::from_le_bytes(d.to_le_bytes()));
    // Of whole bytes, read a byte width at a time; of other widths, a
    // value's window at a time.
    if bits.checked_rem(8)? != 0 {
        out.reserve(to.saturating_sub(from));
        return unpack(deltas, bits, (from, to), |d| out.push(plus(d)));
    }
    let bytes = usize::try_from(bits.checked_div(8)?).ok()?;
    if bytes == 0 {
        out.extend(std::iter::repeat_n(least, to.checked_sub(from)?));
        return Some(());
    }
    let given = deltas.get(from.checked_mul(bytes)?..to.checked_mul(bytes)?)?;
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
        Plan::Runs(..) => RUNS,
        Plan::Alp(..) => ALP,
    }
}
