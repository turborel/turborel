//! \[DESIGN\] (RULES.md D1252, D1316) The texts of a run of a column's
//! rows, gathered of its chunks into one vector of octets.
//!
//! A scan reads a run of at most a vector of rows, and the chunks of a
//! column of long texts hold a hundred or two rows each, so a run reads a
//! dozen chunks or more, of whatever encodings their values took. Each
//! chunk adds its octets to the one vector, room made for them once, and
//! where each text ends. Nothing is judged as it is added. The vector is
//! made the run's text by one judgement of all of it
//! (`Kernels::text`), and each end must then lie on a character's
//! boundary, so each text is UTF-8 alone. Octets that are no text give no
//! column, and the rows are then read as values, which raises what is
//! malformed.
//!
//! Rows are entries, one each, until a chunk gives entries its rows
//! share. A dictionary gives its entries and a code a row, and a chunk of
//! one value gives one entry. From there every row has a code. The rows
//! gathered before are given the codes of the entries they are, and the
//! rows of a later chunk that shares nothing are each an entry and its
//! code. So a run across chunks of several encodings is still read an
//! entry at a time by what reads codes (`storage::Coded`), and a run with
//! no chunk that shares entries is texts and their ends as before.
//!
//! A dictionary asked for no fewer rows than it has entries gives its
//! entries whole, by one copy where its type pads nothing. One asked for
//! fewer gives only the entries those rows read, each once, in the order
//! the rows meet them. Either way a run has no more entries than rows.

use std::sync::Arc;

use super::encoded::{Body, between};
use super::packed::{dense, get, unpack_words};
use super::{BITS, Chunk, WORD, bit_of, read_word};
use crate::kernels::Kernels;
use crate::storage::{Coded, Mask, Texts};

/// The octet a CHARACTER string is padded with as it is read, a space.
const PAD: u8 = b' ';
/// The bits of an octet that tell whether it goes on with a character of
/// UTF-8, and what they are of one that does (RFC 3629 §3).
const CONTINUES: (u8, u8) = (0xC0, 0x80);

/// No entry given of a code yet: no run has as many entries.
const NONE: u32 = u32::MAX;

/// The rows asked of a chunk, by their positions in it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Asked<'p> {
    /// Every row from the first position to before the second.
    Run(usize, usize),
    /// The rows at these positions, ascending.
    At(&'p [usize]),
}

impl Asked<'_> {
    /// How many rows.
    fn count(&self) -> usize {
        match self {
            Asked::Run(from, to) => to.saturating_sub(*from),
            Asked::At(places) => places.len(),
        }
    }

    /// The first position asked and the one past the last, none of no
    /// rows.
    fn span(&self) -> Option<(usize, usize)> {
        match self {
            Asked::Run(from, to) => (from < to).then_some((*from, *to)),
            Asked::At(places) => Some((*places.first()?, places.last()?.checked_add(1)?)),
        }
    }

    /// Whether every position asked is before `count`.
    fn within(&self, count: usize) -> bool {
        match self {
            Asked::Run(_, to) => *to <= count,
            Asked::At(places) => places.iter().all(|&n| n < count),
        }
    }

    /// Gives each position asked, in order, to `row`, until it gives none.
    fn each(&self, row: impl FnMut(usize) -> Option<()>) -> Option<()> {
        match self {
            Asked::Run(from, to) => (*from..*to).try_for_each(row),
            Asked::At(places) => places.iter().copied().try_for_each(row),
        }
    }
}

/// \[DESIGN\] (RULES.md D1252, D1316) The texts of a run of rows as its
/// chunks add them, one after another.
#[derive(Debug, Default)]
pub(super) struct Gathered {
    /// Every entry's octets, one after another.
    octets: Vec<u8>,
    /// Where each entry ends in `octets`.
    ends: Vec<usize>,
    /// Each row's entry, once a chunk gave entries its rows share. None
    /// while every row is the entry of its own place.
    codes: Option<Vec<u32>>,
    /// Whether each row is null.
    nulls: Mask,
    /// How many rows the run has, to make room for their codes once.
    rows: usize,
    /// A dictionary's codes of the rows asked of it, and the position
    /// its first is of, if they are those of a whole span of positions.
    unpacked: (Vec<i64>, Option<usize>),
    /// By a dictionary's code, the entry it was given of the chunk being
    /// read by the entries its rows use. An entry given of a chunk is
    /// one after those held before the chunk, so what a chunk before
    /// left here, or nothing, is told by its place, and nothing is
    /// cleared from chunk to chunk.
    given: Vec<u32>,
}

