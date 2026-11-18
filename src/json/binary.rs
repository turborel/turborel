//! \[DESIGN\] The stored form of a value of the JSON type [ISO/IEC
//! 9075-2:2023 §4.8]: an SQL/JSON item [§4.48.3] as a compact tree of
//! octets, read in place.
//!
//! A value of the JSON type is held, logged and kept in a record as these
//! octets, never as JSON text, which is its input and output form only
//! (`json/parse_fn.rs`, `json/serialize_fn.rs`). An accessor walks the
//! octets without making the item: an array's elements and an object's
//! members are found by offsets, and an object's member of a key by a
//! binary search of its key directory, so a path of members and subscripts
//! reads only the nodes on its way (`binary/path.rs`).
//!
//! ```text
//! document:  version u8 (1) | node
//! node:      tag u8 | its body
//!   0 null, 1 false, 2 true           no body
//!   3 integer                         i64, an exact number of scale 0
//!   4 decimal                         units i64 | scale u8
//!   5 wide decimal                    units i128 | scale u32
//!   6 decimal floating point          coefficient i128 | exponent i32
//!   7 approximate                     the bits of an f64
//!   8 string                          length u32 | UTF-8 octets
//!   9 datetime                        moment u8 | utc i64 | zone i16 |
//!                                     zoned u8 | precision u8
//!   10 array                          count u32 | size u32 |
//!                                     count × offset u32 | elements
//!   11 object                         count u32 | size u32 |
//!                                     count × offset u32 | members
//! member:    key length u32 | key UTF-8 | value node
//! ```
//!
//! Integers are little-endian. An array's or an object's size is the
//! octets of its elements or members, after its offsets, so a node is
//! skipped without reading it. Each offset counts from the first element
//! or member. An object keeps its members in the order written, those of
//! one key all (UA057, US039, RULES.md D84), and its offsets are its key
//! directory: sorted by key, octet by octet, which is Unicode code point
//! order, and of one key by the order written, so the first member of a
//! key is found by a binary search, as an accessor takes it (UA075).
//!
//! The comparison key of a value, by which values of the JSON type are
//! equal, grouped and ordered [§4.8.2, §8.2 GR10], is `binary/key.rs`'s.

mod key;
mod node;
mod path;
mod write;

pub use key::{compare_documents, document_key, item_of_key};
pub use node::{Node, NodeKind};
pub use path::fast_path;
pub use write::encode;

use super::{Item, MAX_NESTING, beyond};
use crate::error::{Code, Result, SqlError};

/// The version of the format, the first octet of every document. It stays
/// 1 while turborel is alpha, as every format does, and a change to it
/// needs no migration (RULES.md D89).
pub const FORMAT_VERSION: u8 = 1;

/// \[DESIGN\] The most octets a document holds: its offsets are four
/// octets, and a value this long is far past what a row should carry.
/// One longer is SQLCODE -900 (RULES.md D18).
pub const MAX_DOCUMENT_OCTETS: usize = 1 << 28;

// The tags of the nodes, as the module's documentation lists them.
/// The SQL/JSON null.
const NULL: u8 = 0;
/// The truth value False.
const FALSE: u8 = 1;
/// The truth value True.
const TRUE: u8 = 2;
/// An exact number of scale 0 that an i64 holds.
const INTEGER: u8 = 3;
/// An exact number whose units an i64 holds, of a scale a u8 holds.
const DECIMAL: u8 = 4;
/// Any other exact number.
const WIDE: u8 = 5;
/// A decimal floating-point number.
const DECFLOAT: u8 = 6;
/// An approximate number.
const APPROXIMATE: u8 = 7;
/// A character string.
const STRING: u8 = 8;
/// A datetime.
const DATETIME: u8 = 9;
/// An array.
const ARRAY: u8 = 10;
/// An object.
const OBJECT: u8 = 11;

// The widths of the fields of a body.
/// A count, a size, an offset or a string's length.
const WORD: usize = 4;
/// An i64, an f64 or a datetime's instant.
const LONG: usize = 8;
/// An i128.
const WIDE_WORD: usize = 16;
/// An array's or an object's head after its tag: its count and its size.
const CONTAINER_HEAD: usize = 2 * WORD;
/// A datetime's body: its moment, instant, zone, zoned flag and precision.
const DATETIME_BODY: usize = 1 + LONG + 2 + 1 + 1;

/// The SQL/JSON item the document `bytes` holds.
///
/// # Errors
///
/// SQLCODE -916 if `bytes` is no document of this format.
pub fn decode(bytes: &[u8]) -> Result<Item> {
    root(bytes)?.item()
}

/// The root node of the document `bytes`.
///
/// # Errors
///
/// SQLCODE -916 if `bytes` begins with no version this format reads, or
/// holds nothing after it.
pub fn root(bytes: &[u8]) -> Result<Node<'_>> {
    match bytes.split_first() {
        Some((&FORMAT_VERSION, body)) if !body.is_empty() => Ok(Node::at(body)),
        Some(_) | None => Err(corrupt("it does not begin with its format's version")),
    }
}

/// That the document `bytes` is whole and of this format: each node within
/// it, every string UTF-8, every offset an element's or a member's, so a
/// document read from a record or the log is refused if damaged.
///
/// # Errors
///
/// SQLCODE -916 if it is not.
pub fn validate(bytes: &[u8]) -> Result<()> {
    let node = root(bytes)?;
    let length = node.check(0)?;
    if length.saturating_add(1) != bytes.len() {
        return Err(corrupt("octets follow its root"));
    }
    Ok(())
}

/// The error of a document that is not one of this format, `why`.
fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a JSON value's octets are damaged: {why}"))
}

/// The error of a document past `MAX_DOCUMENT_OCTETS`.
fn too_long() -> SqlError {
    beyond("its stored form is longer than 256 MiB")
}

/// That `depth` arrays and objects nest within `MAX_NESTING`.
///
/// # Errors
///
/// SQLCODE -900 if they do not.
fn within(depth: usize) -> Result<()> {
    if depth > MAX_NESTING {
        return Err(beyond("arrays and objects nest too deep"));
    }
    Ok(())
}

// Unit tests: the format written, read, validated and walked (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
