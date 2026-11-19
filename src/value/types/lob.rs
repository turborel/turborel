//! [ISO/IEC 9075-2:2023 §6.1, §4.3, §4.4] CHARACTER LARGE OBJECT(length)
//! and BINARY LARGE OBJECT(length): large object strings, whose values are
//! `Value::Lob`, read a chunk at a time (RULES.md D79).
//!
//! A large object is compared for equality alone [§4.3.3.4, §4.4.2], by
//! the executor, which reads it a chunk at a time and may fail as it does
//! (`functions/lobs.rs`); `compare` here compares only values held inline.
//! It is in no key [§9.12 SR3, §9.14 SR3]. In a record it is its octets,
//! if a table keeps them in the row, or the id of the large object the
//! table keeps them in (`engine/rows/lobs.rs`); in the log, the same.

use std::cmp::Ordering;

use super::character::{put_units, read_units};
use super::{Descriptor, SqlType};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::value::lob::{Lob, LobKind, Part, count_chars};
use crate::value::{Binary, Bucket, CharUnits, DataType, Value, assignment_rule, compare_strings, unassignable};

/// [ISO/IEC 9075-2:2023 §6.1 SR1] CLOB, CHARACTER LARGE OBJECT by another
/// name; no key word here, as no key word of SQL-92.
pub const CLOB_NAME: &str = "CLOB";
/// [ISO/IEC 9075-2:2023 §6.1 SR1] NCLOB, NATIONAL CHARACTER LARGE OBJECT
/// by another name.
pub const NCLOB_NAME: &str = "NCLOB";
/// [ISO/IEC 9075-2:2023 §6.1 SR1] BLOB, BINARY LARGE OBJECT by another
/// name.
pub const BLOB_NAME: &str = "BLOB";
/// [ISO/IEC 9075-2:2023 §6.1] LARGE, of `LARGE OBJECT`.
pub const LARGE_NAME: &str = "LARGE";
/// [ISO/IEC 9075-2:2023 §6.1] OBJECT, of `LARGE OBJECT`.
pub const OBJECT_NAME: &str = "OBJECT";

/// The kind of a large object string type.
#[derive(Debug)]
pub struct LobType {
    kind: LobKind,
}

/// CHARACTER LARGE OBJECT(length).
pub static CLOB: LobType = LobType { kind: LobKind::Character };
/// BINARY LARGE OBJECT(length).
pub static BLOB: LobType = LobType { kind: LobKind::Binary };

/// The record and log form of a large object kept in its row: its octets.
const IN_ROW: u8 = 0;
/// The record and log form of a large object its table keeps apart: its
/// id, octets and characters.
const KEPT: u8 = 1;

/// The kind of large object string type of `kind`.
pub(crate) fn kind_of(kind: LobKind) -> &'static LobType {
    match kind {
        LobKind::Character => &CLOB,
        LobKind::Binary => &BLOB,
    }
}

/// The maximum length and its units of `t`, a large object string type.
fn length_of(t: &DataType) -> (u64, CharUnits) {
    if let DataType::Lob { length, units, .. } = t { (*length, *units) } else { (0, CharUnits::Characters) }
}

/// `v` as a large object of `kind`, if it is a string of its family: a
/// character string's text, or a binary string's octets, held inline.
pub(crate) fn as_lob(v: &Value, kind: LobKind) -> Option<Lob> {
    match (v, kind) {
        (Value::Lob(l), _) if l.kind() == kind => Some(l.clone()),
        (Value::Char(s), LobKind::Character) => Some(Lob::of_text(s)),
        (Value::Binary(b), LobKind::Binary) => Some(Lob::of_octets(b.bytes())),
        (
            Value::Null
            | Value::Lob(_)
            | Value::Char(_)
            | Value::Binary(_)
            | Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Json(_)
            | Value::Extension(_)
            | Value::Xml(_),
            _,
        ) => None,
    }
}

/// A large object cut to a length: what is kept, whether any was cut, and
/// whether what was cut holds other than padding, spaces of a character
/// one, X'00' of a binary one.
struct Cut {
    kept: Lob,
    shortened: bool,
    lost: bool,
}