/// Adds `field` to `octets`, then spaces up to `pad` characters if given,
/// as a CHARACTER string, written without its trailing spaces, is read
/// [X3.135-1986 §8.7 GR7b] (`record.rs`). Its characters are its octets
/// that go on with none, which holds of what is judged UTF-8 afterwards.
fn put(octets: &mut Vec<u8>, field: &[u8], pad: Option<usize>) {
    octets.extend_from_slice(field);
    if let Some(length) = pad {
        let count = field.iter().filter(|&&octet| octet & CONTINUES.0 != CONTINUES.1).count();
        octets.resize(octets.len().saturating_add(length.saturating_sub(count)), PAD);
    }
}

impl Gathered {
    /// No texts yet, of a run of `rows` rows, with room for `octets`
    /// octets, as many as the chunks it reads take stored.
    pub(super) fn with_room(rows: usize, octets: usize) -> Gathered {
        Gathered { octets: Vec::with_capacity(octets), ends: Vec::with_capacity(rows), rows, ..Gathered::default() }
    }

    /// Ends an entry where the octets held end, and gives its place.
    fn entry(&mut self) -> Option<u32> {
        let place = u32::try_from(self.ends.len()).ok()?;
        self.ends.push(self.octets.len());
        Some(place)
    }

    /// Makes the octets just added the text of a row, an entry of its
    /// own.
    fn own(&mut self) -> Option<()> {
        let place = self.entry()?;
        if let Some(codes) = &mut self.codes {
            codes.push(place);
        }
        Some(())
    }

    /// Adds a null row: an empty entry while rows are entries, as a
    /// null's text is empty, and entry 0 once rows have codes.
    fn null(&mut self) -> Option<()> {
        match &mut self.codes {
            Some(codes) => codes.push(0),
            None => {
                self.entry()?;
            }
        }
        Some(())
    }

    /// \[DESIGN\] (RULES.md D1316) The rows' codes, to add to. The first
    /// time, every row gathered so far is given the code of the entry it
    /// is.
    fn coded(&mut self) -> Option<&mut Vec<u32>> {
        if self.codes.is_none() {
            let so_far = u32::try_from(self.ends.len()).ok()?;
            let mut codes = Vec::with_capacity(self.rows);
            codes.extend(0..so_far);
            self.codes = Some(codes);
        }
        self.codes.as_mut()
    }

    /// \[DESIGN\] (RULES.md D1131) Adds the fields `from` to `to` of
    /// `bytes` that `offsets` place as entries, the octets from the
    /// first's start to the last's end by one copy, as each field begins
    /// where the one before it ends. None if a field ends before it
    /// begins, as none written does, or past `bytes`.
    fn fields(&mut self, (offsets, bytes): (&[u8], &[u8]), (from, to): (usize, usize)) -> Option<()> {
        if from >= to {
            return Some(());
        }
        let (start, at) = (read_word(offsets, from.checked_mul(WORD)?).ok()?, self.octets.len());
        let mut last = start;
        for n in from..to {
            // A field begins where the one before it ends, so each end
            // is read once.
            let ends = read_word(offsets, n.checked_add(1)?.checked_mul(WORD)?).ok()?;
            if ends < last {
                return None;
            }
            last = ends;
            self.ends.push(at.checked_add(ends.checked_sub(start)?)?);
        }
        self.octets.extend_from_slice(bytes.get(start..last)?);
        Some(())
    }

    /// \[DESIGN\] (RULES.md D360) Unpacks the codes of the rows `asked`
    /// of a dictionary whose codes are `packed`, each of `bits`: those of
    /// the span from the first position asked to the last at once, unless
    /// positions are few of their span (`packed::dense`), each then read
    /// alone.
    fn unpack(&mut self, (packed, bits): (&[u8], u32), asked: Asked<'_>) -> Option<()> {
        let (codes, first) = &mut self.unpacked;
        codes.clear();
        let (low, past) = asked.span()?;
        if let Asked::At(places) = asked
            && !dense(places.len(), past.saturating_sub(low))
        {
            *first = None;
            for &n in places {
                codes.push(i64::try_from(get(packed, bits, n)?).ok()?);
            }
            return Some(());
        }
        *first = Some(low);
        unpack_words(packed, (bits, 0), (low, past), codes)
    }

    /// The code unpacked of the row at position `n`, the `ordinal`th
    /// asked, if it is one of `count` entries.
    fn code(&self, (ordinal, n): (usize, usize), count: usize) -> Option<usize> {
        let (codes, first) = &self.unpacked;
        let at = first.map_or(Some(ordinal), |low| n.checked_sub(low))?;
        usize::try_from(*codes.get(at)?).ok().filter(|&code| code < count)
    }

