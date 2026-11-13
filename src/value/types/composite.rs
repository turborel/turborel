//! \[DESIGN\] What an array and a row share: values made of values, each
//! of its own type, the parts written one after another (RULES.md D68).
//!
//! ```text
//! value:  parts u32, then each part as `value::codec` writes it
//! record: parts u32, then per part 0 for null, or 1 then its record's
//!         length u32 and bytes, as the part's type writes them
//! ```

use std::cmp::Ordering;
use std::sync::Arc;

use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::value::{Bucket, CompOp, DataType, NormKey, Value, codec, compare};

/// The flag of a part that is null, and of one present.
const ABSENT: u8 = 0;
const PRESENT: u8 = 1;

/// A value not of a column of `t`'s type.
pub(crate) fn mismatch(v: &Value, t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}"))
}

/// [ISO/IEC 9075-2:2023 §8.2 GR1b] How two arrays' or rows' parts compare,
/// pair by pair: as the first pair known unequal does, else unknown if a
/// pair is, else equal. Arrays of other cardinalities are unequal
/// \[GR1b ii\], longer greater.
pub(crate) fn compare_parts(a: &[Value], b: &[Value]) -> Option<Ordering> {
    if a.len() != b.len() {
        return Some(a.len().cmp(&b.len()));
    }
    let mut known = true;
    for (x, y) in a.iter().zip(b) {
        match compare(x, y) {
            Some(Ordering::Equal) => {}
            Some(unequal) => return Some(unequal),
            None => known = false,
        }
    }
    known.then_some(Ordering::Equal)
}

/// [ISO/IEC 9075-2:2023 §4.1] Whether `t` is S-ordered, S the types
/// `member` holds of: one of them, or a row with a field of a type so, or
/// an array whose element type is.
fn ordered_by(t: &DataType, member: fn(&DataType) -> bool) -> bool {
    member(t)
        || match t {
            DataType::Array(a) => ordered_by(&a.element, member),
            DataType::Row(r) => r.fields.iter().any(|f| ordered_by(&f.data_type, member)),
            // Ordered by MAP of its cast to its source [§11.51 GR2c].
            DataType::Distinct(d) => ordered_by(&d.source, member),
            DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Exact { .. }
            | DataType::Approximate { .. }
            | DataType::DecFloat { .. }
            | DataType::Binary { .. }
            | DataType::VarBinary { .. }
            | DataType::Boolean
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Interval(_)
            | DataType::Extension(_) => false,
        }
}

/// [ISO/IEC 9075-2:2023 §4.1] Whether `t` is array-ordered.
pub(crate) fn array_ordered(t: &DataType) -> bool {
    ordered_by(t, |t| matches!(t, DataType::Array(_)))
}

/// [ISO/IEC 9075-2:2023 §4.1] Whether `t` is row-ordered.
pub(crate) fn row_ordered(t: &DataType) -> bool {
    ordered_by(t, |t| matches!(t, DataType::Row(_)))
}

/// How deep arrays and rows nest in `t`: 0 of a predefined type.
pub(crate) fn nesting(t: &DataType) -> usize {
    match t {
        DataType::Array(a) => nesting(&a.element).saturating_add(1),
        DataType::Row(r) => r.fields.iter().map(|f| nesting(&f.data_type)).max().unwrap_or(0).saturating_add(1),
        DataType::Distinct(d) => nesting(&d.source),
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Extension(_) => 0,
    }
}

/// Kleene's AND and OR of truth values, none unknown.
fn and(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        (None | Some(true), None) | (None, Some(true)) => None,
    }
}

fn or(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    and(a.map(|x| !x), b.map(|x| !x)).map(|x| !x)
}

/// [ISO/IEC 9075-2:2023 §8.2 GR1b i, GR7] "x = y" of two values, rows
/// field by field, so of nested rows too; none unknown.
fn equal(x: &Value, y: &Value) -> Option<bool> {
    if let (Value::Row(a), Value::Row(b)) = (x, y) {
        return rows_equal(a, b);
    }
    compare(x, y).map(Ordering::is_eq)
}

/// [ISO/IEC 9075-2:2023 §8.2 GR7] "x < y" of two values: of rows, true if
/// the fields are equal up to one less, so of nested rows too.
fn less(x: &Value, y: &Value) -> Option<bool> {
    if let (Value::Row(a), Value::Row(b)) = (x, y) {
        return rows_less(a, b);
    }
    compare(x, y).map(Ordering::is_lt)
}