/// [ISO/IEC 9075-2:2023 §6.13 GR11d, GR12d, GR13; §9.2 GR3b, GR3e] `lob`
/// cut to `length`, in octets if `by_octets` or it is binary, else in
/// characters; whole characters of no more than `length` octets.
///
/// # Errors
///
/// As reading the chunks the cut falls in, and those after it, fails.
fn cut_to(lob: Lob, length: u64, by_octets: bool) -> Result<Cut> {
    let by_octets = by_octets || lob.kind() == LobKind::Binary;
    let count = if by_octets { lob.octets() } else { lob.chars() };
    if count <= length {
        return Ok(Cut { kept: lob, shortened: false, lost: false });
    }
    let kept = match (lob.kind(), by_octets) {
        (LobKind::Binary, _) => lob.slice_octets(0, length),
        (LobKind::Character, true) => lob.slice_chars(0, whole_chars_within(&lob, length)?)?,
        (LobKind::Character, false) => lob.slice_chars(0, length)?,
    };
    let pad = if lob.kind() == LobKind::Binary { 0 } else { b' ' };
    let mut lost = false;
    lob.reader().each(kept.octets(), lob.octets(), &mut |piece| {
        lost = piece.iter().any(|&b| b != pad);
        Ok(!lost)
    })?;
    Ok(Cut { kept, shortened: true, lost })
}

/// The characters of `lob` whose octets all lie within its first `most`.
fn whole_chars_within(lob: &Lob, most: u64) -> Result<u64> {
    let head = lob.reader().read(0, most.saturating_add(1))?;
    let cut = head.get(..usize::try_from(most).unwrap_or(usize::MAX).min(head.len())).unwrap_or_default();
    let mut end = cut.len();
    // Back off a character `most` would split.
    if head.get(end).is_some_and(|b| b & 0xC0 == 0x80) {
        while end > 0 && cut.get(end.saturating_sub(1)).is_some_and(|b| b & 0xC0 == 0x80) {
            end = end.saturating_sub(1);
        }
        end = end.saturating_sub(1);
    }
    Ok(count_chars(cut.get(..end).unwrap_or_default()))
}

/// The text of `lob`, a character large object, without its leading and
/// trailing spaces, if no longer than `most` octets; the spaces found by
/// reading from each end, a chunk at a time.
///
/// # Errors
///
/// As reading a chunk fails; SQLCODE -910 if the octets are not UTF-8.
fn trimmed(lob: &Lob, most: u64) -> Result<Option<String>> {
    let mut front = 0_u64;
    lob.reader().each(0, lob.octets(), &mut |piece| {
        let spaces = piece.iter().take_while(|&&b| b == b' ').count();
        front = front.saturating_add(u64::try_from(spaces).unwrap_or(u64::MAX));
        Ok(spaces == piece.len())
    })?;
    let (mut end, window) = (lob.octets(), u64::try_from(crate::value::lob::CHUNK_OCTETS).unwrap_or(u64::MAX));
    while end > front {
        let from = end.saturating_sub(window).max(front);
        let piece = lob.reader().read(from, end.saturating_sub(from))?;
        let spaces = piece.iter().rev().take_while(|&&b| b == b' ').count();
        end = end.saturating_sub(u64::try_from(spaces).unwrap_or(u64::MAX));
        if spaces < piece.len() {
            break;
        }
    }
    if end.saturating_sub(front) > most {
        return Ok(None);
    }
    let bytes = lob.reader().read(front, end.saturating_sub(front))?;
    String::from_utf8(bytes).map(Some).map_err(|_| crate::value::lob::broken("a character large object's octets are not UTF-8"))
}

/// How a large object is taken to a type of no large object: by CAST, or
/// by store or retrieval assignment of the rule cited.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Lowering<'r> {
    /// [ISO/IEC 9075-2:2023 §6.13] CAST.
    Cast,
    /// [ISO/IEC 9075-2:2023 §9.1, §9.2] Assignment, as the rule says.
    Assign(&'r str),
}

