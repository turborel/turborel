//! \[DESIGN\] The keys of an adjacency index's tree (RULES.md D34), tagged
//! by their first byte, each endpoint's values encoded as an index key is
//! (`engine/key.rs`), so equal vertices have equal bytes and no vertex's
//! bytes begin another's:
//!
//! ```text
//! F source destination row included     forward: the versions holding it
//! G O vertex | G I vertex                 the vertex's out and in degree
//! R destination source row included     reverse: the versions holding it
//! ```
//!
//! An entry's value counts the versions of its row holding it (D29):
//! empty for one, else four bytes. A degree counts the entries under the
//! vertex, eight bytes. A vertex's entries are adjacent, ordered by the
//! other endpoint, then the row, so versions of one edge differing only in
//! an included property are adjacent too, and one step is made of them.

use std::ops::Bound;

use crate::engine::key::{anchor, decode_front, encode_key};
use crate::error::{Code, Result, SqlError};
use crate::graph::adjacency::Direction;
use crate::index::Key;
use crate::storage::RowId;
use crate::value::{DataType, Value};

use super::options::Layout;

/// The tag of a forward entry's key.
pub const FORWARD: u8 = b'F';
/// The tag of a degree's key.
pub const DEGREE: u8 = b'G';
/// The tag of a reverse entry's key.
pub const REVERSE: u8 = b'R';
/// The byte after `DEGREE` of an out degree.
const OUT_DEGREE: u8 = b'O';
/// The byte after `DEGREE` of an in degree.
const IN_DEGREE: u8 = b'I';
/// Bytes of a row id.
const ROW_BYTES: usize = 8;
/// Bytes of a count of versions.
const VERSIONS_BYTES: usize = 4;

/// The error of a key or value not of this layout.
pub fn corrupt() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] an adjacency index's entry is not of its layout")
}

/// The tag of the entries followed in `direction`.
pub fn tag(direction: Direction) -> u8 {
    match direction {
        Direction::Out => FORWARD,
        Direction::In => REVERSE,
    }
}

/// The byte of a degree in `direction`.
fn degree_byte(direction: Direction) -> u8 {
    match direction {
        Direction::Out => OUT_DEGREE,
        Direction::In => IN_DEGREE,
    }
}

/// An index key's parts, encoded.
pub struct Encoded {
    /// The source's.
    pub source: Vec<u8>,
    /// The destination's.
    pub destination: Vec<u8>,
    /// The included columns'.
    pub included: Vec<u8>,
}

impl Encoded {
    /// The parts of `key`, of `layout`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it is not a key of the layout's types.
    pub fn of(key: &Key, layout: &Layout) -> Result<Encoded> {
        let values = |at: std::ops::Range<usize>| key.0.get(at).unwrap_or_default();
        let (source, ends) = (layout.source, layout.endpoints());
        if key.0.len() != layout.types.len() {
            return Err(SqlError::new(
                Code::Api,
                format!("[DESIGN] a key of {} values for an index on {} columns", key.0.len(), layout.types.len()),
            ));
        }
        let mut parts = Encoded { source: Vec::new(), destination: Vec::new(), included: Vec::new() };
        encode_key(values(0..source), layout.source_types(), &mut parts.source)?;
        encode_key(values(source..ends), layout.destination_types(), &mut parts.destination)?;
        encode_key(values(ends..key.0.len()), layout.included_types(), &mut parts.included)?;
        Ok(parts)
    }

    /// The vertex followed from, then the one reached, in `direction`.
    pub fn ends(&self, direction: Direction) -> (&[u8], &[u8]) {
        match direction {
            Direction::Out => (&self.source, &self.destination),
            Direction::In => (&self.destination, &self.source),
        }
    }

    /// The whole key's bytes, as one key of its columns encodes.
    pub fn whole(&self) -> Vec<u8> {
        [self.source.as_slice(), &self.destination, &self.included].concat()
    }
}

/// The key of the entry of `row` in `direction`, from `vertex` to `other`.
pub fn entry_key(direction: Direction, (vertex, other): (&[u8], &[u8]), row: RowId, included: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(vertex.len().saturating_add(other.len()).saturating_add(included.len()).saturating_add(ROW_BYTES + 1));
    key.push(tag(direction));
    key.extend_from_slice(vertex);
    key.extend_from_slice(other);
    key.extend_from_slice(&row.to_be_bytes());
    key.extend_from_slice(included);
    key
}

