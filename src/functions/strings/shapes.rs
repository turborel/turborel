//! [ISO/IEC 9075-2:2023 §6.32, §6.33] The string types the string
//! functions take, by family and form, and the types of their results: of
//! a large object, a large object of its family \[§6.32 SR3b i, SR6a i;
//! §6.33 SR4a ii, SR14b ii, SR21b, SR22b ii\].

use crate::error::{Code, Result, fail};
use crate::value::lob::{LobKind, MAX_LOB_LENGTH};
use crate::value::{CharUnits, DataType};

/// The form of a string type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Form {
    /// Fixed length: CHARACTER or BINARY.
    Fixed,
    /// Varying length: CHARACTER VARYING or BINARY VARYING.
    Varying,
    /// A large object: CHARACTER or BINARY LARGE OBJECT.
    Large,
}

/// A string type: its family, form, length or maximum length, and what a
/// character string's length counts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Shape {
    /// The family: binary or character.
    pub kind: LobKind,
    /// Its form.
    pub form: Form,
    /// Its length or maximum length.
    pub length: u64,
    /// What its length counts: characters of a binary string, as a binary
    /// large object's type says, whose characters are its octets.
    pub units: CharUnits,
}

/// The shape of `t`; none if it is no string type.
pub(crate) fn shape(t: &DataType) -> Option<Shape> {
    let (kind, form, length, units) = match t {
        DataType::Character { length, units } => (LobKind::Character, Form::Fixed, u64::from(*length), *units),
        DataType::Varying { length, units } => (LobKind::Character, Form::Varying, u64::from(*length), *units),
        DataType::Binary { length } => (LobKind::Binary, Form::Fixed, u64::from(*length), CharUnits::Characters),
        DataType::VarBinary { length } => (LobKind::Binary, Form::Varying, u64::from(*length), CharUnits::Characters),
        DataType::Lob { kind, length, units } => (*kind, Form::Large, *length, *units),
        DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Extension(_) => return None,
    };
    Some(Shape { kind, form, length, units })
}

/// The shape of `t` if it is a large object.
pub(crate) fn large(t: &DataType) -> Option<Shape> {
    shape(t).filter(|s| s.form == Form::Large)
}

/// The large object type of `kind`, `length` at most the longest, counted
/// in `units`.
pub(crate) fn large_type(kind: LobKind, length: u64, units: CharUnits) -> DataType {
    DataType::Lob { kind, length: length.min(MAX_LOB_LENGTH), units }
}

/// [ISO/IEC 9075-2:2023 §6.32 SR3b i, SR6a i] The type of `||` of `a` and
/// `b` if either is a large object: a large object of their family, of
/// the lesser of the sum of their lengths and the longest; none if
/// neither is.
///
/// # Errors
///
/// SQLCODE -103 if they are not of one family \[SR1, SR5\].
pub(crate) fn large_concatenation(a: &DataType, b: &DataType) -> Result<Option<DataType>> {
    let (Some(x), Some(y)) = (shape(a), shape(b)) else { return Ok(None) };
    if x.form != Form::Large && y.form != Form::Large {
        return Ok(None);
    }
    if x.kind != y.kind {
        return fail(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §6.32 SR1, SR5] || joins strings of one family, not {a} and {b}"));
    }
    // [ISO/IEC 9075-2:2023 §6.1 SR16] in octets if both are, else in
    // characters.
    let units = if x.units == y.units { x.units } else { CharUnits::Characters };
    Ok(Some(large_type(x.kind, x.length.saturating_add(y.length), units)))
}
