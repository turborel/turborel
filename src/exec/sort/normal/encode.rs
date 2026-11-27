//! \[DESIGN\] RULES.md D364: a row's normal key written, field by field, as
//! `normal.rs` lays it out. A batch's vector of one kind is written a
//! vector at a time, a value read where it lies; a vector of values, or a
//! row's values, a value at a time. A value the layout cannot hold, of a
//! kind not its field's or past its integers' bytes, gives no key, and the
//! sort sorts without them.
//!
//! A string longer than its field whose rest is not spaces leaves its row's
//! order to the values compared whole, if another row's field is the same,
//! so every byte after it is zero, and the keys of two such rows are equal.
//! The fields are written last to first for that.

use super::{Code, Field, Ints, Layout};
use crate::kernels::Kernels;
use crate::storage::{Batch, Cell, Column, Row};
use crate::value::{Value, pow10};

/// The bit that orders an approximate number's bits after its sign.
const SIGN: u64 = 1 << 63;

/// The byte after a string's prefix of which the rest is less than spaces.
const REST_LESS: u8 = 0;
/// The byte after a string's prefix of which the rest, if any, is spaces,
/// so the string equals its prefix padded.
const REST_EQUAL: u8 = 1;
/// The byte after a string's prefix of which the rest is greater than
/// spaces.
const REST_GREATER: u8 = 2;

/// A value as a key's field reads it.
#[derive(Clone, Copy)]
enum Val<'a> {
    /// The null value.
    Null,
    /// An integer of a kind: exact units at a scale, an instant, an
    /// interval's units.
    Int(Ints, i128, u32),
    /// An approximate number.
    Approx(f64),
    /// A character string's bytes.
    Text(&'a [u8]),
    /// A binary string's octets.
    Binary(&'a [u8]),
    /// A truth value.
    Bool(bool),
}

/// Writes into `keys`, a key of `layout.width` bytes after another, the
/// keys of `batch`'s rows at `rows`, in order; none if a value cannot be.
/// A vector of integers of none null, of the field's scale, is written by
/// `kernels` at once (`Kernels::normalize_i64`, RULES.md D380).
pub(in crate::exec::sort) fn encode_batch(
    (layout, kernels): (&Layout, &dyn Kernels),
    batch: &Batch,
    rows: &[usize],
    keys: &mut [u8],
) -> Option<()> {
    if layout.width == 0 {
        return Some(());
    }
    for field in layout.fields.iter().rev() {
        if let Some(ints) = batch.columns.get(field.column).and_then(|c| at_once(c, field)) {
            put_ints(kernels, (layout.width, field), ints, rows, keys)?;
            continue;
        }
        let slots = keys.chunks_exact_mut(layout.width).zip(rows);
        match batch.columns.get(field.column)? {
            Column::Ints(ints) => {
                for (key, &r) in slots {
                    let unit = ints.units.get(r).filter(|_| !ints.is_null(r));
                    put(key, field, unit.map_or(Val::Null, |&u| Val::Int(Ints::Exact, i128::from(u), ints.scale)))?;
                }
            }
            Column::Floats(floats) => {
                for (key, &r) in slots {
                    put(key, field, floats.get(r).map_or(Val::Null, Val::Approx))?;
                }
            }
            Column::Texts(texts) => {
                for (key, &r) in slots {
                    put(key, field, texts.get(r).map_or(Val::Null, |t| Val::Text(t.as_bytes())))?;
                }
            }
            Column::Datetimes(d) => {
                for (key, &r) in slots {
                    let utc = d.utc.get(r).filter(|_| !d.is_null(r));
                    put(key, field, utc.map_or(Val::Null, |&u| Val::Int(Ints::Instant(d.moment), i128::from(u), 0)))?;
                }
            }
            Column::Bools(_) | Column::Values(_) | Column::Nulls(_) | Column::Late(_) => {
                for (key, &r) in slots {
                    put(key, field, val_of(batch.cell(field.column, r))?)?;
                }
            }
        }
    }
    Some(())
}

/// A vector's integers and the base, bytes and inversion
/// `Kernels::normalize_i64` writes them by.
type AtOnce<'a> = (&'a [i64], (i64, usize, bool));