/// The key of `vertex`'s degree in `direction`.
pub fn degree_key(direction: Direction, vertex: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(vertex.len().saturating_add(2));
    key.push(DEGREE);
    key.push(degree_byte(direction));
    key.extend_from_slice(vertex);
    key
}

/// \[DESIGN\] A load's record of an entry's place in its vertex's degree:
/// the degree's key, then the entry's other end, row and included
/// columns, so the records of one vertex sort together, and those of the
/// versions of one entry are alike (`load.rs`).
pub fn degree_record(direction: Direction, (vertex, other): (&[u8], &[u8]), row: RowId, included: &[u8]) -> Vec<u8> {
    let mut record = degree_key(direction, vertex);
    record.extend_from_slice(other);
    record.extend_from_slice(&row.to_be_bytes());
    record.extend_from_slice(included);
    record
}

/// The degree's key a load's record of it begins with (`degree_record`).
///
/// # Errors
///
/// SQLCODE -916 if it is not of the layout.
pub fn degree_prefix<'a>(record: &'a [u8], layout: &Layout) -> Result<&'a [u8]> {
    let (head, mut rest) = record.split_at_checked(2).ok_or_else(corrupt)?;
    let direction = match head.get(1).copied() {
        Some(OUT_DEGREE) => Direction::Out,
        Some(IN_DEGREE) => Direction::In,
        Some(_) | None => return Err(corrupt()),
    };
    let vertex = take(&mut rest, layout.ends(direction).0)?;
    record.get(..vertex.len().saturating_add(2)).ok_or_else(corrupt)
}

/// The bounds of the keys beginning with `prefix`: from it, to the least
/// key after every key it begins.
pub fn within(prefix: &[u8]) -> (Bound<Vec<u8>>, Bound<Vec<u8>>) {
    let upper = match prefix.iter().rposition(|&b| b < u8::MAX) {
        Some(at) => {
            let mut next = prefix.get(..=at).unwrap_or_default().to_vec();
            if let Some(last) = next.last_mut() {
                *last = last.saturating_add(1);
            }
            Bound::Excluded(next)
        }
        None => Bound::Unbounded,
    };
    (Bound::Included(prefix.to_vec()), upper)
}

/// The bytes of `values`, of columns of `types`, as a prefix of keys, and
/// whether the keys it finds are to be checked: each value placed by its
/// anchor (`engine/key.rs`), the keys checked if one is not the value's
/// own, and the prefix ended before a value of another kind, which every
/// key is checked against. None of a null, which no key equals
/// [X3.135-1986 §5.11 GR2].
pub fn anchored(values: &[Value], types: &[DataType]) -> Option<(Vec<u8>, bool)> {
    if values.iter().any(Value::is_null) {
        return None;
    }
    let (mut prefix, mut checked) = (Vec::new(), false);
    for (value, data_type) in values.iter().zip(types) {
        let Some(at) = anchor(value, data_type) else { return Some((prefix, true)) };
        checked |= !at.exact;
        prefix.extend(at.key);
    }
    Some((prefix, checked))
}

/// An entry's key read back: of its direction, the vertex followed from,
/// the one reached, the row, and the included columns, each in bytes.
pub struct Parsed<'a> {
    /// The direction it is followed in.
    pub direction: Direction,
    /// The vertex it is under.
    pub vertex: &'a [u8],
    /// The vertex at its other end.
    pub other: &'a [u8],
    /// The edge's row.
    pub row: RowId,
    /// The included columns.
    pub included: &'a [u8],
}

/// The bytes a key of columns of `types` takes at the start of `rest`, `rest` left after it.
fn take<'a>(rest: &mut &'a [u8], types: &[DataType]) -> Result<&'a [u8]> {
    let start = *rest;
    decode_front(rest, types)?;
    start.get(..start.len().saturating_sub(rest.len())).ok_or_else(corrupt)
}

