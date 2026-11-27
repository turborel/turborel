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
//! FSST:        texts, each as codes of a table of their common substrings
//!              (`fsst.rs`)
//! DICTIONARY:  bits u8, its high bit set if the entries are in order as
//!              strings compare (`SORTED`) | entries u32 | (entries + 1) ×
//!              u32, where each entry begins and the last ends | the
//!              entries | count × bits, bit-packed, each value's entry, a
//!              null's 0
//! ```
//!
//! Integers are little-endian. Each value is as its type writes it in a
//! record, so every type is kept, and reading one encoded is as fast as
//! reading it plain, a value at a time: frame of reference needs only that
//! a field be 8 bytes, any 8 bytes being a signed integer.

mod dictionary;
mod words;

use super::alp::{Alp, Factors, Trials};
use super::fields::Cells;
use super::fsst::{Coded, Symbols, Table};
use super::packed::{bits_for, dense, get, pack, packed_len, unpack_words};
use super::runs::{Runs, runs_len};
use super::{HEADER, WORD, corrupt, nulls_len, padded, read_word, too_large, word};
use crate::error::Result;
use crate::hash::xxhash::Xxh3Map;
use crate::kernels::Kernels;
use crate::value::{DataType, Value};

pub(super) const FIXED: u8 = 1;
pub(super) const VARYING: u8 = 2;
pub(super) const CONSTANT: u8 = 3;
pub(super) const FRAME: u8 = 4;
pub(super) const DICTIONARY: u8 = 5;
pub(super) const RUNS: u8 = 6;
pub(super) const ALP: u8 = 7;
pub(super) const FSST: u8 = 8;
/// The bytes of a field frame of reference takes.
const FRAMED: usize = 8;
/// The most entries of a dictionary: as many as a code of 16 bits names.
const MOST_ENTRIES: usize = 1 << 16;
/// The bits of a code of the most entries.
const MOST_CODE_BITS: u32 = 16;
/// The bit of a dictionary's bits byte set if its entries are in order
/// (RULES.md D512).
const SORTED: u8 = 0x80;

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
    /// Approximate numbers as decimals of a pair of factors, each one's
    /// digits less the least in this many bits.
    Alp(Factors, i64, u32),
    /// Texts as codes of the sizer's table of symbols.
    Fsst,
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
    entries: Xxh3Map<&'f [u8], usize>,
    entry_bytes: usize,
    /// The exponents tried, of approximate numbers.
    alp: Option<Trials>,
    /// Of texts, the table of symbols chosen, and the bytes of their codes.
    fsst: Option<(&'f Table, usize)>,
}

fn integer(field: &[u8]) -> Option<i64> {
    <[u8; FRAMED]>::try_from(field).ok().map(i64::from_le_bytes)
}

impl<'f> Sizer<'f> {
    /// No values yet, of fields of `width` if the type has one, of
    /// approximate numbers if `floats`, of texts `table` encodes if given;
    /// encoded plain only, unless `compress`.
    pub(super) fn new(width: Option<usize>, (compress, floats): (bool, bool), table: Option<&'f Table>) -> Sizer<'f> {
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
            entries: Xxh3Map::default(),
            entry_bytes: 0,
            alp: (floats && compress && width == Some(FRAMED)).then(Trials::default),
            fsst: table.filter(|_| compress).map(|t| (t, 0)),
        }
    }