    /// Adds the codes unpacked of a run of rows none of which is null as
    /// the rows' codes, each past `base`, the place of the first of the
    /// `count` entries its dictionary gave whole, in one loop. None if a
    /// code is of no entry.
    fn run_codes(&mut self, base: u32, count: usize) -> Option<()> {
        let (unpacked, codes) = (&self.unpacked.0, self.codes.as_mut()?);
        let past = unpacked.iter().copied().max().map_or(Some(0), |most| usize::try_from(most).ok()?.checked_add(1))?;
        base.checked_add(u32::try_from(count).ok()?)?;
        if past > count {
            return None;
        }
        codes.extend(unpacked.iter().map(|&code| base.wrapping_add(u32::try_from(code).unwrap_or(0))));
        Some(())
    }

    /// Adds every entry of a dictionary of `count` entries, `bytes` that
    /// `offsets` place, each padded to `pad` characters if given, and
    /// gives the place of the first.
    fn whole(&mut self, (offsets, bytes): (&[u8], &[u8]), count: usize, pad: Option<usize>) -> Option<u32> {
        let base = u32::try_from(self.ends.len()).ok()?;
        if pad.is_none() {
            self.fields((offsets, bytes), (0, count))?;
            return Some(base);
        }
        for e in 0..count {
            let (from, to) = between(offsets, e)?;
            put(&mut self.octets, bytes.get(from..to)?, pad);
            self.entry()?;
        }
        Some(base)
    }

    /// The place of the entry of `code` of the dictionary being read by
    /// the entries its rows use, its entries `bytes` that `offsets`
    /// place: the one given when a row before used the code, else a new
    /// one, the entry's octets added, padded to `pad` characters if
    /// given. `first` is how many entries were held before the chunk.
    fn used(&mut self, (offsets, bytes): (&[u8], &[u8]), (code, first): (usize, usize), pad: Option<usize>) -> Option<u32> {
        let held = *self.given.get(code)?;
        if usize::try_from(held).is_ok_and(|place| place >= first && place < self.ends.len()) {
            return Some(held);
        }
        let (from, to) = between(offsets, code)?;
        put(&mut self.octets, bytes.get(from..to)?, pad);
        let place = self.entry()?;
        *self.given.get_mut(code)? = place;
        Some(place)
    }

    /// \[DESIGN\] (RULES.md D1252) The texts gathered, of `rows` rows:
    /// the octets judged UTF-8 once by `kernels` and made the text, the
    /// vector itself, each entry's end on a character's boundary. None if
    /// the rows gathered are not `rows` or the octets are no text.
    pub(super) fn finish(self, kernels: &dyn Kernels) -> Option<Texts> {
        let Gathered { octets, ends, codes, nulls, rows, .. } = self;
        if codes.as_ref().map_or(ends.len(), Vec::len) != rows || nulls.len() != rows {
            return None;
        }
        let text = kernels.text(octets)?;
        if !ends.iter().all(|&end| text.is_char_boundary(end)) {
            return None;
        }
        let nulls = nulls.any().then_some(nulls);
        Some(match codes {
            Some(codes) => Texts::coded(Coded { codes, entries: Arc::new(Texts::new(text, ends, None)) }, nulls),
            None => Texts::new(text, ends, nulls),
        })
    }
}

