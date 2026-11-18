//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.7.2 and §9.2.2 Description
//! 1 to 8] The constructors of a compound surface and a B-rep solid of a
//! representation. A CHARACTER LARGE OBJECT is read as GML if it holds an
//! XML element, the text beginning with `<`, and as well-known text else;
//! a BINARY LARGE OBJECT as well-known binary. Each is the type's
//! FromText, FromWKB or FromGML function, given the reference systems
//! last where the text may be GML.

use crate::error::Result;
use crate::functions::{AppendedQuery, ScalarFunction};
use crate::geometry::functions::{BINARY, MAX_REPRESENTATION, TEXT};
use crate::geometry::model::Kind;
use crate::geometry::representations::{CONSTRUCTORS, Format, MORE_CONSTRUCTORS, systems_query};
use crate::value::lob::LobKind;
use crate::value::{DataType, Value};

/// Whether `t` is a representation's type, a character or binary string.
pub fn is_representation(t: &DataType) -> bool {
    t.is_character() || t.is_binary()
}

/// The type of the representation argument of a call whose first
/// argument is known as `known`, none if it is no representation.
pub fn representation_type(known: &[Option<DataType>]) -> Option<DataType> {
    let first = known.first().cloned().flatten().filter(is_representation)?;
    Some(if first.is_character() { TEXT } else { BINARY })
}

/// The arguments of a call, the appended reference systems of a text
/// representation apart.
pub fn own(args: &[DataType]) -> usize {
    let systems = usize::from(args.first().is_some_and(DataType::is_character));
    args.len().saturating_sub(systems)
}

/// The reference systems' query, appended to a call of a character
/// string first, which may be GML.
pub fn appended(types: &[DataType]) -> Option<AppendedQuery> {
    types.first().filter(|t| t.is_character()).map(|_| systems_query())
}

/// Whether `v` is a character string, a text representation.
fn is_text(v: &Value) -> bool {
    if let Value::Lob(lob) = v { lob.kind() == LobKind::Character } else { matches!(v, Value::Char(_)) }
}

/// Whether `v` is a binary string, a well-known binary representation.
fn is_binary(v: &Value) -> bool {
    if let Value::Lob(lob) = v { lob.kind() == LobKind::Binary } else { matches!(v, Value::Binary(_)) }
}

/// Whether the text `v` begins with an XML element.
fn is_gml(v: &Value) -> Result<bool> {
    let bytes = if let Value::Lob(lob) = v {
        lob.bounded(MAX_REPRESENTATION)?.unwrap_or_default()
    } else if let Value::Char(s) = v {
        s.as_bytes().to_vec()
    } else {
        Vec::new()
    };
    Ok(bytes.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<'))
}

/// The value of type `target` the representation first of `args` gives,
/// the SRID after it if given; none if the first argument is no
/// representation.
///
/// # Errors
///
/// As the type's FromText, FromWKB or FromGML function.
pub fn read(target: Kind, args: &[Value]) -> Result<Option<Value>> {
    let Some(first) = args.first() else { return Ok(None) };
    let format = if is_binary(first) {
        Format::Binary
    } else if !is_text(first) {
        return Ok(None);
    } else if is_gml(first)? {
        Format::Gml
    } else {
        Format::Text
    };
    // A text's call is given the reference systems last; only GML reads them.
    let given = if format == Format::Text { args.split_last().map_or(args, |(_, rest)| rest) } else { args };
    let all = CONSTRUCTORS.iter().chain(&MORE_CONSTRUCTORS);
    let Some(f) = all.into_iter().find(|c| c.target == target && c.format == format) else { return Ok(None) };
    f.call(given).map(Some)
}
