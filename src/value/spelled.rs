//! The name a number's type was written with [ISO/IEC 9075:1992 §6.1,
//! §21.3.5], kept for display alone.

use super::DataType;

/// [ISO/IEC 9075:1992 §6.1, §21.3.5] The name a number's type was
/// written with, which its data type descriptor keeps: INTEGER is
/// NUMERIC(10, 0), and REAL FLOAT(24), but each is shown as written. It is
/// no part of what the type is: two types of one precision and scale are
/// equal, however written, so a name changes no comparison, assignment or
/// result. A type computed, as of a sum, is NUMERIC, or FLOAT.
#[derive(Debug, Clone, Copy, Default)]
pub enum Spelled {
    /// NUMERIC, or a type computed.
    #[default]
    Numeric,
    /// DECIMAL or DEC.
    Decimal,
    /// INTEGER or INT.
    Integer,
    /// SMALLINT.
    Smallint,
    /// BIGINT [ISO/IEC 9075-2:2023 §6.1, Feature T071].
    Bigint,
    /// FLOAT, or an approximate type computed.
    Float,
    /// REAL.
    Real,
    /// DOUBLE PRECISION.
    DoublePrecision,
}

/// How the number type `t` was written; NUMERIC of any other type.
pub fn spelling(t: &DataType) -> Spelled {
    match t {
        DataType::Exact { spelled, .. } | DataType::Approximate { spelled, .. } => *spelled,
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::DecFloat { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Json
        | DataType::Extension(_) => Spelled::Numeric,
    }
}

impl PartialEq for Spelled {
    /// Always: the name is not the type.
    fn eq(&self, _: &Spelled) -> bool {
        true
    }
}