    /// The table of symbols it sized texts by, if any.
    pub(super) fn table(&self) -> Option<&'f Table> {
        self.fsst.as_ref().map(|(t, _)| *t)
    }

    /// Adds a value, none for a null.
    pub(super) fn add(&mut self, field: Option<&'f [u8]>) {
        self.count = self.count.saturating_add(1);
        let Some(f) = field else { return };
        if let Some(alp) = &mut self.alp {
            alp.add(f);
        }
        if let Some((table, bytes)) = &mut self.fsst {
            *bytes = bytes.saturating_add(table.encoded_len(f));
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
        // [DESIGN] RULES.md D1144: the least kept as each is found, the first of the least as before, no list made.
        let mut fewest = plain;
        let mut found = |plan: (Plan, usize)| {
            if plan.1 < fewest.1 {
                fewest = plan;
            }
        };
        if self.same {
            found((Plan::Constant, base.saturating_add(WORD).saturating_add(self.first.map_or(0, <[u8]>::len))));
        }
        // One value throughout is a constant, though a frame of no bits
        // takes a few bytes fewer.
        if let (true, false, Some((lo, hi))) = (self.framed, self.same, self.range) {
            let bits = bits_for(u64::try_from(i128::from(hi).saturating_sub(i128::from(lo))).unwrap_or(u64::MAX));
            let packed = packed_len(self.count, bits).unwrap_or(usize::MAX);
            found((Plan::Frame(lo, bits), base.saturating_add(1 + FRAMED).saturating_add(packed)));
            let runs = runs_len((self.count, self.runs, bits)).unwrap_or(usize::MAX);
            found((Plan::Runs(lo, bits), base.saturating_add(runs)));
        }
        if let Some(((factors, least, bits), bytes)) = self.alp.as_ref().and_then(|a| a.best(self.count)) {
            found((Plan::Alp(factors, least, bits), base.saturating_add(bytes)));
        }
        if let Some((table, codes)) = &self.fsst {
            let offsets = self.count.saturating_add(1).saturating_mul(WORD);
            found((Plan::Fsst, base.saturating_add(table.bytes()).saturating_add(offsets).saturating_add(*codes)));
        }
        if self.entries.len() < MOST_ENTRIES {
            let bits = bits_for(u64::try_from(self.entries.len().saturating_sub(1)).unwrap_or(u64::MAX));
            let table = self.entries.len().saturating_add(1).saturating_mul(WORD).saturating_add(self.entry_bytes);
            found((
                Plan::Dictionary(bits),
                base.saturating_add(1 + WORD).saturating_add(table).saturating_add(packed_len(self.count, bits).unwrap_or(usize::MAX)),
            ));
        }
        fewest
    }
}

/// Appends `fields` to `out` as `plan` writes them, of texts by `table`.
/// Of fixed fields with no null, and of varying ones, their records are
/// copied as the column holds them, end to end (RULES.md D1148).
pub(super) fn encode(plan: Plan, fields: Cells<'_>, (table, sorted): (Option<&Table>, bool), out: &mut Vec<u8>) -> Result<()> {
    match plan {
        Plan::Fixed(width) => {
            out.extend_from_slice(&word(width)?);
            if fields.no_nulls() {
                out.extend_from_slice(fields.span());
                return Ok(());
            }
            for field in fields.iter() {
                match field {
                    Some(f) => out.extend_from_slice(f),
                    None => out.resize(out.len().saturating_add(width), 0),
                }
            }
        }
        Plan::Varying => {
            let mut at = 0_usize;
            out.extend_from_slice(&word(at)?);
            for field in fields.iter() {
                at = at.checked_add(field.map_or(0, <[u8]>::len)).ok_or_else(too_large)?;
                out.extend_from_slice(&word(at)?);
            }
            out.extend_from_slice(fields.span());
        }
        Plan::Constant => {
            let one = fields.iter().flatten().next().unwrap_or_default();
            out.extend_from_slice(&word(one.len())?);
            out.extend_from_slice(one);
        }
        Plan::Frame(least, bits) => framed(fields, (least, bits), out),
        Plan::Dictionary(bits) => dictionary::write(fields, (bits, sorted), out)?,
        Plan::Runs(least, bits) => super::runs::encode(fields, (least, bits), out)?,
        Plan::Alp(factors, least, bits) => super::alp::encode(fields, (factors, least, bits), out)?,
        Plan::Fsst => super::fsst::encode(fields, table.ok_or_else(|| corrupt("texts have no table"))?, out)?,
    }
    Ok(())
}

