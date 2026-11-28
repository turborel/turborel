//! \[DESIGN\] The column layout's chunks (RULES.md D31): the values of one
//! column of a frozen row group, a run of consecutive rows to a page, each
//! value as its type writes it in a record (`SqlType::encode_record`), so
//! every type, a plug-in's too, is kept.
//!
//! ```text
//! chunk:     kind u8 | count u32 | nulls, ceil(count / 8) bytes, a bit set
//!            for each null | values, as their encoding writes them
//!            (`encoded.rs`)
//! ```
//!
//! Integers are little-endian. Of the encodings that hold a chunk's values,
//! the one that takes fewest bytes is chosen, and a chunk holds as many
//! values as its page does so encoded; or, not compressed, fixed if its
//! type has a width in a record and every value is written in it, else
//! varying. Built on:
//!
//! * `group`: a frozen row group, its row ids, its chunks, the least and
//!   greatest value and nulls of each, and the pages they are on;
//! * `scan`: the rows of a group a reader asks for, chunks a condition
//!   cannot match left unread;
//! * `ahead`: the pages a scan of groups will read, read ahead.

pub mod ahead;
mod alp;
mod bloom;
pub mod cache;
mod encoded;
mod fields;
mod fsst;
pub mod group;
pub mod imprints;
pub mod judged;
mod packed;
pub mod prepared;
mod runs;
pub mod scan;
pub mod sketch;
mod texts;

pub(crate) use encoded::kind_name;
pub use fields::Fields;
pub(crate) use packed::unpack_words;

use crate::error::{Code, Result, SqlError};
use crate::storage::Mask;
use crate::value::{CharUnits, DataType, Value};

use encoded::Body;
use fsst::Symbols;

/// Bytes of a count, a width or an offset.
const WORD: usize = 4;
/// Bytes before the nulls.
const HEADER: usize = 1 + WORD;
const BITS: usize = 8;

fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a column chunk is malformed: {why}"))
}

fn too_large() -> SqlError {
    SqlError::new(Code::Unsupported, "[DESIGN] a column chunk would be too large")
}

/// The byte of a null bitmap that value `n`'s bit is in, and the bit.
fn bit_of(n: usize) -> (usize, u8) {
    (n.checked_div(BITS).unwrap_or(0), u8::try_from(n.checked_rem(BITS).unwrap_or(0)).unwrap_or(0))
}

/// Bytes of the null bitmap of `count` values.
fn nulls_len(count: usize) -> usize {
    count.div_ceil(BITS)
}

fn word(n: usize) -> Result<[u8; WORD]> {
    u32::try_from(n).map(u32::to_le_bytes).map_err(|_| too_large())
}

fn read_word(bytes: &[u8], at: usize) -> Result<usize> {
    let end = at.checked_add(WORD).ok_or_else(|| corrupt("it is too short"))?;
    let b = bytes.get(at..end).and_then(|b| <[u8; WORD]>::try_from(b).ok()).ok_or_else(|| corrupt("it is too short"))?;
    usize::try_from(u32::from_le_bytes(b)).map_err(|_| corrupt("a count is too large"))
}

/// A chunk, read in place.
pub struct Chunk<'a> {
    count: usize,
    nulls: &'a [u8],
    body: Body<'a>,
}

