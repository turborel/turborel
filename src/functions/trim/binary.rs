//! [ISO/IEC 9075-2:2023 §6.33] `<binary trim function>` (Feature T022):
//! TRIM of a binary string, BINARY, BINARY VARYING or BINARY LARGE OBJECT,
//! removing the octets equal to its trim octet, X'00' if none is written
//! \[SR22a\], from the ends its specification names \[GR18\]. Of a large
//! object, the octets are counted a chunk at a time and the result is a
//! slice of it (RULES.md D79).

use super::Ends;
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::functions::lobs::ends::{leading_octets, trailing_octets};
use crate::value::lob::{Lob, LobKind};
use crate::value::{Binary, Value};

/// [ISO/IEC 9075-2:2023 §6.33 SR22a] The trim octet none is written gives.
const ZERO: u8 = 0;

/// Whether `v` is a binary string.
fn binary(v: &Value) -> bool {
    match v {
        Value::Binary(_) => true,
        Value::Lob(l) => l.kind() == LobKind::Binary,
        Value::Null
        | Value::Char(_)
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
        | Value::Xml(_) => false,
    }
}

/// \[GR18d\] The trim octet of a trim of `source`, `so` if written, neither
/// null; none if `source` is no binary string.
///
/// # Errors
///
/// SQLSTATE 22027 if `so` is not of one octet; SQLCODE -910 if it is no
/// binary string.
pub(super) fn trim_octet(source: &Value, so: Option<&Value>) -> Result<Option<u8>> {
    if !binary(source) {
        return Ok(None);
    }
    let octets = match so {
        None => return Ok(Some(ZERO)),
        Some(Value::Binary(b)) => b.bytes().to_vec(),
        // Only the octets of one of a single octet are read.
        Some(Value::Lob(l)) => {
            if l.octets() == 1 {
                l.reader().read(0, 1)?
            } else {
                Vec::new()
            }
        }
        Some(
            Value::Null
            | Value::Char(_)
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
        ) => return fail(Code::Api, "[DESIGN] a binary trim was given a trim octet not a binary string"),
    };
    match octets.as_slice() {
        [octet] => Ok(Some(*octet)),
        [] | [_, _, ..] => fail_with(Code::Data, SqlState::TRIM_ERROR, "[ISO/IEC 9075-2:2023 §6.33 GR18d] the trim octet is not one octet"),
    }
}

/// How many octets at the start and end of `bytes` are `octet`, by `ends`,
/// the end's no more than those left after the start's.
fn counts(bytes: &[u8], octet: u8, ends: Ends) -> (usize, usize) {
    let lead = if matches!(ends, Ends::Leading | Ends::Both) { bytes.iter().take_while(|&&b| b == octet).count() } else { 0 };
    let rest = bytes.get(lead..).unwrap_or_default();
    let trail = if matches!(ends, Ends::Trailing | Ends::Both) { rest.iter().rev().take_while(|&&b| b == octet).count() } else { 0 };
    (lead, trail)
}

/// \[GR18e\] `lob`, a binary large object, with the octets equal to `octet`
/// removed from `ends`: a slice of it.
fn trimmed_lob(lob: &Lob, octet: u8, ends: Ends) -> Result<Lob> {
    let n = lob.octets();
    let lead = if matches!(ends, Ends::Leading | Ends::Both) { leading_octets(lob, octet)? } else { 0 };
    let trail = if matches!(ends, Ends::Trailing | Ends::Both) { trailing_octets(lob, octet, n.saturating_sub(lead))? } else { 0 };
    Ok(lob.slice_octets(lead, n.saturating_sub(lead).saturating_sub(trail)))
}

/// \[GR18e\] `source`, a binary string, not null, with the octets equal to
/// `octet` removed from `ends`.
///
/// # Errors
///
/// As reading a chunk of a large object fails.
pub(super) fn trimmed(source: &Value, octet: u8, ends: Ends) -> Result<Value> {
    match source {
        Value::Binary(b) => {
            let (lead, trail) = counts(b.bytes(), octet, ends);
            let kept = b.bytes().get(lead..b.bytes().len().saturating_sub(trail)).unwrap_or_default();
            Ok(Value::Binary(Binary::new(kept.to_vec())))
        }
        Value::Lob(l) => Ok(Value::Lob(trimmed_lob(l, octet, ends)?)),
        Value::Null
        | Value::Char(_)
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
        | Value::Xml(_) => Ok(Value::Null),
    }
}
