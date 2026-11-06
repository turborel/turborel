//! [X3.135-1986 §5.6; ISO/IEC 9075:1992 §6.2] `<value specification>`: its
//! value and data type, taken from a parameter, a literal or the session.

use super::datetime;
use crate::ast::{Literal, ValueSpecification};
use crate::error::{At, Result};
use crate::params::{ParameterType, Params};
use crate::value::{DOUBLE_PRECISION, DataType, Spelled, Value};

/// [X3.135-1986 §5.6] `<value specification>`: its value and data type.
///
/// # Errors
///
/// A parameter is undeclared, its indicator is not exact with scale 0, or its
/// argument does not suit it.
pub fn value_specification(v: &ValueSpecification, auth: &str, params: &Params) -> Result<(Value, DataType)> {
    Ok(match v {
        // [X3.135-1986 §5.6 GR1]
        ValueSpecification::Parameter(p) => params.input(p).or_at(p.at)?,
        // [X3.135-1986 §5.6 GR3]
        ValueSpecification::Literal(Literal::Character(s)) => (Value::Char(s.clone()), DataType::Character { length: char_length(s) }),
        ValueSpecification::Literal(Literal::Exact(d)) => {
            (Value::Exact(*d), DataType::Exact { precision: d.digits().max(d.scale), scale: d.scale, spelled: Spelled::Numeric })
        }
        ValueSpecification::Literal(Literal::Approximate(f)) => {
            (Value::Approx(*f), DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float })
        }
        // [ISO/IEC 9075:1992 §5.3 SR10, SR11] fixed, of its bits.
        ValueSpecification::Literal(Literal::Bit(b)) => (Value::Bits(b.clone()), DataType::Bit { length: b.len() }),
        // [ISO/IEC 9075:1992 §5.3 SR15-SR18] here of UTC: a statement's
        // binder gives its session's displacement (`Binder::value_specification`).
        ValueSpecification::Literal(Literal::Datetime(w)) => datetime::literal(w, 0)?,
        // [ISO/IEC 9075:1992 §5.3 SR20] of its qualifier.
        ValueSpecification::Literal(Literal::Interval(i)) => (Value::Interval(*i), DataType::Interval(i.qualifier)),
        // [X3.135-1986 §5.6 GR4] USER is the current authorization identifier.
        // [X3.135-1986 §5.6 SR6] its length is implementor-defined; we use the identifier's own length.
        // [ISO/IEC 9075:1992 §6.2 GR10, GR11] So are SESSION_USER, there
        // being no other in a session, and SYSTEM_USER, the
        // implementation-defined string being it: a library runs under no
        // operating system user of its own.
        ValueSpecification::User(_) => (Value::Char(auth.to_string()), DataType::Character { length: char_length(auth) }),
        // [ISO/IEC 9075:1992 §17.6 GR4] where nothing gives it a type: the
        // contexts that do bind it by `Binder::typed`.
        ValueSpecification::Dynamic { index, at } => params.dynamic_value(*index, None).or_at(*at)?,
    })
}

/// [ISO/IEC 9075:1992 §6.2] `v`, a `<value specification>`, a `<dynamic
/// parameter specification>` taking the type `taken` its context gives it
/// [§17.6 GR5].
///
/// # Errors
///
/// As `value_specification`.
pub fn value_specification_as(v: &ValueSpecification, auth: &str, params: &Params, taken: ParameterType) -> Result<(Value, DataType)> {
    if let ValueSpecification::Dynamic { index, at } = v {
        return params.dynamic_value(*index, Some(taken)).or_at(*at);
    }
    value_specification(v, auth, params)
}

/// The length of `s` in characters, as a CHARACTER type's length. A string
/// longer than `u32::MAX` characters is given that length.
pub(super) fn char_length(s: &str) -> u32 {
    u32::try_from(s.chars().count()).unwrap_or(u32::MAX)
}