impl<'a> Chunk<'a> {
    /// The chunk `bytes` hold.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not one.
    pub fn parse(bytes: &'a [u8]) -> Result<Chunk<'a>> {
        Chunk::parse_with(bytes, None)
    }

    /// The chunk `bytes` hold, of texts coded by `table` if given, with the
    /// bytes it takes, read of the same bytes before (`prepared.rs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not one.
    fn parse_with(bytes: &'a [u8], table: Option<(&'a Symbols, usize)>) -> Result<Chunk<'a>> {
        let kind = bytes.first().copied().ok_or_else(|| corrupt("it is empty"))?;
        let count = read_word(bytes, 1)?;
        let nulls_end = HEADER.checked_add(nulls_len(count)).ok_or_else(|| corrupt("its count is too large"))?;
        let nulls = bytes.get(HEADER..nulls_end).ok_or_else(|| corrupt("it is too short"))?;
        Ok(Chunk { count, nulls, body: encoded::body(kind, count, (bytes, nulls_end), table)? })
    }

    /// Of texts coded by a table of symbols, the table and the bytes it
    /// takes, to give when its bytes are parsed again.
    fn table(&self) -> Option<(Box<Symbols>, usize)> {
        match &self.body {
            Body::Fsst(coded) => Some(coded.table()),
            Body::Fixed { .. }
            | Body::Varying { .. }
            | Body::Constant { .. }
            | Body::Frame { .. }
            | Body::Dictionary { .. }
            | Body::Runs(_)
            | Body::Alp(_) => None,
        }
    }

    /// How many values it holds.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Whether its `n`th value is null.
    fn is_null(&self, n: usize) -> bool {
        let (byte, bit) = bit_of(n);
        self.nulls.get(byte).is_some_and(|b| b & (1 << bit) != 0)
    }

    /// \[DESIGN\] Its values at positions `from` to `to`, each an exact
    /// number's units or an approximate number's bits, as integers pushed
    /// to `units`, and whether each is null to `nulls`
    /// (`encoded::Body::ints`): none if its encoding does not hold them.
    pub fn ints(&self, (from, to): (usize, usize), (units, nulls): (&mut Vec<i64>, &mut Mask)) -> Option<()> {
        self.ints_with((from, to), (units, nulls), (None, &crate::kernels::Portable))
    }

    /// As `ints`, of a dictionary whose entries as integers are `entries`
    /// if given, read once for the runs of a group (`prepared.rs`).
    fn ints_with(
        &self,
        (from, to): (usize, usize),
        (units, nulls): (&mut Vec<i64>, &mut Mask),
        entries: (Option<&[i64]>, &dyn crate::kernels::Kernels),
    ) -> Option<()> {
        if to > self.count || from > to {
            return None;
        }
        self.body.ints((from, to), units, entries)?;
        self.nulls_of((from, to), nulls);
        Some(())
    }

    /// \[DESIGN\] (RULES.md D354) Whether each of its values at positions
    /// `from` to `to` is null, its bitmap's bits added to `nulls` as they
    /// are, a word at a time (`Mask::append`): none set at once if no bit
    /// of the range's bytes is.
    fn nulls_of(&self, (from, to): (usize, usize), nulls: &mut Mask) {
        let (first, last) = (bit_of(from).0, bit_of(to.saturating_add(BITS.saturating_sub(1))).0);
        let count = to.saturating_sub(from);
        if self.nulls.get(first..last).unwrap_or_default().iter().all(|&b| b == 0) {
            nulls.extend_unset(count);
            return;
        }
        nulls.append(&Mask::from_lsb_bytes(self.nulls, from, count));
    }

    /// \[DESIGN\] (RULES.md D354) Its values at positions `from` to `to`,
    /// truth values, each true one's bit added to `bits`, a null's unset,
    /// and whether each is null to `nulls` (`nulls_of`): none if a value
    /// not null is not a truth value as a record writes it.
    pub fn bools(&self, (from, to): (usize, usize), (bits, nulls): (&mut Mask, &mut Mask)) -> Option<()> {
        if to > self.count || from > to {
            return None;
        }
        for n in from..to {
            if self.is_null(n) {
                bits.push(false);
                continue;
            }
            let Some(Value::Bool(truth)) = DataType::Boolean.decode_record(self.body.whole_field(n)?) else { return None };
            bits.push(truth);
        }
        self.nulls_of((from, to), nulls);
        Some(())
    }

    /// \[DESIGN\] (RULES.md D1171) As `ints`, of runs or one value, and
    /// where each run among positions `from` to `to` ends, past `base`,
    /// pushed to `ends`, of one reading (`encoded::Body::ints_ending`):
    /// whether its encoding keeps its values so, nothing pushed if not;
    /// none if they are not there. Its runs are unpacked into `unpacked`
    /// (`runs::Unpacked`, RULES.md D1315).
    pub(super) fn ints_ending(
        &self,
        (from, to): (usize, usize),
        (units, nulls): (&mut Vec<i64>, &mut Mask),
        (ends, base): (&mut Vec<usize>, usize),
        unpacked: &mut runs::Unpacked,
    ) -> Option<bool> {
        if to > self.count || from > to {
            return None;
        }
        let ran = self.body.ints_ending((from, to), units, (ends, base), unpacked)?;
        if ran {
            self.nulls_of((from, to), nulls);
        }
        Some(ran)
    }

    /// \[DESIGN\] (RULES.md D1340) Of runs, its values at `positions`,
    /// which ascend from `low` to `high`, pushed to `units`, and whether
    /// each is null to `nulls`, the runs they span unpacked into
    /// `unpacked` where they are dense among them (`Runs::ints_at`):
    /// whether its encoding keeps its values as runs, nothing pushed if
    /// not; none if they are not there.
    pub(super) fn ints_among(
        &self,
        positions: impl ExactSizeIterator<Item = usize> + Clone,
        (low, high): (usize, usize),
        (units, nulls): (&mut Vec<i64>, &mut Mask),
        unpacked: &mut runs::Unpacked,
    ) -> Option<bool> {
        let Body::Runs(runs) = &self.body else { return Some(false) };
        runs.ints_at(positions.clone(), (low, high), units, unpacked)?;
        positions.for_each(|p| nulls.push(self.is_null(p)));
        Some(true)
    }

    /// Its `n`th value, of type `t`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is not there, or not one of `t`.
    pub fn value(&self, n: usize, t: &DataType) -> Result<Value> {
        if n >= self.count {
            return Err(corrupt("a value past its end is asked for"));
        }
        if self.is_null(n) {
            return Ok(Value::Null);
        }
        self.body.value(n, t)
    }
}

/// \[DESIGN\] Of a character string type, the characters a value of it is
/// padded to as it is read, if any: a CHARACTER string's length, written
/// without its trailing spaces (`record.rs`), a VARCHAR's none, written as
/// it is; none of any other type, nor of CHARACTER whose length is in
/// octets, each of whose values is padded to its own number of characters.
pub fn padding(t: &DataType) -> Option<Option<usize>> {
    match t {
        DataType::Character { length, units: CharUnits::Characters } => usize::try_from(*length).ok().map(Some),
        DataType::Varying { .. } => Some(None),
        DataType::Distinct(d) => padding(&d.source),
        DataType::Character { units: CharUnits::Octets, .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Json
        | DataType::Extension(_)
        | DataType::Xml(_) => None,
    }
}

/// Pushes `s` to `text`, padded with spaces to `pad` characters if given
/// (`padding`): a CHARACTER string, written without its trailing spaces,
/// as it is read [X3.135-1986 §8.7 GR7b] (`record.rs`).
pub fn padded(text: &mut String, s: &str, pad: Option<usize>) {
    text.push_str(s);
    if let Some(length) = pad {
        let count = if s.is_ascii() { s.len() } else { s.chars().count() };
        text.extend(std::iter::repeat_n(' ', length.saturating_sub(count)));
    }
}

// Unit tests: chunks read back as written, fixed and varying, with nulls;
// how many values fit; malformed chunks refused; each compressed encoding
// chosen and read back, one value at a time and as integers, texts or
// codes at once (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