/// [ISO/IEC 9075-2:2023 §6.13 GR11d, GR12d, GR13; §9.2 GR3b, GR3e] `lob`
/// as a value of the string type `to` of its family, reading only what
/// fits: the value kept, a character string's text or a binary string's
/// octets, and the warning string data, right truncation if CAST cut
/// other than spaces of a character one, or any of a binary one. Of a
/// character large object and a type of another kind, its text without
/// its leading and trailing spaces, which that kind's CAST trims anyway
/// [§6.13 GR6b, GR8b]. None of any other target, which refuses it.
///
/// # Errors
///
/// SQLSTATE 22001 if assignment would cut other than padding \[§9.2 GR3b
/// ix 2, GR3b xii 2, GR3e ii\]; 22018 if CAST is given a character large
/// object longer than the longest character string, past its spaces, as no
/// literal is; as reading a chunk fails.
pub(crate) fn lowered(lob: Lob, to: &DataType, how: Lowering<'_>) -> Result<Option<(Value, Option<SqlState>)>> {
    let (length, by_octets) = match (lob.kind(), to) {
        (LobKind::Character, DataType::Character { length, units } | DataType::Varying { length, units }) => {
            (*length, *units == CharUnits::Octets)
        }
        (LobKind::Binary, DataType::Binary { length } | DataType::VarBinary { length }) => (*length, true),
        (LobKind::Character, DataType::Exact { .. } | DataType::Approximate { .. } | DataType::DecFloat { .. })
        | (LobKind::Character, DataType::Boolean | DataType::Date | DataType::Time { .. } | DataType::Timestamp { .. })
        | (LobKind::Character, DataType::Interval(_)) => return text_of(&lob, to, how),
        (LobKind::Character | LobKind::Binary, _) => return Ok(None),
    };
    let kind = lob.kind();
    let cut = cut_to(lob, u64::from(length), by_octets)?;
    let warned = match kind {
        LobKind::Character => cut.lost,
        LobKind::Binary => cut.shortened,
    };
    if let Lowering::Assign(rule) = how
        && cut.lost
    {
        let at = match (kind, to) {
            (LobKind::Binary, DataType::Binary { .. }) => "ISO/IEC 9075-2:2023 §9.2 GR3b ix 2",
            (LobKind::Binary, _) => "ISO/IEC 9075-2:2023 §9.2 GR3b xii 2",
            (LobKind::Character, _) => "ISO/IEC 9075-2:2023 §9.2 GR3e ii",
        };
        let msg = format!("[{}] a large object longer than {to}, not padding beyond it, does not fit it", assignment_rule(rule, at));
        return fail_with(Code::Data, SqlState::STRING_DATA_RIGHT_TRUNCATION, msg);
    }
    let bytes = cut.kept.reader().read(0, cut.kept.octets())?;
    let value = match kind {
        LobKind::Character => {
            Value::Char(String::from_utf8(bytes).map_err(|_| crate::value::lob::broken("a character large object's octets are not UTF-8"))?)
        }
        LobKind::Binary => Value::Binary(Binary::new(bytes)),
    };
    let warning = (matches!(how, Lowering::Cast) && warned).then_some(SqlState::STRING_TRUNCATED);
    Ok(Some((value, warning)))
}

/// [ISO/IEC 9075-2:2023 §6.13 GR6b, GR8b] The text of `lob`, a character
/// large object, past its spaces, for CAST to `to`, a type of another kind.
fn text_of(lob: &Lob, to: &DataType, how: Lowering<'_>) -> Result<Option<(Value, Option<SqlState>)>> {
    if let Lowering::Assign(_) = how {
        return Ok(None);
    }
    match trimmed(lob, u64::from(crate::value::MAX_CHARACTER_LENGTH))? {
        Some(text) => Ok(Some((Value::Char(text), None))),
        None => fail_with(
            Code::Data,
            SqlState::INVALID_CHARACTER_VALUE_FOR_CAST,
            format!("[ISO/IEC 9075-2:2023 §6.13, §5.3] a character large object of {} characters is no literal of {to}", lob.chars()),
        ),
    }
}

impl LobType {
    /// The value `v`, of a string of its family, as a large object of `t`,
    /// cut to its maximum length.
    fn of(&self, v: &Value, t: &DataType) -> Option<Result<Cut>> {
        let (length, units) = length_of(t);
        as_lob(v, self.kind).map(|lob| cut_to(lob, length, units == CharUnits::Octets))
    }

    /// The name of a type of it, of `units`, by `name`.
    fn written_as(&self, length: u64, units: CharUnits) -> String {
        match units {
            CharUnits::Octets if self.kind == LobKind::Character => format!("{}({length} OCTETS)", self.name()),
            CharUnits::Characters | CharUnits::Octets => format!("{}({length})", self.name()),
        }
    }
}

