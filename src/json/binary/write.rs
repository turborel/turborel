//! \[DESIGN\] An SQL/JSON item written as a document of the stored form
//! (`json/binary.rs`): each node after its tag, an array's and an object's
//! offsets reserved, then filled once their elements or members are
//! written.

use super::{
    APPROXIMATE, ARRAY, DATETIME, DECFLOAT, DECIMAL, FALSE, FORMAT_VERSION, INTEGER, MAX_DOCUMENT_OCTETS, NULL, OBJECT, STRING, TRUE, WIDE,
    WORD, corrupt, too_long, within,
};
use crate::error::Result;
use crate::json::{Item, Member};
use crate::value::datetime::Moment;
use crate::value::{Datetime, Decimal, Value};

/// The document of `item`.
///
/// # Errors
///
/// SQLCODE -900 if arrays and objects nest deeper than `MAX_NESTING`, or
/// the document would be longer than `MAX_DOCUMENT_OCTETS`.
pub fn encode(item: &Item) -> Result<Vec<u8>> {
    let mut out = vec![FORMAT_VERSION];
    write_node(item, &mut out, 0)?;
    if out.len() > MAX_DOCUMENT_OCTETS {
        return Err(too_long());
    }
    Ok(out)
}

/// `item`, within `depth` arrays and objects, appended to `out`.
fn write_node(item: &Item, out: &mut Vec<u8>, depth: usize) -> Result<()> {
    within(depth)?;
    match item {
        Item::Null => out.push(NULL),
        Item::Scalar(scalar) => write_scalar(scalar.value(), out)?,
        Item::Array(elements) => write_array(elements, out, depth.saturating_add(1))?,
        Item::Object(members) => write_object(members, out, depth.saturating_add(1))?,
    }
    Ok(())
}

/// A count or a length as a u32.
///
/// # Errors
///
/// SQLCODE -900 if it is past a u32, so past `MAX_DOCUMENT_OCTETS`.
fn word(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|_| too_long())
}

/// Appends `n` as a little-endian u32.
fn put_word(out: &mut Vec<u8>, n: usize) -> Result<()> {
    out.extend_from_slice(&word(n)?.to_le_bytes());
    Ok(())
}

/// Writes `n` as a little-endian u32 at `at` of `out`, reserved before.
fn patch_word(out: &mut [u8], at: usize, n: usize) -> Result<()> {
    let bytes = word(n)?.to_le_bytes();
    let end = at.checked_add(WORD).ok_or_else(too_long)?;
    let slot = out.get_mut(at..end).ok_or_else(|| corrupt("an offset was not reserved"))?;
    slot.copy_from_slice(&bytes);
    Ok(())
}

/// Appends the string `s`: its length, then its octets.
fn put_string(out: &mut Vec<u8>, s: &str) -> Result<()> {
    put_word(out, s.len())?;
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

/// The node of the SQL/JSON scalar `value` (`Scalar::new`'s kinds).
fn write_scalar(value: &Value, out: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Bool(truth) => out.push(if *truth { TRUE } else { FALSE }),
        Value::Exact(d) => write_exact(*d, out),
        Value::DecFloat(d) => {
            out.push(DECFLOAT);
            out.extend_from_slice(&d.coefficient().to_le_bytes());
            out.extend_from_slice(&d.exponent().to_le_bytes());
        }
        Value::Approx(x) => {
            out.push(APPROXIMATE);
            out.extend_from_slice(&x.to_bits().to_le_bytes());
        }
        Value::Char(s) => {
            out.push(STRING);
            put_string(out, s)?;
        }
        Value::Datetime(d) => write_datetime(d, out),
        // [§4.48.3] No SQL/JSON scalar is of another type (`Scalar::new`).
        Value::Null
        | Value::Binary(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Extension(_) => return Err(corrupt("an SQL/JSON scalar is of no scalar's type")),
    }
    Ok(())
}

/// The node of the exact number `d`: the narrowest of the three forms
/// that holds it.
fn write_exact(d: Decimal, out: &mut Vec<u8>) {
    let narrow = i64::try_from(d.units).ok();
    let scale = u8::try_from(d.scale).ok();
    match (narrow, scale) {
        (Some(units), Some(0)) => {
            out.push(INTEGER);
            out.extend_from_slice(&units.to_le_bytes());
        }
        (Some(units), Some(scale)) => {
            out.push(DECIMAL);
            out.extend_from_slice(&units.to_le_bytes());
            out.push(scale);
        }
        (None, _) | (_, None) => {
            out.push(WIDE);
            out.extend_from_slice(&d.units.to_le_bytes());
            out.extend_from_slice(&d.scale.to_le_bytes());
        }
    }
}

/// The node of the datetime `d`.
fn write_datetime(d: &Datetime, out: &mut Vec<u8>) {
    out.push(DATETIME);
    out.push(moment_tag(d.moment));
    out.extend_from_slice(&d.utc.to_le_bytes());
    out.extend_from_slice(&d.zone.to_le_bytes());
    out.push(u8::from(d.zoned));
    out.push(d.precision);
}

/// The octet of a datetime's moment.
pub(super) fn moment_tag(moment: Moment) -> u8 {
    match moment {
        Moment::Date => 0,
        Moment::Time => 1,
        Moment::Timestamp => 2,
    }
}

/// Reserves an array's or an object's head and `count` offsets after the
/// tag `tag`: where its count is written, and where its first offset is.
fn open_container(out: &mut Vec<u8>, tag: u8, count: usize) -> Result<(usize, usize)> {
    out.push(tag);
    let head = out.len();
    put_word(out, count)?;
    put_word(out, 0)?;
    let offsets = out.len();
    let reserved = count.checked_mul(WORD).ok_or_else(too_long)?;
    out.resize(offsets.checked_add(reserved).ok_or_else(too_long)?, 0);
    Ok((head, offsets))
}

/// Writes the size of a container whose head is at `head` and whose
/// elements or members begin at `body`, now that they are written.
fn close_container(out: &mut [u8], head: usize, body: usize) -> Result<()> {
    let size = out.len().saturating_sub(body);
    patch_word(out, head.saturating_add(WORD), size)
}

/// [§4.48.3] The node of an array of `elements`.
fn write_array(elements: &[Item], out: &mut Vec<u8>, depth: usize) -> Result<()> {
    let (head, offsets) = open_container(out, ARRAY, elements.len())?;
    let body = out.len();
    for (i, element) in elements.iter().enumerate() {
        let at = out.len().saturating_sub(body);
        patch_word(out, offsets.saturating_add(i.saturating_mul(WORD)), at)?;
        write_node(element, out, depth)?;
    }
    close_container(out, head, body)
}

/// [§4.48.3] The node of an object of `members`: the members in the order
/// given, then the directory of their offsets sorted by key, and of one
/// key by that order, written into the offsets reserved.
fn write_object(members: &[Member], out: &mut Vec<u8>, depth: usize) -> Result<()> {
    let (head, offsets) = open_container(out, OBJECT, members.len())?;
    let body = out.len();
    let mut directory = Vec::with_capacity(members.len());
    for member in members {
        directory.push((member.key.as_bytes(), out.len().saturating_sub(body)));
        put_string(out, &member.key)?;
        write_node(&member.value, out, depth)?;
    }
    // A stable sort, so the members of one key stay in the order given.
    directory.sort_by(|a, b| a.0.cmp(b.0));
    for (i, (_, at)) in directory.into_iter().enumerate() {
        patch_word(out, offsets.saturating_add(i.saturating_mul(WORD)), at)?;
    }
    close_container(out, head, body)
}
