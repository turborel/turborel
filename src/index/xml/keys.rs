//! \[DESIGN\] RULES.md D106: the byte forms of an XML index's keys, each
//! comparing as bytes as what it stands for compares, so one B-link tree
//! holds a dictionary, the markers of the values it holds, and its
//! entries, each a space of keys beginning with its own byte.
//!
//! ```text
//! ordinal:  an unsigned number, 1 or more, as its length in octets, then
//!           its octets, big-endian, leading zeros dropped; 0 (one octet)
//!           ends a list of them
//! string:   its octets, 00 written 00 FF, then 00 01 if whole or 00 02 if
//!           cut
//! value:    1 (unordered) | 2 (number) xs:double, its sign bit flipped,
//!           or every bit if negative, big-endian | 3 (string) string
//! path:     1 (attribute) or 2 (element), then the ordinals of the
//!           tokens of its name and of each element above it to its item
//! ORDPATH:  the ordinals of its components, then 0
//! ```

use std::ops::Bound;

use super::Method;
use crate::error::{Code, Result, SqlError};
use crate::storage::RowId;
use crate::xml::{Atomic, AtomicType, cast};

// The spaces of an index's tree, by their first byte.
/// The dictionary's names: an expanded name to its token.
pub(super) const NAMES: u8 = 1;
/// The dictionary's paths: a path to its id.
pub(super) const PATHS: u8 = 2;
/// The dictionary's paths by id.
pub(super) const PATH_IDS: u8 = 3;
/// The dictionary's counters of ids given.
pub(super) const COUNTERS: u8 = 4;
/// The markers of the (value, row) pairs held.
pub(super) const MARKS: u8 = 5;
/// The entries of values held as large objects, by object and row.
pub(super) const DOCUMENTS: u8 = 6;
/// The index's entries, as its method orders them.
pub(super) const ENTRIES: u8 = 7;
/// The primary's values of each row's versions, by row.
pub(super) const VERSIONS: u8 = 8;
/// The primary's nodes by row, path and value [Pal et al. §3.2.2].
pub(super) const PATHED: u8 = 9;

// The classes of value, in their order.
/// A number that is no number: its node's value does not cast to one, is
/// cut, or is NaN.
pub(super) const UNORDERED: u8 = 1;
/// A node's value cast to xs:double.
pub(super) const NUMBER: u8 = 2;
/// A node's value as a string.
pub(super) const STRING: u8 = 3;

// The kinds of node a path ends at.
/// An attribute.
pub(super) const ATTRIBUTE: u8 = 1;
/// An element.
pub(super) const ELEMENT: u8 = 2;

/// The octet that ends a list of ordinals.
const END: u8 = 0;
/// After 00, the escaped octet 00.
const ESCAPED: u8 = 0xFF;
/// After 00, the end of a whole string.
const WHOLE: u8 = 1;
/// After 00, the end of a cut string.
const CUT: u8 = 2;
/// Bytes of a row id in a key.
pub(super) const ROW: usize = 8;
/// Bytes of an xs:double in a key.
const DOUBLE: usize = 8;
/// The most octets of an ordinal: a u64's, after its length.
pub(super) const MOST_ORDINAL: usize = 9;
/// The octets an entry takes besides its value and ORDPATH, at most: its
/// space, its path's ordinal, its row and its ORDPATH's end.
pub(super) const ENTRY_OVERHEAD: usize = 1 + MOST_ORDINAL + ROW + 1;
/// The sign bit of an xs:double's bits.
const SIGN: u64 = 0x8000_0000_0000_0000;

/// The error of a key that is no key of this format.
pub(super) fn corrupt() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] an XML index's key is malformed (RULES.md D106)")
}

/// Appends ordinal `n`.
pub(super) fn put_ordinal(out: &mut Vec<u8>, n: u64) {
    let bytes = n.to_be_bytes();
    let skip = bytes.iter().take_while(|&&b| b == 0).count();
    let kept = bytes.get(skip..).unwrap_or_default();
    out.push(u8::try_from(kept.len()).unwrap_or(0));
    out.extend_from_slice(kept);
}

