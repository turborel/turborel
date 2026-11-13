//! \[DESIGN\] A data type's bytes, as the catalog and the log keep it: a
//! tag of its kind, then its parameters; a number type's tag says the name
//! it was written with [ISO/IEC 9075:1992 §21.3.5]. An array type's
//! parameters hold its element type's bytes, and a row type's its fields'
//! (RULES.md D68); a distinct type's its name and its source type's
//! (RULES.md D69).

use super::{
    APPROXIMATE, ARRAY, BINARY, BINARY_VARYING, BOOLEAN, CHARACTER, DATE, DECFLOAT, DISTINCT, EXACT, INTERVAL_DAY_TIME, ROW, TIME,
    TIME_ZONED, TIMESTAMP, TIMESTAMP_ZONED, VARYING,
};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::{DataType, Spelled, SqlType};

/// The tag of each built-in kind of type, before its parameters.
const CHARACTER_TYPE: u8 = 0;
const EXACT_TYPE: u8 = 1;
const APPROXIMATE_TYPE: u8 = 2;
/// A type beyond the standard: its kind's name, then its parameters.
const EXTENSION_TYPE: u8 = 3;
const VARYING_TYPE: u8 = 4;
const BINARY_TYPE: u8 = 5;
const VARBINARY_TYPE: u8 = 6;
const DATE_TYPE: u8 = 7;
const TIME_TYPE: u8 = 8;
const TIME_ZONED_TYPE: u8 = 9;
const TIMESTAMP_TYPE: u8 = 10;
const TIMESTAMP_ZONED_TYPE: u8 = 11;
const INTERVAL_TYPE: u8 = 12;
/// An exact or approximate type written by another name than NUMERIC or
/// FLOAT [ISO/IEC 9075:1992 §21.3.5], kept so; an image of before reads
/// as NUMERIC or FLOAT, the same type.
const DECIMAL_TYPE: u8 = 13;
const INTEGER_TYPE: u8 = 14;
const SMALLINT_TYPE: u8 = 15;
const REAL_TYPE: u8 = 16;
const DOUBLE_PRECISION_TYPE: u8 = 17;
const BOOLEAN_TYPE: u8 = 18;
const DECFLOAT_TYPE: u8 = 19;
/// An array type: its maximum cardinality, then its element type.
const ARRAY_TYPE: u8 = 20;
/// A row type: its fields, each its name and then its type.
const ROW_TYPE: u8 = 21;
/// A distinct type: its schema's name and its own, then its source type
/// (RULES.md D69).
const DISTINCT_TYPE: u8 = 22;
/// An exact type written BIGINT [ISO/IEC 9075-2:2023 §6.1].
const BIGINT_TYPE: u8 = 23;

/// Appends `data_type`'s bytes to `out`.
pub(crate) fn put_type(out: &mut Vec<u8>, data_type: &DataType) {
    match data_type {
        DataType::Character { .. } => out.put_u8(CHARACTER_TYPE),
        DataType::Varying { .. } => out.put_u8(VARYING_TYPE),
        DataType::Binary { .. } => out.put_u8(BINARY_TYPE),
        DataType::VarBinary { .. } => out.put_u8(VARBINARY_TYPE),
        DataType::Boolean => out.put_u8(BOOLEAN_TYPE),
        DataType::DecFloat { .. } => out.put_u8(DECFLOAT_TYPE),
        DataType::Date => out.put_u8(DATE_TYPE),
        DataType::Time { zoned: false, .. } => out.put_u8(TIME_TYPE),
        DataType::Time { zoned: true, .. } => out.put_u8(TIME_ZONED_TYPE),
        DataType::Timestamp { zoned: false, .. } => out.put_u8(TIMESTAMP_TYPE),
        DataType::Timestamp { zoned: true, .. } => out.put_u8(TIMESTAMP_ZONED_TYPE),
        DataType::Interval(_) => out.put_u8(INTERVAL_TYPE),
        DataType::Exact { spelled, .. } | DataType::Approximate { spelled, .. } => out.put_u8(number_tag(*spelled)),
        DataType::Array(_) => out.put_u8(ARRAY_TYPE),
        DataType::Row(_) => out.put_u8(ROW_TYPE),
        DataType::Distinct(_) => out.put_u8(DISTINCT_TYPE),
        DataType::Extension(e) => {
            out.put_u8(EXTENSION_TYPE);
            out.put_str(e.kind.name());
        }
    }
    data_type.kind().put_params(data_type, out);
}