impl SqlType for LobType {
    fn name(&self) -> &'static str {
        match self.kind {
            LobKind::Character => "CHARACTER LARGE OBJECT",
            LobKind::Binary => "BINARY LARGE OBJECT",
        }
    }

    fn written(&self, t: &DataType) -> String {
        let (length, units) = length_of(t);
        self.written_as(length, units)
    }

    /// [ISO/IEC 9075-2:2023 §4.3.2, §4.4.2] A character large object is
    /// comparable with character strings, a binary one with binary strings.
    fn comparable(&self, other: &DataType) -> bool {
        match self.kind {
            LobKind::Character => other.is_character(),
            LobKind::Binary => other.is_binary(),
        }
    }

    /// [ISO/IEC 9075-2:2023 §4.3.3.4, §4.4.2] Of values held inline alone:
    /// the executor compares others for equality (`functions/lobs.rs`).
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        let (a, b) = (as_lob(a, self.kind)?, as_lob(b, self.kind)?);
        let (x, y) = (a.inline()?, b.inline()?);
        match self.kind {
            LobKind::Character => Some(compare_strings(std::str::from_utf8(x).ok()?, std::str::from_utf8(y).ok()?)),
            LobKind::Binary => Some(x.cmp(y)),
        }
    }

    /// None: no large object is grouped or hashed [ISO/IEC 9075-2:2023
    /// §9.12 SR3], so none is in a bucket.
    fn bucket(&self, _: &Value) -> Option<Bucket> {
        None
    }

    /// [ISO/IEC 9075-2:2023 §9.2 GR3e, GR3b xii] As it is, if no longer
    /// than the maximum length; else cut to it, if what is cut is
    /// `<truncating whitespace>` of a character large object, X'00' of a
    /// binary one.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        let Some(fit) = self.of(&v, t) else { return unassignable(&v, t, rule) };
        let Cut { kept, lost, .. } = fit?;
        if lost {
            let at =
                if self.kind == LobKind::Binary { "ISO/IEC 9075-2:2023 §9.2 GR3b xii 2" } else { "ISO/IEC 9075-2:2023 §9.2 GR3e ii" };
            let msg = format!("[{}] a large object longer than {t}, not padding beyond it, does not fit it", assignment_rule(rule, at));
            return fail_with(Code::Data, SqlState::STRING_DATA_RIGHT_TRUNCATION, msg);
        }
        Ok(Value::Lob(kept))
    }

    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        self.cast_warned(v, t).map(|(cast, _)| cast)
    }

    /// [ISO/IEC 9075-2:2023 §6.13 GR12, GR13b] A string of its family cut
    /// to the maximum length, with the warning string data, right
    /// truncation if what is cut is other than spaces of a character large
    /// object, or any of a binary one \[GR12d ii, GR13b ii\]; of a character
    /// large object, a number's, datetime's, interval's or truth value's
    /// literal, as CHARACTER VARYING takes it \[GR12a to GR12c, GR12e,
    /// GR12f\]; a value of no other type \[SR7, SR8\].
    fn cast_warned(&self, v: Value, t: &DataType) -> Result<(Value, Option<SqlState>)> {
        if v.is_null() {
            return Ok((Value::Null, None));
        }
        if let Some(fit) = self.of(&v, t) {
            let Cut { kept, shortened, lost } = fit?;
            let warned = if self.kind == LobKind::Binary { shortened } else { lost };
            return Ok((Value::Lob(kept), warned.then_some(SqlState::STRING_TRUNCATED)));
        }
        let literal =
            matches!(v, Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_) | Value::Bool(_) | Value::Datetime(_) | Value::Interval(_));
        if self.kind == LobKind::Binary || !literal {
            return crate::value::cast::uncastable(&v, t).map(|v| (v, None));
        }
        let (length, units) = length_of(t);
        let most = u32::try_from(length).unwrap_or(u32::MAX).min(crate::value::MAX_CHARACTER_LENGTH);
        let (text, warning) = super::VARYING.cast_warned(v, &DataType::Varying { length: most, units })?;
        let lob = as_lob(&text, LobKind::Character).ok_or_else(|| crate::value::lob::broken("a literal is not a character string"))?;
        Ok((Value::Lob(lob), warning))
    }

    /// Its literal, if held inline: a character string literal, or a
    /// binary string literal.
    fn literal(&self, v: &Value) -> String {
        match (as_lob(v, self.kind), self.kind) {
            (Some(l), LobKind::Character) if l.inline().is_some() => format!("'{}'", l.to_string().replace('\'', "''")),
            (Some(l), LobKind::Binary) if l.inline().is_some() => format!("X'{l}'"),
            (Some(_) | None, LobKind::Character | LobKind::Binary) => v.to_string(),
        }
    }

    fn text(&self, v: &Value) -> String {
        v.to_string()
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    /// [ISO/IEC 9075-2:2023 §6.1] Of a character large object, its length
    /// and four octets to a character, the most UTF-8 takes, and its
    /// collation; of a binary one, its length in octets.
    fn descriptor(&self, t: &DataType) -> Descriptor {
        let (length, units) = length_of(t);
        let octets = if self.kind == LobKind::Binary || units == CharUnits::Octets {
            length
        } else {
            length.saturating_mul(u64::try_from(super::character::OCTETS_PER_CHARACTER).unwrap_or(u64::MAX))
        };
        Descriptor { length: Some(length), octets: Some(octets), collated: self.kind == LobKind::Character, ..Descriptor::of(self.name()) }
    }

    /// Its octets, if held inline; else the id its table keeps it by, if
    /// its value is one whole reference, as a row the log carries holds
    /// (`engine/rows/lobs.rs`).
    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        put_lob(v, out);
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        read_lob(r, self.kind)
    }

    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        let (length, units) = length_of(t);
        out.put_u64(length);
        put_units(units, out);
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        Ok(DataType::Lob { kind: self.kind, length: r.u64()?, units: read_units(r)? })
    }

    /// Of variable length.
    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    /// As `put_value`: a table has made it held inline, or a reference to
    /// the large object it keeps.
    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        if !matches!(v, Value::Lob(l) if l.kind() == self.kind) {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}")));
        }
        let mut out = Vec::new();
        put_lob(v, &mut out);
        Ok(out)
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        read_lob(&mut Reader::new(field, "a large object"), self.kind).ok()
    }

    /// None: no large object is in a key [ISO/IEC 9075-2:2023 §9.12 SR3].
    fn encode_key(&self, v: &Value, t: &DataType, _: &mut Vec<u8>) -> Result<()> {
        Err(SqlError::new(Code::Api, format!("[ISO/IEC 9075-2:2023 §9.12 SR3] {v} of {t} is in no key, as no large object is")))
    }

    fn decode_key(&self, _: &mut &[u8], t: &DataType) -> Result<Value> {
        Err(SqlError::new(Code::Api, format!("[ISO/IEC 9075-2:2023 §9.12 SR3] no key is of {t}, as no large object is")))
    }

    fn anchor(&self, _: &Value, _: &DataType) -> Option<(Value, bool)> {
        None
    }
}