/// The ordinal `n` alone.
pub(super) fn ordinal_of(n: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(MOST_ORDINAL);
    put_ordinal(&mut out, n);
    out
}

/// Appends `bytes` escaped, then the end of a string whole or cut.
pub(super) fn put_string(out: &mut Vec<u8>, bytes: &[u8], whole: bool) {
    for &b in bytes {
        out.push(b);
        if b == 0 {
            out.push(ESCAPED);
        }
    }
    out.extend_from_slice(&[0, if whole { WHOLE } else { CUT }]);
}

/// The value key of a string compared by code points [XQueryFO §7.3.1]:
/// its first `prefix` octets, cut there if longer, so keys
/// order as the strings they key, ties of strings that a cut makes alike
/// apart.
pub(super) fn string_value(text: &[u8], prefix: usize) -> Vec<u8> {
    let cut = text.get(..prefix).unwrap_or(text);
    let mut out = vec![STRING];
    put_string(&mut out, cut, cut.len() == text.len());
    out
}

/// The value key of `number`, ordered as numbers are; -0 as 0, which it
/// equals [XQueryFO §6.3]; NaN, which nothing equals, unordered.
pub(super) fn number_value(number: f64) -> Vec<u8> {
    if number.is_nan() {
        return vec![UNORDERED];
    }
    let bits = number.to_bits();
    // Of -0, the bits of 0.
    let bits = if bits & !SIGN == 0 { 0 } else { bits };
    let flipped = if bits & SIGN == 0 { bits | SIGN } else { !bits };
    let mut out = vec![NUMBER];
    out.extend_from_slice(&flipped.to_be_bytes());
    out
}

/// \[XQuery §3.5.2\] The xs:double a node's typed value, an
/// `xdt:untypedAtomic` of `text`, casts to [XQueryFO §17.1.1], if `whole`
/// and it casts; none if it does not, as a numeric comparison of it
/// raises err:FORG0001.
pub(super) fn number_of(text: &[u8], whole: bool) -> Option<f64> {
    let text = std::str::from_utf8(text).ok().filter(|_| whole)?;
    let cast = cast(&Atomic::new(AtomicType::UntypedAtomic, text), AtomicType::Double).ok()?;
    match &*cast.lexical {
        "INF" => Some(f64::INFINITY),
        "-INF" => Some(f64::NEG_INFINITY),
        lexical => lexical.parse::<f64>().ok(),
    }
}

/// The two value keys of a node whose string value begins `text`, whole
/// or not: as a string, and as a number or unordered.
pub(super) fn values_of(text: &[u8], whole: bool) -> [Vec<u8>; 2] {
    let string = {
        let mut out = vec![STRING];
        put_string(&mut out, text, whole);
        out
    };
    [string, number_of(text, whole).map_or_else(|| vec![UNORDERED], number_value)]
}

/// The text of `value`, a string value key, and whether it is whole.
///
/// # Errors
///
/// SQLCODE -916 if it is no string value key.
pub(super) fn text_of(value: &[u8]) -> Result<(Vec<u8>, bool)> {
    let mut at = Cursor { bytes: value, at: 1 };
    if value.first() != Some(&STRING) {
        return Err(corrupt());
    }
    let mut text = Vec::new();
    loop {
        let b = at.byte()?;
        if b != 0 {
            text.push(b);
            continue;
        }
        match at.byte()? {
            ESCAPED => text.push(0),
            WHOLE => return Ok((text, true)),
            CUT => return Ok((text, false)),
            _ => return Err(corrupt()),
        }
    }
}

/// A node as an index keys it: its path's id, its kind, a value key, its
/// ORDPATH's ordinals without their end.
#[derive(Debug, Clone, Copy)]
pub(super) struct Node<'a> {
    pub(super) path: u64,
    pub(super) kind: u8,
    pub(super) value: &'a [u8],
    pub(super) ordpath: &'a [u8],
}