/// The tag of a number type written as `spelled`.
fn number_tag(spelled: Spelled) -> u8 {
    match spelled {
        Spelled::Numeric => EXACT_TYPE,
        Spelled::Decimal => DECIMAL_TYPE,
        Spelled::Integer => INTEGER_TYPE,
        Spelled::Smallint => SMALLINT_TYPE,
        Spelled::Bigint => BIGINT_TYPE,
        Spelled::Float => APPROXIMATE_TYPE,
        Spelled::Real => REAL_TYPE,
        Spelled::DoublePrecision => DOUBLE_PRECISION_TYPE,
    }
}

/// `t`, of a number type, written as `spelled`.
fn spelled_as(t: DataType, spelled: Spelled) -> DataType {
    match t {
        DataType::Exact { precision, scale, .. } => DataType::Exact { precision, scale, spelled },
        DataType::Approximate { precision, .. } => DataType::Approximate { precision, spelled },
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::DecFloat { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Extension(_) => t,
    }
}

/// Reads a type `put_type` wrote: of a kind beyond the standard, found
/// among the reader's types (`Reader::with_types`).
///
/// # Errors
///
/// SQLCODE -916 if the bytes are not a type; -910 if they name a kind no
/// plug-in registered.
pub(crate) fn data_type(r: &mut Reader<'_>) -> Result<DataType> {
    let tag = r.u8()?;
    let spelled = match tag {
        DECIMAL_TYPE => Some(Spelled::Decimal),
        INTEGER_TYPE => Some(Spelled::Integer),
        SMALLINT_TYPE => Some(Spelled::Smallint),
        BIGINT_TYPE => Some(Spelled::Bigint),
        REAL_TYPE => Some(Spelled::Real),
        DOUBLE_PRECISION_TYPE => Some(Spelled::DoublePrecision),
        _ => None,
    };
    let kind: &dyn SqlType = match tag {
        CHARACTER_TYPE => &CHARACTER,
        EXACT_TYPE | DECIMAL_TYPE | INTEGER_TYPE | SMALLINT_TYPE | BIGINT_TYPE => &EXACT,
        APPROXIMATE_TYPE | REAL_TYPE | DOUBLE_PRECISION_TYPE => &APPROXIMATE,
        VARYING_TYPE => &VARYING,
        BINARY_TYPE => &BINARY,
        VARBINARY_TYPE => &BINARY_VARYING,
        BOOLEAN_TYPE => &BOOLEAN,
        DECFLOAT_TYPE => &DECFLOAT,
        DATE_TYPE => &DATE,
        TIME_TYPE => &TIME,
        TIME_ZONED_TYPE => &TIME_ZONED,
        TIMESTAMP_TYPE => &TIMESTAMP,
        TIMESTAMP_ZONED_TYPE => &TIMESTAMP_ZONED,
        INTERVAL_TYPE => &INTERVAL_DAY_TIME,
        ARRAY_TYPE => &ARRAY,
        ROW_TYPE => &ROW,
        DISTINCT_TYPE => &DISTINCT,
        EXTENSION_TYPE => {
            let name = r.str()?;
            r.kind(&name)?
        }
        _ => return Err(r.corrupt("not a data type")),
    };
    let t = kind.params(r)?;
    Ok(match spelled {
        Some(s) => spelled_as(t, s),
        None => t,
    })
}

// Unit tests: each number type's name read back as written, and an image
// of before the names read as the same type (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// `t` written and read back.
    fn again(t: &DataType) -> DataType {
        let mut out = Vec::new();
        put_type(&mut out, t);
        data_type(&mut Reader::new(&out, "type")).unwrap()
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.3.5] a number type is
    /// read back with the name it was written with; the tags of before,
    /// NUMERIC's and FLOAT's, read as of no other name, the same type.
    #[test]
    fn number_types_keep_their_names() {
        let exact = |spelled| DataType::Exact { precision: 10, scale: 0, spelled };
        let approximate = |spelled| DataType::Approximate { precision: 53, spelled };
        for t in [
            exact(Spelled::Numeric),
            exact(Spelled::Decimal),
            exact(Spelled::Integer),
            exact(Spelled::Smallint),
            exact(Spelled::Bigint),
            approximate(Spelled::Float),
            approximate(Spelled::Real),
            approximate(Spelled::DoublePrecision),
        ] {
            assert_eq!(again(&t).to_string(), t.to_string());
        }
        assert_eq!(again(&exact(Spelled::Integer)), exact(Spelled::Numeric), "one type, whatever its name");
        let mut before = vec![EXACT_TYPE];
        before.put_u32(10);
        before.put_u32(0);
        let read = data_type(&mut Reader::new(&before, "type")).unwrap();
        assert_eq!(read.to_string(), "NUMERIC(10,0)");
    }
}