/// Appends `fields`, of 8 bytes each, as frame of reference of `least`,
/// each difference in `bits`, bit-packed.
fn framed(fields: Cells<'_>, (least, bits): (i64, u32), out: &mut Vec<u8>) {
    out.push(u8::try_from(bits).unwrap_or(u8::MAX));
    out.extend_from_slice(&least.to_le_bytes());
    let deltas = fields.iter().map(|field| {
        let n = field.and_then(integer).map_or(0, |n| i128::from(n).saturating_sub(i128::from(least)));
        u64::try_from(n).unwrap_or(0)
    });
    pack(deltas, bits, out);
}

/// Of a dictionary of texts, its entries as text, one after another, and
/// where each ends.
#[derive(Debug, Clone, Default)]
pub(super) struct Entries {
    text: String,
    ends: Vec<usize>,
}

/// A chunk's values, read in place, as their encoding wrote them.
pub(super) enum Body<'a> {
    Fixed { width: usize, values: &'a [u8] },
    Varying { offsets: &'a [u8], values: &'a [u8] },
    Constant { field: &'a [u8] },
    Frame { least: i64, bits: u32, deltas: &'a [u8] },
    Dictionary { bits: u32, sorted: bool, offsets: &'a [u8], entries: &'a [u8], codes: &'a [u8] },
    Runs(Runs<'a>),
    Alp(Alp<'a>),
    Fsst(Coded<'a>),
}

/// `bytes` from `at` on, `len` of them.
fn slice(bytes: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    at.checked_add(len).and_then(|end| bytes.get(at..end)).ok_or_else(|| corrupt("it is too short"))
}

/// The body of a chunk of `kind`, of `count` values, that `bytes` hold
/// from `at`, of texts coded by `table` if given, with the bytes it takes,
/// read of the same bytes before (`prepared.rs`).
pub(super) fn body<'a>(kind: u8, count: usize, (bytes, at): (&'a [u8], usize), table: Option<(&'a Symbols, usize)>) -> Result<Body<'a>> {
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
            let byte = *rest.first().ok_or_else(|| corrupt("it is too short"))?;
            let (bits, sorted) = (u32::from(byte & !SORTED), byte & SORTED != 0);
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
            Ok(Body::Dictionary { bits, sorted, offsets, entries: held, codes })
        }
        RUNS => Ok(Body::Runs(Runs::parse(rest, count)?)),
        ALP => Ok(Body::Alp(Alp::parse(rest, count)?)),
        FSST => Ok(Body::Fsst(Coded::parse(rest, count, table)?)),
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
            Body::Fsst(coded) => decoded(Some(&coded.text(n).ok_or_else(|| corrupt("a text's codes are malformed"))?)),
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
    /// \[DESIGN\] (RULES.md D1171) Of runs, or one value, the values at
    /// positions `from` to `to` pushed to `out`, as `ints` gives them, and
    /// where each run among them ends, past `base`, pushed to `ends`, both
    /// of one reading of its runs (`Runs::ints_ends`): whether it keeps
    /// its values so, none if they are not there. Of another encoding,
    /// nothing is pushed.
    pub(super) fn ints_ending(
        &self,
        (from, to): (usize, usize),
        out: &mut Vec<i64>,
        (ends, base): (&mut Vec<usize>, usize),
    ) -> Option<bool> {
        match self {
            Body::Runs(runs) => runs.ints_ends((from, to), out, Some((ends, base))).map(|()| true),
            Body::Constant { field } => {
                out.extend(std::iter::repeat_n(words::constant(field)?, to.checked_sub(from)?));
                if to > from {
                    ends.push(base.checked_add(to.checked_sub(from)?)?);
                }
                Some(true)
            }
            Body::Frame { .. } | Body::Alp(_) | Body::Dictionary { .. } | Body::Fixed { .. } | Body::Varying { .. } | Body::Fsst(_) => {
                Some(false)
            }
        }
    }

    /// \[DESIGN\] The text of each value at positions `from` to `to`, a
    /// character string written as it is, pushed to `text`, and where each
    /// ends to `ends`, of those `null` says are not nulls (`Texts`), each
    /// padded with spaces to `pad` characters if given, as a CHARACTER
    /// string is read: none if the encoding is of fixed fields, a frame,
    /// runs or decimals, or a value is not UTF-8, as `kernels` judges it.
    pub(super) fn texts(
        &self,
        (from, to): (usize, usize),
        (text, ends): (&mut String, &mut Vec<usize>),
        (null, pad, kernels): (impl Fn(usize) -> bool, Option<usize>, &dyn Kernels),
    ) -> Option<()> {
        if let Body::Fsst(coded) = self {
            // Of VARCHAR texts alone (`columns.rs`), never padded.
            return pad.is_none().then(|| coded.texts((from, to), (text, ends), (null, kernels))).flatten();
        }
        if let Body::Varying { offsets, values } = self {
            return varying_texts((offsets, values), (from, to), (text, ends), (null, pad, kernels));
        }
        for n in from..to {
            if !null(n) {
                padded(text, kernels.utf8(self.field(n)?)?, pad);
            }
            ends.push(text.len());
        }
        Some(())
    }

    /// \[DESIGN\] Of a dictionary, its entries' text, `entries`, pushed to
    /// `text` and where each ends to `ends`, then of each of `positions`
    /// its entry's place among all those `ends` holds pushed to `codes`, 0
    /// of those `null` says are nulls (`Coded`): none if the encoding is
    /// not a dictionary. The codes from the first of `positions`, in
    /// order, to the last are unpacked at once, unless they are too few of
    /// that span (`packed::dense`), each then read alone.
    pub(super) fn coded(
        &self,
        positions: &[usize],
        (text, ends, codes): (&mut String, &mut Vec<usize>, &mut Vec<u32>),
        (null, entries): (impl Fn(usize) -> bool, &Entries),
    ) -> Option<()> {
        let Body::Dictionary { bits, codes: packed, .. } = self else { return None };
        let base = u32::try_from(ends.len()).ok()?;
        let at = text.len();
        text.push_str(&entries.text);
        ends.extend(entries.ends.iter().map(|&end| end.saturating_add(at)));
        let (Some(&first), Some(&last)) = (positions.first(), positions.last()) else { return Some(()) };
        let span = last.checked_sub(first)?.saturating_add(1);
        // Positions few of their span each read alone (RULES.md D360).
        if !dense(positions.len(), span) {
            for &n in positions {
                let code = u32::try_from(get(packed, *bits, n)?).ok()?;
                codes.push(if null(n) { 0 } else { base.checked_add(code)? });
            }
            return Some(());
        }
        let mut held = Vec::with_capacity(span);
        unpack_words(packed, (*bits, 0), (first, last.checked_add(1)?), &mut held)?;
        for &n in positions {
            let code = u32::try_from(*held.get(n.checked_sub(first)?)?).ok()?;
            codes.push(if null(n) { 0 } else { base.checked_add(code)? });
        }
        Some(())
    }

    /// \[DESIGN\] Of a dictionary, its entries' text, each padded to `pad`
    /// characters if given, as `texts` pads: none if the encoding is not a
    /// dictionary, or an entry is not UTF-8. The octets from the first
    /// entry's start to the last's end are judged UTF-8 once by `kernels`,
    /// each entry then a slice of them, as `varying_texts` judges fields
    /// (RULES.md D1131, D1175).
    pub(super) fn entry_texts(&self, pad: Option<usize>, kernels: &dyn Kernels) -> Option<Entries> {
        let Body::Dictionary { offsets, entries, .. } = self else { return None };
        let count = offsets.len().checked_div(WORD)?.checked_sub(1)?;
        let mut held = Entries { text: String::new(), ends: Vec::with_capacity(count) };
        let Some(last) = count.checked_sub(1) else { return Some(held) };
        let (start, end) = (between(offsets, 0)?.0, between(offsets, last)?.1);
        let span = kernels.utf8(entries.get(start..end)?)?;
        // Room for every entry's octets at once, all an unpadded text takes.
        held.text.reserve(span.len());
        for e in 0..count {
            let (from, to) = between(offsets, e)?;
            padded(&mut held.text, span.get(from.checked_sub(start)?..to.checked_sub(start)?)?, pad);
            held.ends.push(held.text.len());
        }
        Some(held)
    }

    /// The entry of the `n`th value of a dictionary.
    fn code(&self, n: usize) -> Option<u16> {
        let Body::Dictionary { bits, codes, .. } = self else { return None };
        u16::try_from(get(codes, *bits, n)?).ok()
    }

    /// \[DESIGN\] (RULES.md D354) The field of the `n`th value as written,
    /// of any encoding that keeps its fields whole: fixed, varying,
    /// constant or dictionary, as a truth value's byte is kept.
    pub(super) fn whole_field(&self, n: usize) -> Option<&[u8]> {
        if let Body::Fixed { width, values } = self {
            let at = n.checked_mul(*width)?;
            return values.get(at..at.checked_add(*width)?);
        }
        self.field(n)
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
            Body::Fixed { .. } | Body::Frame { .. } | Body::Runs(_) | Body::Alp(_) | Body::Fsst(_) => None,
        }
    }
}

/// \[DESIGN\] (RULES.md D1131) As `Body::texts`, of varying fields of
/// `values` that `offsets` place: the octets from the first's start to
/// the last's end judged UTF-8 once by `kernels` (D1175), each field then
/// a slice of them, none if they are not UTF-8 or a field does not begin
/// and end on a character. Fields that are each UTF-8 are UTF-8 together, so this
/// gives what judging each would.
fn varying_texts(
    (offsets, values): (&[u8], &[u8]),
    (from, to): (usize, usize),
    (text, ends): (&mut String, &mut Vec<usize>),
    (null, pad, kernels): (impl Fn(usize) -> bool, Option<usize>, &dyn Kernels),
) -> Option<()> {
    let Some(last) = to.checked_sub(1).filter(|&last| last >= from) else { return Some(()) };
    let (start, end) = (between(offsets, from)?.0, between(offsets, last)?.1);
    let span = kernels.utf8(values.get(start..end)?)?;
    for n in from..to {
        if !null(n) {
            let (a, b) = between(offsets, n)?;
            padded(text, span.get(a.checked_sub(start)?..b.checked_sub(start)?)?, pad);
        }
        ends.push(text.len());
    }
    Some(())
}

/// Where the `e`th of the fields `offsets` gives begins and ends.
fn between(offsets: &[u8], e: usize) -> Option<(usize, usize)> {
    read_word(offsets, e.checked_mul(WORD)?).ok().zip(read_word(offsets, e.checked_add(1)?.checked_mul(WORD)?).ok())
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
        Plan::Fsst => FSST,
    }
}

/// \[DESIGN\] The name of the encoding of `kind`, a chunk's first byte, as
/// INFORMATION_SCHEMA.COLUMN_STORAGE shows it (RULES.md D1165); none of a
/// kind no encoding has.
pub(crate) fn kind_name(kind: u8) -> Option<&'static str> {
    let names = [(FIXED, "FIXED"), (VARYING, "VARYING"), (CONSTANT, "CONSTANT"), (FRAME, "FRAME")];
    let more = [(DICTIONARY, "DICTIONARY"), (RUNS, "RUNS"), (ALP, "ALP"), (FSST, "FSST")];
    names.into_iter().chain(more).find(|&(k, _)| k == kind).map(|(_, name)| name)
}