/// Writes `field` of `keys`, each `width` bytes, of the rows at `rows` of
/// `units`, none null, by `kernels`, and its null byte if it has one.
fn put_ints(
    kernels: &dyn Kernels,
    (width, field): (usize, &Field),
    (units, shape): AtOnce<'_>,
    rows: &[usize],
    keys: &mut [u8],
) -> Option<()> {
    let places: Vec<u32> = rows.iter().map(|&r| u32::try_from(r).unwrap_or(u32::MAX)).collect();
    let value_at = field.at.checked_add(usize::from(field.nullable))?;
    kernels.normalize_i64((units, &places), shape, (width, value_at), keys)?;
    if field.nullable {
        // Of no null, each null byte is a value's [ISO/IEC 9075-2:2023 §10.10 GR1e ii].
        let byte = u8::from(field.order.nulls_first);
        keys.chunks_exact_mut(width).take(rows.len()).try_for_each(|key| key.get_mut(field.at).map(|b| *b = byte))?;
    }
    Some(())
}

/// Of `column`, a vector of integers of none null, of the scale of `field`
/// of exact numbers, its units and the base, bytes and inversion
/// `Kernels::normalize_i64` writes them by; none of another.
fn at_once<'a>(column: &'a Column, field: &Field) -> Option<AtOnce<'a>> {
    let (Column::Ints(ints), Code::Int { scale, base, bytes }) = (column, field.code) else { return None };
    let fits = ints.nulls.is_none() && ints.scale == scale && field.ints == Ints::Exact && bytes <= size_of::<u64>();
    fits.then_some(())?;
    Some((&ints.units, (i64::try_from(base).ok()?, bytes, field.order.descending)))
}

/// Writes into `key`, of `layout.width` bytes, the key of `row`; none if
/// a value cannot be.
pub(in crate::exec::sort) fn encode_row(layout: &Layout, row: &Row, key: &mut [u8]) -> Option<()> {
    for field in layout.fields.iter().rev() {
        put(key, field, val_of(Cell::Value(row.get(field.column)?))?)?;
    }
    Some(())
}

