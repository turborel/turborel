//! [ISO/IEC 9075-2:2023 §6.32, §6.33] The string operations on large
//! objects that read or make one (RULES.md D79): an operand made a large
//! object of its family, concatenation cut to the longest large object,
//! SUBSTRING by octets of a character large object, UPPER and LOWER made a
//! chunk at a time, and the characters before an octet counted.

use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::value::Value;
use crate::value::lob::{CHUNK_OCTETS, Lob, LobBuilder, LobKind, MAX_LOB_LENGTH, TempLobs, broken};

/// Whether `b` begins a UTF-8 character.
fn begins(b: u8) -> bool {
    b & 0xC0 != 0x80
}

/// `v`, a string of the family of `kind`, as a large object of it; none
/// if it is null.
///
/// # Errors
///
/// SQLCODE -910 if it is a string of the other family or no string.
pub(crate) fn as_large(v: &Value, kind: LobKind) -> Result<Option<Lob>> {
    let other = || fail(Code::Api, "[DESIGN] a string function was given a string of another family than its large object's");
    match (v, kind) {
        (Value::Null, _) => Ok(None),
        (Value::Lob(l), _) => {
            if l.kind() == kind {
                Ok(Some(l.clone()))
            } else {
                other()
            }
        }
        (Value::Char(s), LobKind::Character) => Ok(Some(Lob::of_text(s))),
        (Value::Binary(b), LobKind::Binary) => Ok(Some(Lob::of_octets(b.bytes()))),
        (
            Value::Char(_)
            | Value::Binary(_)
            | Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Row(_)
            | Value::Extension(_),
            _,
        ) => other(),
    }
}

/// [ISO/IEC 9075-2:2023 §6.32 GR2b ii 3A, GR3b i] `joined`, the operands
/// of `||` joined, if no longer than `most` characters or octets; else its
/// first `most`, if what is past them is all spaces, the `<truncating
/// whitespace>` (IV074), of a character large object, or X'00' of a binary
/// one.
///
/// # Errors
///
/// SQLSTATE 22001 if anything else is past them; as reading a chunk fails.
pub(crate) fn within(joined: Lob, most: u64) -> Result<Lob> {
    if joined.chars() <= most {
        return Ok(joined);
    }
    let (pad, rule) = match joined.kind() {
        LobKind::Character => (b' ', "§6.32 GR2b ii 3A III"),
        LobKind::Binary => (0_u8, "§6.32 GR3b i 3"),
    };
    let from = joined.reader().octet_of_char(most)?;
    let padding = joined.reader().each(from, joined.octets(), &mut |piece| Ok(piece.iter().all(|&b| b == pad)))?;
    if !padding {
        return fail_with(
            Code::Data,
            SqlState::STRING_DATA_RIGHT_TRUNCATION,
            format!("[ISO/IEC 9075-2:2023 {rule}] || of {} is longer than {most}", joined.chars()),
        );
    }
    joined.slice_chars(0, most)
}

/// The value of `||` of `a` and `b`, either a large object, of the family
/// of `kind`: null if either is \[GR2b i, GR3a\].
///
/// # Errors
///
/// As `within`.
pub(crate) fn concatenation(a: &Value, b: &Value, kind: LobKind) -> Result<Value> {
    let (Some(x), Some(y)) = (as_large(a, kind)?, as_large(b, kind)?) else { return Ok(Value::Null) };
    Ok(Value::Lob(within(x.concat(&y), MAX_LOB_LENGTH)?))
}

/// The characters of `lob` before octet `octet`: its octets, of a binary
/// large object.
///
/// # Errors
///
/// As reading a chunk fails.
pub(crate) fn chars_before(lob: &Lob, octet: u64) -> Result<u64> {
    if lob.kind() == LobKind::Binary {
        return Ok(octet);
    }
    let mut count = 0_u64;
    lob.reader().each(0, octet, &mut |piece| {
        let n = piece.iter().filter(|&&b| begins(b)).count();
        count = count.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        Ok(true)
    })?;
    Ok(count)
}

/// [ISO/IEC 9075-2:2023 §6.33 GR3a] The characters of `lob`, a character
/// large object, whose octets lie wholly within octets `from` to `to`,
/// from 0, end excluded: a character cut by either end is not taken
/// (UV074), as of a character string.
///
/// # Errors
///
/// As reading a chunk fails.
pub(crate) fn within_octets(lob: &Lob, from: u64, to: u64) -> Result<Lob> {
    let mut reader = lob.reader();
    // The characters begun before `from` are not taken, one cut by it
    // among them; nor is one begun before `to` and cut by it.
    let first = chars_before(lob, from)?;
    let end = chars_before(lob, to)?.saturating_sub(u64::from(!starts_at(&mut reader, to)?));
    lob.slice_chars(first, end.saturating_sub(first))
}

/// Whether a character begins at octet `at` of the value `reader` reads,
/// or `at` is its end.
fn starts_at(reader: &mut crate::value::lob::LobReader<'_>, at: u64) -> Result<bool> {
    Ok(reader.read(at, 1)?.first().is_none_or(|&b| begins(b)))
}

/// [ISO/IEC 9075-2:2023 §6.33 GR7] `lob`, a character large object, with
/// each simple Latin letter made upper case if `upper`, else lower case,
/// as of a character string; written a chunk at a time, to `temp` past
/// one.
///
/// # Errors
///
/// As reading or writing a chunk fails; SQLCODE -910 if it is longer than
/// a chunk and no temporary store is given.
pub(crate) fn fold(lob: &Lob, upper: bool, temp: Option<&dyn TempLobs>) -> Result<Lob> {
    let mut built = LobBuilder::new(LobKind::Character, temp);
    let mut folded = Vec::with_capacity(CHUNK_OCTETS.min(usize::try_from(lob.octets()).unwrap_or(CHUNK_OCTETS)));
    lob.reader().each(0, lob.octets(), &mut |piece| {
        // An ASCII letter is one octet and no other character's octet is
        // one, so a piece folds alone, wherever it is cut.
        folded.clear();
        folded.extend(piece.iter().map(|b| if upper { b.to_ascii_uppercase() } else { b.to_ascii_lowercase() }));
        built.push_bytes(&folded)?;
        Ok(true)
    })?;
    let made = built.finish()?;
    if made.octets() != lob.octets() {
        return Err(broken("a folded large object is not of its source's length"));
    }
    Ok(made)
}