/// [ISO/IEC 9075-2:2023 §8.2 GR1b i] Whether rows `xs` and `ys` are
/// equal: the AND of their fields' equality, so a pair known unequal is
/// False though another is Unknown.
fn rows_equal(xs: &[Value], ys: &[Value]) -> Option<bool> {
    if xs.len() != ys.len() {
        return Some(false);
    }
    let mut all = Some(true);
    for (x, y) in xs.iter().zip(ys) {
        all = and(all, equal(x, y));
    }
    all
}

fn rows_less(xs: &[Value], ys: &[Value]) -> Option<bool> {
    let (mut any, mut before) = (Some(false), Some(true));
    for (x, y) in xs.iter().zip(ys) {
        any = or(any, and(before, less(x, y)));
        before = and(before, equal(x, y));
    }
    any
}

/// [ISO/IEC 9075-2:2023 §8.2 GR7] "Rx op Ry", rows of values of one
/// degree, each field a row compared so in turn: = and <> as each pair is
/// equal; < and > as the pairs are equal up to one less or greater; <= and
/// >= the negation of > and <; none unknown.
pub(crate) fn rows_hold(xs: &[Value], op: CompOp, ys: &[Value]) -> Option<bool> {
    let not = |t: Option<bool>| t.map(|b| !b);
    match op {
        CompOp::Eq => rows_equal(xs, ys),
        CompOp::Ne => not(rows_equal(xs, ys)),
        CompOp::Lt => rows_less(xs, ys),
        CompOp::Gt => rows_less(ys, xs),
        CompOp::Le => not(rows_less(ys, xs)),
        CompOp::Ge => not(rows_less(xs, ys)),
    }
}

/// The bucket of parts, shared by every value whose parts compare equal
/// with them: none if a part is null, as such a value equals nothing.
pub(crate) fn bucket_parts(parts: &[Value]) -> Option<Bucket> {
    parts.iter().map(crate::value::bucket).collect::<Option<Vec<_>>>().map(|b| Bucket::Composite(b.into()))
}

/// The form `Value::norm` gives parts: each part's.
pub(crate) fn norm_parts(parts: &[Value]) -> NormKey {
    NormKey::Composite(parts.iter().map(Value::norm).collect())
}

/// `parts` written as `<literal>`s, comma separated.
pub(crate) fn literals(parts: &[Value]) -> String {
    parts.iter().map(Value::to_literal).collect::<Vec<_>>().join(", ")
}

/// Appends `parts` to `out`: their count, then each.
pub(crate) fn put_parts(parts: &[Value], out: &mut Vec<u8>) {
    out.put_u32(u32::try_from(parts.len()).unwrap_or(u32::MAX));
    for p in parts {
        codec::put_value(out, p);
    }
}

/// Reads parts `put_parts` wrote, nested within what `r` reads.
///
/// # Errors
///
/// SQLCODE -916 if they are not parts, or nested too deep.
pub(crate) fn read_parts(r: &mut Reader<'_>) -> Result<Arc<[Value]>> {
    r.nested(|r| {
        let count = r.u32()?;
        // Pushed one by one, so a corrupt count fails at the first part
        // missing.
        let mut parts = Vec::new();
        for _ in 0..count {
            parts.push(codec::value(r)?);
        }
        Ok(parts.into())
    })
}

/// The record bytes of `parts`, the part at each position of the type
/// `types` gives it.
///
/// # Errors
///
/// SQLCODE -910 if a part is not a value of its type.
pub(crate) fn encode_parts<'t>(parts: &[Value], mut types: impl FnMut(usize) -> Option<&'t DataType>, t: &DataType) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.put_u32(u32::try_from(parts.len()).map_err(|_| mismatch(&Value::Null, t))?);
    for (i, p) in parts.iter().enumerate() {
        if p.is_null() {
            out.put_u8(ABSENT);
            continue;
        }
        let part_type = types(i).ok_or_else(|| mismatch(p, t))?;
        out.put_u8(PRESENT);
        out.put_bytes(&part_type.encode_record(p)?);
    }
    Ok(out)
}

/// The parts whose record bytes are `field`, each of the type `types`
/// gives its position; none if they are not such parts.
pub(crate) fn decode_parts<'t>(field: &[u8], mut types: impl FnMut(usize) -> Option<&'t DataType>) -> Option<Arc<[Value]>> {
    let mut r = Reader::new(field, "record");
    let count = usize::try_from(r.u32().ok()?).ok()?;
    let mut parts = Vec::new();
    for i in 0..count {
        let part = match r.u8().ok()? {
            ABSENT => Value::Null,
            PRESENT => types(i)?.decode_record(r.bytes().ok()?)?,
            _ => return None,
        };
        parts.push(part);
    }
    r.finish().ok()?;
    Some(parts.into())
}