/// The parts of the entry's key `entry`, of `layout`.
///
/// # Errors
///
/// SQLCODE -916 if it is not of the layout.
pub fn parse<'a>(entry: &'a [u8], layout: &Layout) -> Result<Parsed<'a>> {
    let (first, mut rest) = entry.split_first().ok_or_else(corrupt)?;
    let direction = match *first {
        FORWARD => Direction::Out,
        REVERSE => Direction::In,
        _ => return Err(corrupt()),
    };
    let (vertex_types, other_types) = layout.ends(direction);
    let vertex = take(&mut rest, vertex_types)?;
    let other = take(&mut rest, other_types)?;
    let (row, included) = rest.split_at_checked(ROW_BYTES).ok_or_else(corrupt)?;
    let row = RowId::from_be_bytes(row.try_into().map_err(|_| corrupt())?);
    Ok(Parsed { direction, vertex, other, row, included })
}

impl Parsed<'_> {
    /// The index key it is of, its columns in order.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if a part is not of the layout.
    pub fn key(&self, layout: &Layout) -> Result<Key> {
        let (vertex_types, other_types) = layout.ends(self.direction);
        let vertex = decode_all(self.vertex, vertex_types)?;
        let other = decode_all(self.other, other_types)?;
        let mut values = match self.direction {
            Direction::Out => [vertex, other].concat(),
            Direction::In => [other, vertex].concat(),
        };
        values.extend(decode_all(self.included, layout.included_types())?);
        Ok(Key(values))
    }
}

/// The values of the key `bytes`, of columns of `types`, nothing after it.
///
/// # Errors
///
/// SQLCODE -916 if it is not one.
pub fn decode_all(bytes: &[u8], types: &[DataType]) -> Result<Vec<Value>> {
    let mut rest = bytes;
    let values = decode_front(&mut rest, types)?;
    if rest.is_empty() { Ok(values) } else { Err(corrupt()) }
}

/// The versions an entry's value counts: empty for one.
///
/// # Errors
///
/// SQLCODE -916 of another length than none or four bytes.
pub fn versions_of(value: &[u8]) -> Result<u64> {
    if value.is_empty() {
        return Ok(1);
    }
    let four: [u8; VERSIONS_BYTES] = value.try_into().map_err(|_| corrupt())?;
    Ok(u64::from(u32::from_be_bytes(four)))
}

/// The value counting `n` versions.
pub fn versions_value(n: u64) -> Vec<u8> {
    if n == 1 { Vec::new() } else { u32::try_from(n).unwrap_or(u32::MAX).to_be_bytes().to_vec() }
}

/// The degree a value holds.
///
/// # Errors
///
/// SQLCODE -916 if it is not eight bytes.
pub fn degree_of(value: &[u8]) -> Result<u64> {
    value.try_into().map(u64::from_be_bytes).map_err(|_| corrupt())
}

/// The value of degree `n`.
pub fn degree_value(n: u64) -> Vec<u8> {
    n.to_be_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::adjacency::options::Kept;
    use crate::value::{Decimal, Spelled};

    fn int() -> DataType {
        DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }
    }

    fn n(v: i128) -> Value {
        Value::Exact(Decimal::new(v, 0))
    }

    /// Verifies: [DESIGN] an entry's key reads back its parts, in either
    /// direction, the index key in column order; degrees sort between the
    /// forward entries and the reverse; counts read back.
    #[test]
    fn keys_read_back_their_parts() {
        let layout = Layout::new(&[int(), int(), int()], (1, 1), Kept::Both).unwrap();
        let key = Key(vec![n(7), n(9), n(100)]);
        let parts = Encoded::of(&key, &layout).unwrap();
        for direction in [Direction::Out, Direction::In] {
            let entry = entry_key(direction, parts.ends(direction), 42, &parts.included);
            let parsed = parse(&entry, &layout).unwrap();
            assert_eq!((parsed.direction, parsed.row, parsed.included), (direction, 42, parts.included.as_slice()));
            assert_eq!(parsed.key(&layout).unwrap(), key, "{direction:?}");
            assert!(entry_key(Direction::Out, parts.ends(direction), 42, &[]) < degree_key(direction, &parts.source));
            assert!(degree_key(direction, &parts.source) < entry_key(Direction::In, parts.ends(direction), 0, &[]));
        }
        assert!(Encoded::of(&Key(vec![n(1)]), &layout).is_err(), "a short key");
        assert_eq!((versions_of(&versions_value(1)).unwrap(), versions_of(&versions_value(3)).unwrap()), (1, 3));
        assert_eq!(degree_of(&degree_value(5)).unwrap(), 5);
        assert!(parse(b"X", &layout).is_err() && versions_of(&[1]).is_err());
        assert_eq!(anchored(&[Value::Null], &[int()]), None);
    }
}
