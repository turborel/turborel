//! \[DESIGN\] A message's tail (RULES.md D185): the fields version 1 gained
//! after a message's first form, each a tag and its field, in increasing
//! order of tag, to the end of the body. A field is written only where
//! the capability that adds it was granted and it says something, so a
//! message without them is byte for byte its first form, and a peer that
//! did not ask never reads one. A tag a message does not take, or one out
//! of order, is a connection failure: no peer sends one it was not
//! granted.
//!
//! ```text
//! tail:   { tag u8, then its field }...
//! 1  attributes   count u32, then each name and value, strings
//! 2  multi        none: the Query's text is several statements
//! 3  cursor       u32: the client's number of a statement's cursor
//! 4  position     u64: the log position of the commit
//! 5  more         none: another statement's replies follow
//! 6  rows         u64: the rows a statement inserted
//! 7  severity     string     8  detail       string
//! 9  hint         string     10 context      string
//! 11 schema       string     12 table        string
//! 13 column       string     14 data type    string
//! 15 constraint   string
//! 16 generated    count u32, then each identity value as SQL writes it
//! 17 conditions   count u32, then each condition, the body of an Error
//! 18 time         u64 each: the server's microseconds for the statement,
//!                 of them queued for a worker, of them waiting for locks
//! ```

use turborel::bytes::{Put, Reader};
use turborel::error::Result;

use crate::Attributes;

/// A Query's or an Execute's attributes, or Hello's.
pub(crate) const ATTRIBUTES: u8 = 1;
/// A Query of several statements.
pub(crate) const MULTI: u8 = 2;
/// The cursor of an Execute, a Fetch or a Close.
pub(crate) const CURSOR: u8 = 3;
/// A commit's log position, of a Complete.
pub(crate) const POSITION: u8 = 4;
/// More replies follow, of a Complete.
pub(crate) const MORE: u8 = 5;
/// The rows inserted, of a Complete.
pub(crate) const ROWS: u8 = 6;
/// The first of a failure's or a notice's diagnostics, in the order of
/// `Diagnostics::strings`.
pub(crate) const SEVERITY: u8 = 7;
/// The identity values an INSERT generated, of a Complete.
pub(crate) const GENERATED: u8 = 16;
/// The conditions a statement raised, of a Complete.
pub(crate) const CONDITIONS: u8 = 17;
/// The server's time for a statement, of a Complete.
pub(crate) const TIME: u8 = 18;

/// \[DESIGN\] The most attributes a message takes.
pub const MAX_ATTRIBUTES: usize = 32;
/// \[DESIGN\] The longest name of an attribute, in bytes.
pub const MAX_ATTRIBUTE_NAME: usize = 64;
/// \[DESIGN\] The longest value of an attribute, in bytes.
pub const MAX_ATTRIBUTE_VALUE: usize = 1024;

/// Writes `tag` with no field.
pub(crate) fn put_mark(b: &mut Vec<u8>, tag: u8, set: bool) {
    if set {
        b.put_u8(tag);
    }
}

/// Writes `tag` and `n`, unless `n` is `none`.
pub(crate) fn put_number(b: &mut Vec<u8>, tag: u8, n: u32, none: u32) {
    if n != none {
        b.put_u8(tag);
        b.put_u32(n);
    }
}

/// Writes `tag` and `n`, if there is one.
pub(crate) fn put_long(b: &mut Vec<u8>, tag: u8, n: Option<u64>) {
    if let Some(n) = n {
        b.put_u8(tag);
        b.put_u64(n);
    }
}

/// Writes `tag` and `s`, if there is one.
pub(crate) fn put_text(b: &mut Vec<u8>, tag: u8, s: Option<&str>) {
    if let Some(s) = s {
        b.put_u8(tag);
        b.put_str(s);
    }
}

/// Writes `tag` and `attributes`, unless there are none.
pub(crate) fn put_attributes(b: &mut Vec<u8>, tag: u8, attributes: &Attributes) {
    if attributes.is_empty() {
        return;
    }
    b.put_u8(tag);
    b.put_u32(u32::try_from(attributes.len()).unwrap_or(u32::MAX));
    for (name, value) in attributes {
        b.put_str(name);
        b.put_str(value);
    }
}

/// Attributes, as `put_attributes` wrote them: at most `MAX_ATTRIBUTES`,
/// each name and value within its bound.
pub(crate) fn attributes(r: &mut Reader<'_>) -> Result<Attributes> {
    let count = usize::try_from(r.u32()?).map_err(|_| r.corrupt("a count is too large"))?;
    if count > MAX_ATTRIBUTES {
        return Err(r.corrupt(&format!("{count} attributes are more than the {MAX_ATTRIBUTES} taken")));
    }
    let mut all = Vec::with_capacity(count);
    for _ in 0..count {
        let (name, value) = (r.str()?, r.str()?);
        if name.is_empty() || name.len() > MAX_ATTRIBUTE_NAME || value.len() > MAX_ATTRIBUTE_VALUE {
            return Err(
                r.corrupt(&format!("an attribute's name is 1 to {MAX_ATTRIBUTE_NAME} bytes and its value at most {MAX_ATTRIBUTE_VALUE}"))
            );
        }
        all.push((name, value));
    }
    Ok(all)
}

/// The tags of a tail, read in order: each must be one the message takes,
/// past the last.
pub(crate) struct Tail {
    last: u8,
}

impl Tail {
    /// The start of a tail.
    pub(crate) fn new() -> Tail {
        Tail { last: 0 }
    }

    /// The next tag, none at the end of the body; refused if it is not
    /// past the last or not among `taken`.
    pub(crate) fn next(&mut self, r: &mut Reader<'_>, taken: &[u8]) -> Result<Option<u8>> {
        if r.at_end() {
            return Ok(None);
        }
        let tag = r.u8()?;
        if tag <= self.last || !taken.contains(&tag) {
            return Err(r.corrupt(&format!("its tail has tag {tag} out of place")));
        }
        self.last = tag;
        Ok(Some(tag))
    }
}
