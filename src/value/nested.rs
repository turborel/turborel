//! \[DESIGN\] RULES.md D79, D86: the large objects and XML values inside an
//! array, a multiset, a row or a value of a structured type, kept in the
//! record of the row holding that value, and logged with it.
//!
//! A table keeps large objects of its own for its own columns alone
//! (`engine/rows/lobs.rs`), so it never names one inside another value: an
//! id there would be another table's, or none once its row is gone. So,
//! before a row is written, each such part is read whole into its value: an
//! XML value's stored form, up to `xml::MAX_STORED` octets, as an XML value
//! is read whole (D86); a large object up to a chunk, as what is held whole
//! is (D79). A part past either is refused, SQLCODE -900, and the row is
//! not written.

use std::sync::Arc;

use super::Value;
use super::lob::{CHUNK_OCTETS, Lob, LobKind};
use super::types::{MAX_TYPE_NESTING, Structured};
use super::xml::XmlValue;
use crate::error::{Code, Result, SqlError};
use crate::xml::MAX_STORED;

/// How many values may enclose a value walked (R-CF-6): as many as types
/// nest, so each value of a column's type is walked whole.
const MAX_DEPTH: usize = MAX_TYPE_NESTING;

/// The parts of `v`, if it is made of values: an array's or a multiset's
/// elements, a row's fields, a structured value's attributes.
fn parts_of(v: &Value) -> Option<&[Value]> {
    match v {
        Value::Array(parts) | Value::Multiset(parts) | Value::Row(parts) => Some(parts),
        Value::Structured(s) => Some(&s.attributes),
        Value::Null
        | Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Reference(_)
        | Value::Xml(_)
        | Value::Extension(_) => None,
    }
}

/// Whether `v`, inside `depth` values, holds a large object or an XML
/// value, or is a value made of values deeper than `MAX_DEPTH`, which
/// `inlined` refuses.
fn holds(v: &Value, depth: usize) -> bool {
    if matches!(v, Value::Lob(_) | Value::Xml(_)) {
        return true;
    }
    let Some(parts) = parts_of(v) else { return false };
    depth >= MAX_DEPTH || parts.iter().any(|p| holds(p, depth.saturating_add(1)))
}

/// The error of a part beyond what a record holds of one: `what`, held
/// in a row at most `most`.
fn beyond(what: &str, most: &str) -> SqlError {
    SqlError::new(
        Code::Unsupported,
        format!(
            "[DESIGN] {what} inside an array, a multiset, a row or a structured value is beyond turborel's limits: its row holds one of at most {most} (RULES.md D79, D86)"
        ),
    )
}

/// `l` held inline, read whole if a source holds it: at most a chunk.
fn lob_inline(l: &Lob) -> Result<Value> {
    let most = u64::try_from(CHUNK_OCTETS).unwrap_or(u64::MAX);
    if l.octets() > most {
        return Err(beyond(&format!("a large object of {} octets", l.octets()), &format!("{CHUNK_OCTETS} octets")));
    }
    if l.inline().is_some() {
        return Ok(Value::Lob(l.clone()));
    }
    let bytes = l.bounded(most)?.unwrap_or_default();
    match l.kind() {
        LobKind::Binary => Ok(Value::Lob(Lob::of_octets(&bytes))),
        LobKind::Character => {
            let text =
                String::from_utf8(bytes).map_err(|_| SqlError::new(Code::Corrupt, "[DESIGN] a character large object is not UTF-8"))?;
            Ok(Value::Lob(Lob::of_text(&text)))
        }
    }
}

/// `x` as its stored form held inline, read whole if a source holds it: at
/// most `MAX_STORED` octets.
fn xml_inline(x: &XmlValue) -> Result<Value> {
    let lob = match x.stored_lob() {
        Some(l) => l.clone(),
        None => x.lob()?,
    };
    if lob.inline().is_some() {
        return Ok(Value::Xml(XmlValue::stored(lob)));
    }
    let most = u64::try_from(MAX_STORED).unwrap_or(u64::MAX);
    let bytes = lob.bounded(most)?.ok_or_else(|| beyond("an XML value", &format!("{MAX_STORED} octets stored")))?;
    Ok(Value::Xml(XmlValue::stored(Lob::of_octets(&bytes))))
}

/// `parts`, each that holds a large object or an XML value inlined, inside
/// `depth` values.
fn each(parts: &[Value], depth: usize) -> Result<Arc<[Value]>> {
    parts.iter().map(|p| if holds(p, depth) { inlined(p, depth) } else { Ok(p.clone()) }).collect()
}

/// `v`, inside `depth` values, with each large object and XML value in it
/// held inline.
///
/// # Errors
///
/// SQLCODE -900 if one is beyond what a record holds of one, or values
/// are nested deeper than `MAX_DEPTH`; the errors of reading one.
fn inlined(v: &Value, depth: usize) -> Result<Value> {
    if let Value::Lob(l) = v {
        return lob_inline(l);
    }
    if let Value::Xml(x) = v {
        return xml_inline(x);
    }
    if depth >= MAX_DEPTH && parts_of(v).is_some() {
        return Err(beyond("a value made of values", &format!("{MAX_DEPTH} levels")));
    }
    let inner = depth.saturating_add(1);
    Ok(match v {
        Value::Array(parts) => Value::Array(each(parts, inner)?),
        Value::Multiset(parts) => Value::Multiset(each(parts, inner)?),
        Value::Row(parts) => Value::Row(each(parts, inner)?),
        Value::Structured(s) => Value::Structured(Arc::new(Structured { attributes: each(&s.attributes, inner)?, ..(**s).clone() })),
        Value::Null
        | Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Reference(_)
        | Value::Xml(_)
        | Value::Extension(_) => v.clone(),
    })
}

impl Value {
    /// \[DESIGN\] Whether a table's write of it does more than copy it: a
    /// large object or an XML value, which a table may keep apart (RULES.md
    /// D79, D86), or a value made of values holding one, read into it
    /// (`hold_parts_inline`).
    #[must_use]
    pub fn needs_keeping(&self) -> bool {
        self.is_lob_stored() || (parts_of(self).is_some() && holds(self, 0))
    }
}

/// \[DESIGN\] Holds inline each large object and XML value inside a value
/// of `row` made of values, so its row's record and log record hold them
/// whole (RULES.md D79, D86).
///
/// # Errors
///
/// SQLCODE -900 if one is beyond what a record holds of one: a large
/// object of more than a chunk, an XML value stored in more than
/// `MAX_STORED` octets; the errors of reading one.
pub(crate) fn hold_parts_inline(row: &mut [Value]) -> Result<()> {
    for v in row.iter_mut() {
        if parts_of(v).is_some() && holds(v, 0) {
            *v = inlined(v, 0)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