/// A cell as a field reads it; none of a value of no normal form.
fn val_of(cell: Cell<'_>) -> Option<Val<'_>> {
    Some(match cell {
        Cell::Null => Val::Null,
        Cell::Exact(units, scale) => Val::Int(Ints::Exact, i128::from(units), scale),
        Cell::Approx(f) => Val::Approx(f),
        Cell::Text(t) => Val::Text(t.as_bytes()),
        Cell::Datetime(d) => Val::Int(Ints::Instant(d.moment), i128::from(d.utc), 0),
        Cell::Bool(b) => Val::Bool(b),
        Cell::Value(v) => match v {
            Value::Null => Val::Null,
            Value::Exact(d) => Val::Int(Ints::Exact, d.units, d.scale),
            Value::Approx(f) => Val::Approx(*f),
            Value::Char(s) => Val::Text(s.as_bytes()),
            Value::Binary(b) => Val::Binary(b.bytes()),
            Value::Datetime(d) => Val::Int(Ints::Instant(d.moment), i128::from(d.utc), 0),
            Value::Interval(i) => Val::Int(Ints::Interval(i.qualifier.year_month()), i.units, 0),
            Value::Bool(b) => Val::Bool(*b),
            Value::DecFloat(_)
            | Value::Lob(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Json(_)
            | Value::Extension(_)
            | Value::Xml(_) => return None,
        },
    })
}

/// Writes `val` as `field` of `key`, every byte after it zero if the
/// value is longer than the field and not equal to what it holds; none if
/// it cannot be.
fn put(key: &mut [u8], field: &Field, val: Val<'_>) -> Option<()> {
    let end = field.at.checked_add(field.width())?;
    let (head, tail) = key.split_at_mut_checked(end)?;
    if put_field(head.get_mut(field.at..)?, field, val)? {
        tail.fill(0);
    }
    Some(())
}

/// Writes `val` as `field` into `slot`, its bytes: whether the value is
/// longer than them and not equal to what they hold; none if it cannot be.
fn put_field(slot: &mut [u8], field: &Field, val: Val<'_>) -> Option<bool> {
    let part = if field.nullable {
        let (head, rest) = slot.split_first_mut()?;
        let null = matches!(val, Val::Null);
        // A null's byte comes first under NULLS FIRST, last under NULLS
        // LAST, whatever the direction [ISO/IEC 9075-2:2023 §10.10 GR1e ii].
        *head = u8::from(null != field.order.nulls_first);
        if null {
            rest.fill(0);
            return Some(false);
        }
        rest
    } else {
        slot
    };
    let cut = value(part, field, val)?;
    if field.order.descending {
        for b in part.iter_mut() {
            *b = !*b;
        }
    }
    Some(cut)
}

/// Writes `val`, not null, into `part`, as `field`'s code says: whether it
/// is longer than the part holds and not equal to what it holds.
fn value(part: &mut [u8], field: &Field, val: Val<'_>) -> Option<bool> {
    match (val, field.code) {
        (Val::Int(kind, units, scale), Code::Int { scale: to, base, bytes }) => {
            let at = units.checked_mul(pow10(to.checked_sub(scale)?)?)?;
            if at < base || kind != field.ints {
                return None;
            }
            // At or above the base, the difference fits 128 bits unsigned.
            let offset = at.wrapping_sub(base).cast_unsigned();
            let be = offset.to_be_bytes();
            let skip = be.len().checked_sub(bytes)?;
            // The bytes left off must be zero, or the value is past the field.
            let (high, low) = be.split_at_checked(skip)?;
            if high.iter().any(|&b| b != 0) {
                return None;
            }
            part.copy_from_slice(low);
        }
        (Val::Approx(f), Code::Approx) => {
            if f.is_nan() {
                return None;
            }
            let bits = if f == 0.0_f64 { 0.0_f64.to_bits() } else { f.to_bits() };
            part.copy_from_slice(&(if bits & SIGN == 0 { bits | SIGN } else { !bits }).to_be_bytes());
        }
        (Val::Text(text), Code::Text { prefix }) => return text_into(part, text, prefix),
        (Val::Binary(octets), Code::Binary { prefix }) => return binary_into(part, octets, prefix),
        // A character string of a key by octets, written as a binary
        // string of its octets (RULES.md D1185); of any other key it is
        // not its field's kind.
        (Val::Text(text), Code::Binary { prefix }) => return if field.octets { binary_into(part, text, prefix) } else { None },
        (Val::Bool(b), Code::Bool) => part.fill(u8::from(b)),
        (
            Val::Null | Val::Int(..) | Val::Approx(_) | Val::Text(_) | Val::Binary(_) | Val::Bool(_),
            Code::Nothing | Code::Int { .. } | Code::Approx | Code::Text { .. } | Code::Binary { .. } | Code::Bool,
        ) => return None,
    }
    Some(false)
}

/// Writes `text`'s first `prefix` bytes into `part`, padded with spaces,
/// and then a byte of what is past them: compared with spaces, as a
/// comparison pads the shorter string [X3.135-1986 §5.11 GR3], so two
/// strings whose bytes differ are in their order, and two of equal bytes
/// both `REST_EQUAL` are equal. UTF-8's bytes are in its characters' order.
/// Whether its rest is not spaces.
fn text_into(part: &mut [u8], text: &[u8], prefix: usize) -> Option<bool> {
    let (head, last) = part.split_at_mut_checked(prefix)?;
    head.fill(b' ');
    head.iter_mut().zip(text).for_each(|(to, from)| *to = *from);
    let rest = text.get(prefix..).unwrap_or_default();
    let beyond = rest.iter().find(|&&b| b != b' ').map_or(REST_EQUAL, |&b| if b < b' ' { REST_LESS } else { REST_GREATER });
    last.fill(beyond);
    Some(beyond != REST_EQUAL)
}

/// Writes `octets`' first `prefix` into `part`, padded with zeros, and
/// then their length, or one more than `prefix` if longer. Binary strings
/// compare octet by octet, then by length [ISO/IEC 9075-2:2023 §8.2 GR5],
/// and of two of equal bytes the shorter is the start of the longer.
/// Whether they are longer than `prefix`.
fn binary_into(part: &mut [u8], octets: &[u8], prefix: usize) -> Option<bool> {
    let (head, last) = part.split_at_mut_checked(prefix)?;
    head.fill(0);
    head.iter_mut().zip(octets).for_each(|(to, from)| *to = *from);
    last.fill(u8::try_from(octets.len().min(prefix.checked_add(1)?)).ok()?);
    Some(octets.len() > prefix)
}