/// Writes `v`, a large object, as a record, the log and the catalog keep
/// it: its octets if held inline, or the id of the large object its table
/// keeps, with its octets and characters. A value of neither form, which a
/// table never hands on, is written as no octets.
pub(crate) fn put_lob(v: &Value, out: &mut Vec<u8>) {
    let Value::Lob(l) = v else {
        out.put_u8(IN_ROW);
        out.put_bytes(&[]);
        return;
    };
    match (l.inline(), l.parts()) {
        (Some(bytes), _) => {
            out.put_u8(IN_ROW);
            out.put_bytes(bytes);
        }
        (None, [Part::Reference(id)]) => {
            out.put_u8(KEPT);
            out.put_u64(*id);
            out.put_u64(l.octets());
            out.put_u64(l.chars());
        }
        (None, [Part::Slice(s)]) if s.whole() && s.source.kept().is_some() => {
            out.put_u8(KEPT);
            out.put_u64(s.source.kept().map_or(0, |k| k.id));
            out.put_u64(l.octets());
            out.put_u64(l.chars());
        }
        (None, _) => {
            out.put_u8(IN_ROW);
            out.put_bytes(&[]);
        }
    }
}

/// Reads a large object of `kind` `put_lob` wrote.
///
/// # Errors
///
/// SQLCODE -916 if the bytes are not one.
pub(crate) fn read_lob(r: &mut Reader<'_>, kind: LobKind) -> Result<Value> {
    match r.u8()? {
        IN_ROW => {
            let bytes = r.bytes()?;
            match kind {
                LobKind::Character => std::str::from_utf8(bytes).map(|s| Value::Lob(Lob::of_text(s))).map_err(|_| super::malformed()),
                LobKind::Binary => Ok(Value::Lob(Lob::of_octets(bytes))),
            }
        }
        KEPT => {
            let (id, octets, chars) = (r.u64()?, r.u64()?, r.u64()?);
            Ok(Value::Lob(Lob::of_reference(kind, id, (octets, chars))))
        }
        _ => Err(super::malformed()),
    }
}

#[cfg(test)]
mod tests;