/// The entry of `node` of `row` in an index of `method`.
pub(super) fn entry(method: Method, node: &Node<'_>, row: RowId) -> Vec<u8> {
    let mut out = Vec::with_capacity(node.value.len().saturating_add(node.ordpath.len()).saturating_add(ENTRY_OVERHEAD));
    out.push(ENTRIES);
    match method {
        Method::Path => {
            put_ordinal(&mut out, node.path);
            out.extend_from_slice(node.value);
            out.extend_from_slice(&row.to_be_bytes());
        }
        Method::Value => {
            out.extend_from_slice(node.value);
            put_ordinal(&mut out, node.path);
            out.extend_from_slice(&row.to_be_bytes());
        }
        Method::Property => {
            out.extend_from_slice(&row.to_be_bytes());
            put_ordinal(&mut out, node.path);
            out.extend_from_slice(node.value);
        }
        Method::Primary => out.extend_from_slice(&row.to_be_bytes()),
    }
    out.extend_from_slice(node.ordpath);
    out.push(END);
    // The primary's row of the Infoset [Pal et al. §3.1.1]: after its key,
    // the row and ORDPATH, its kind, its path and its value.
    if method == Method::Primary {
        out.push(node.kind);
        put_ordinal(&mut out, node.path);
        out.extend_from_slice(node.value);
    }
    out
}

/// What an entry of an index of `method` holds: its path's id, its value
/// key and its row.
pub(super) fn parse_entry(method: Method, key: &[u8]) -> Result<(u64, &[u8], RowId)> {
    let mut at = Cursor { bytes: key, at: 0 };
    if at.byte()? != ENTRIES {
        return Err(corrupt());
    }
    Ok(match method {
        Method::Path => {
            let path = at.ordinal()?;
            let value = at.value()?;
            (path, value, at.row()?)
        }
        Method::Value => {
            let value = at.value()?;
            let path = at.ordinal()?;
            (path, value, at.row()?)
        }
        Method::Property => {
            let row = at.row()?;
            let path = at.ordinal()?;
            (path, at.value()?, row)
        }
        Method::Primary => {
            let fields = primary_fields(&mut at)?;
            (fields.path, fields.value, fields.row)
        }
    })
}

/// The fields of a primary's entry, a row of the Infoset [Pal et al.
/// §3.1.1].
pub(super) struct Primary<'a> {
    pub(super) row: RowId,
    pub(super) kind: u8,
    pub(super) path: u64,
    pub(super) value: &'a [u8],
}

/// The fields of a primary's entry, after its space: its row, its kind,
/// its path's id and its value, its ORDPATH passed over.
pub(super) fn primary_fields<'a>(at: &mut Cursor<'a>) -> Result<Primary<'a>> {
    let row = at.row()?;
    at.ordpath()?;
    let kind = at.byte()?;
    let path = at.ordinal()?;
    Ok(Primary { row, kind, path, value: at.value()? })
}

/// A node as an index of any method keeps it: its path's id, its kind,
/// its value keys as a string and as a number, and its ORDPATH's ordinals
/// without their end.
#[derive(Debug, Clone, Copy)]
pub(super) struct Indexed<'a> {
    pub(super) path: u64,
    pub(super) kind: u8,
    pub(super) values: [&'a [u8]; 2],
    pub(super) ordpath: &'a [u8],
}

/// The entries of `node` of `row` in an index of `method`: a secondary
/// method's two, of its string value and of its number; the primary's
/// two, of its string value, its row of the Infoset [Pal et al. §3.1.1]
/// and that row by path and value (`pathed`).
pub(super) fn entries_of(method: Method, node: &Indexed<'_>, row: RowId) -> Vec<Vec<u8>> {
    let entry_of = |value| entry(method, &Node { path: node.path, kind: node.kind, value, ordpath: node.ordpath }, row);
    let [string, number] = node.values;
    if method == Method::Primary {
        return vec![entry_of(string), pathed(&Node { path: node.path, kind: node.kind, value: string, ordpath: node.ordpath }, row)];
    }
    vec![entry_of(string), entry_of(number)]
}