impl Chunk<'_> {
    /// \[DESIGN\] (RULES.md D1252, D1316) Adds to `into` the texts of its
    /// rows `asked`, character strings each padded to `pad` characters if
    /// given, as a CHARACTER string is read, and whether each is null.
    /// None if a position is past its end, its encoding holds no text, or
    /// it is malformed. What `into` holds is then of no use.
    pub(super) fn gather(&self, asked: Asked<'_>, pad: Option<usize>, into: &mut Gathered) -> Option<()> {
        if !asked.within(self.count) {
            return None;
        }
        if asked.count() == 0 {
            return Some(());
        }
        match &self.body {
            Body::Dictionary { bits, offsets, entries, codes, .. } => {
                into.unpack((codes, *bits), asked)?;
                self.dictionary((offsets, entries), asked, pad, into)?;
            }
            Body::Varying { offsets, values } => self.varying((offsets, values), asked, pad, into)?,
            Body::Constant { field } => self.constant(field, asked, pad, into)?,
            Body::Fsst(coded) => self.symbols(coded, asked, pad, into)?,
            Body::Fixed { .. } | Body::Frame { .. } | Body::Runs(_) | Body::Alp(_) => return None,
        }
        match asked {
            Asked::Run(from, to) => self.nulls_of((from, to), &mut into.nulls),
            Asked::At(places) => places.iter().for_each(|&n| into.nulls.push(self.is_null(n))),
        }
        Some(())
    }

    /// Whether no row at positions `from` to `to` is null, told of the
    /// whole bytes of its null bits those rows lie in (`nulls_of`), so a
    /// null beside them in a byte reads as one among them.
    fn none_null(&self, (from, to): (usize, usize)) -> bool {
        let (first, last) = (bit_of(from).0, bit_of(to.saturating_add(BITS.saturating_sub(1))).0);
        self.nulls.get(first..last).unwrap_or_default().iter().all(|&b| b == 0)
    }

    /// \[DESIGN\] (RULES.md D1316) Adds the rows `asked` of a dictionary
    /// whose entries are `bytes` that `offsets` place, their codes
    /// unpacked in `into`: its entries whole if the rows are no fewer
    /// than they, else those the rows use, and each row's code among the
    /// run's entries, a null's 0.
    fn dictionary(&self, (offsets, bytes): (&[u8], &[u8]), asked: Asked<'_>, pad: Option<usize>, into: &mut Gathered) -> Option<()> {
        let count = offsets.len().checked_div(WORD)?.checked_sub(1)?;
        into.coded()?;
        let first = into.ends.len();
        let base = if asked.count() >= count {
            Some(into.whole((offsets, bytes), count, pad)?)
        } else {
            // A place for each code's entry, none given yet of any new.
            if into.given.len() < count {
                into.given.resize(count, NONE);
            }
            None
        };
        // A run with no null among its rows takes its codes in one loop.
        if let (Some(base), Asked::Run(from, to)) = (base, asked)
            && self.none_null((from, to))
        {
            return into.run_codes(base, count);
        }
        let mut ordinal = 0_usize;
        asked.each(|n| {
            let at = (ordinal, n);
            ordinal = ordinal.saturating_add(1);
            if self.is_null(n) {
                return into.null();
            }
            let code = into.code(at, count)?;
            let place = match base {
                Some(base) => base.checked_add(u32::try_from(code).ok()?)?,
                None => into.used((offsets, bytes), (code, first), pad)?,
            };
            into.codes.as_mut()?.push(place);
            Some(())
        })
    }

    /// \[DESIGN\] (RULES.md D1131) Adds the rows `asked` of varying
    /// fields of `values` that `offsets` place, each an entry of its own:
    /// of a run padded to nothing, by one copy of its octets, a null's
    /// field being empty as it is written.
    fn varying(&self, (offsets, values): (&[u8], &[u8]), asked: Asked<'_>, pad: Option<usize>, into: &mut Gathered) -> Option<()> {
        if let (Asked::Run(from, to), None) = (asked, pad) {
            let first = u32::try_from(into.ends.len()).ok()?;
            into.fields((offsets, values), (from, to))?;
            let past = u32::try_from(into.ends.len()).ok()?;
            if let Some(codes) = &mut into.codes {
                codes.extend(first..past);
            }
            return Some(());
        }
        asked.each(|n| {
            if self.is_null(n) {
                return into.null();
            }
            let (from, to) = between(offsets, n)?;
            put(&mut into.octets, values.get(from..to)?, pad);
            into.own()
        })
    }

    /// \[DESIGN\] (RULES.md D1316) Adds the rows `asked` of a chunk of
    /// the one value `field`: one entry, if a row asked is not null, and
    /// its code of each such row, a null's 0.
    fn constant(&self, field: &[u8], asked: Asked<'_>, pad: Option<usize>, into: &mut Gathered) -> Option<()> {
        into.coded()?;
        let mut held = None;
        asked.each(|n| {
            if self.is_null(n) {
                return into.null();
            }
            if held.is_none() {
                put(&mut into.octets, field, pad);
                held = Some(into.entry()?);
            }
            into.codes.as_mut()?.push(held?);
            Some(())
        })
    }

    /// \[DESIGN\] (RULES.md D1252) Adds the rows `asked` of texts coded
    /// by a table of symbols, each decoded straight after the octets
    /// gathered and an entry of its own. They are VARCHAR texts alone
    /// (`columns.rs`), so none is padded, and one asked padded gives none.
    fn symbols(&self, coded: &super::fsst::Coded<'_>, asked: Asked<'_>, pad: Option<usize>, into: &mut Gathered) -> Option<()> {
        if pad.is_some() {
            return None;
        }
        asked.each(|n| {
            if self.is_null(n) {
                return into.null();
            }
            coded.text_onto(n, &mut into.octets)?;
            into.own()
        })
    }
}

// Unit tests: each encoding's rows gathered as their values, as a run and
// at positions, with nulls, the empty string and padding; rows across
// encodings; entries whole and by use; octets no text holds (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