/// The primary's entry of `node` of `row` by path and value: its row, its
/// path's id, its value and its ORDPATH, so a row's nodes of a path, and
/// of a value, are found by a seek, as Pal et al.'s PROPERTY index finds
/// them [§3.2.2].
pub(super) fn pathed(node: &Node<'_>, row: RowId) -> Vec<u8> {
    let mut out = vec![PATHED];
    out.extend_from_slice(&row.to_be_bytes());
    put_ordinal(&mut out, node.path);
    out.extend_from_slice(node.value);
    out.extend_from_slice(node.ordpath);
    out.push(END);
    out
}

/// The value and the ORDPATH's ordinals of the primary's entry by path
/// and value `key`, after its row and path.
pub(super) fn pathed_fields(key: &[u8], at: usize) -> Result<(&[u8], &[u8])> {
    let mut at = Cursor { bytes: key, at };
    let value = at.value()?;
    Ok((value, at.ordpath()?))
}

/// A reader of the fields of a key.
pub(super) struct Cursor<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) at: usize,
}

impl<'a> Cursor<'a> {
    /// The next `n` octets.
    pub(super) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or_else(corrupt)?;
        let taken = self.bytes.get(self.at..end).ok_or_else(corrupt)?;
        self.at = end;
        Ok(taken)
    }

    /// The next octet.
    pub(super) fn byte(&mut self) -> Result<u8> {
        self.take(1)?.first().copied().ok_or_else(corrupt)
    }

    /// The next ordinal; 0 for the end of a list.
    pub(super) fn ordinal(&mut self) -> Result<u64> {
        let len = usize::from(self.byte()?);
        let mut bytes = [0_u8; 8];
        let at = bytes.len().checked_sub(len).ok_or_else(corrupt)?;
        bytes.get_mut(at..).ok_or_else(corrupt)?.copy_from_slice(self.take(len)?);
        Ok(u64::from_be_bytes(bytes))
    }

    /// The next row id.
    pub(super) fn row(&mut self) -> Result<RowId> {
        <[u8; ROW]>::try_from(self.take(ROW)?).map(RowId::from_be_bytes).map_err(|_| corrupt())
    }

    /// The next value key, as its octets.
    pub(super) fn value(&mut self) -> Result<&'a [u8]> {
        let from = self.at;
        match self.byte()? {
            UNORDERED => {}
            NUMBER => {
                self.take(DOUBLE)?;
            }
            STRING => self.string_end()?,
            _ => return Err(corrupt()),
        }
        self.bytes.get(from..self.at).ok_or_else(corrupt)
    }

    /// Past the end of an escaped string.
    fn string_end(&mut self) -> Result<()> {
        loop {
            if self.byte()? == 0 && self.byte()? != ESCAPED {
                return Ok(());
            }
        }
    }

    /// The next ORDPATH's ordinals, without their end, which is read.
    pub(super) fn ordpath(&mut self) -> Result<&'a [u8]> {
        let from = self.at;
        while self.ordinal()? != 0 {}
        self.bytes.get(from..self.at.saturating_sub(1)).ok_or_else(corrupt)
    }
}

/// The least key after every key beginning with `prefix`; unbounded if
/// every octet is FF, when no key comes after.
pub(super) fn after(prefix: &[u8]) -> Bound<Vec<u8>> {
    let Some(at) = prefix.iter().rposition(|&b| b < u8::MAX) else { return Bound::Unbounded };
    let mut next = prefix.get(..=at).unwrap_or_default().to_vec();
    if let Some(last) = next.last_mut() {
        *last = last.saturating_add(1);
    }
    Bound::Excluded(next)
}

/// The count an entry's value holds: one if it is empty, as most are.
pub(super) fn count_of(value: &[u8]) -> Result<u32> {
    if value.is_empty() {
        return Ok(1);
    }
    value.get(..4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_le_bytes).ok_or_else(corrupt)
}

/// The value of an entry holding `count`.
pub(super) fn counted(count: u32) -> Vec<u8> {
    if count == 1 { Vec::new() } else { count.to_le_bytes().to_vec() }
}

// Unit tests: each form orders as what it stands for, and reads back
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
