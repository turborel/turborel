//! \[DESIGN\] A data type in the catalog's bytes: a tag of its kind, then
//! its parameters; a number type's tag says the name it was written with
//! [ISO/IEC 9075:1992 §21.3.5]. Split from catalog/codec.rs (RULES.md
//! R-SIZE-2).

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::types::INTERVAL_DAY_TIME;
use crate::value::types::{APPROXIMATE, BIT, BIT_VARYING, CHARACTER, DATE, EXACT, TIME, TIME_ZONED, TIMESTAMP, TIMESTAMP_ZONED, VARYING};
use crate::value::{DataType, Spelled, SqlType};

/// The tag of each built-in kind of type, before its parameters.
const CHARACTER_TYPE: u8 = 0;
const EXACT_TYPE: u8 = 1;
const APPROXIMATE_TYPE: u8 = 2;
/// A type beyond the standard: its kind's name, then its parameters.
const EXTENSION_TYPE: u8 = 3;
const VARYING_TYPE: u8 = 4;
const BIT_TYPE: u8 = 5;
const BIT_VARYING_TYPE: u8 = 6;
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

pub(super) fn put_type(out: &mut Vec<u8>, data_type: DataType) {
    match data_type {
        DataType::Character { .. } => out.put_u8(CHARACTER_TYPE),
        DataType::Varying { .. } => out.put_u8(VARYING_TYPE),
        DataType::Bit { .. } => out.put_u8(BIT_TYPE),
        DataType::BitVarying { .. } => out.put_u8(BIT_VARYING_TYPE),
        DataType::Date => out.put_u8(DATE_TYPE),
        DataType::Time { zoned: false, .. } => out.put_u8(TIME_TYPE),
        DataType::Time { zoned: true, .. } => out.put_u8(TIME_ZONED_TYPE),
        DataType::Timestamp { zoned: false, .. } => out.put_u8(TIMESTAMP_TYPE),
        DataType::Timestamp { zoned: true, .. } => out.put_u8(TIMESTAMP_ZONED_TYPE),
        DataType::Interval(_) => out.put_u8(INTERVAL_TYPE),
        DataType::Exact { spelled, .. } | DataType::Approximate { spelled, .. } => out.put_u8(number_tag(spelled)),
        DataType::Extension(e) => {
            out.put_u8(EXTENSION_TYPE);
            out.put_str(e.kind.name());
        }
    }
    data_type.kind().put_params(&data_type, out);
}

/// The tag of a number type written as `spelled`.
fn number_tag(spelled: Spelled) -> u8 {
    match spelled {
        Spelled::Numeric => EXACT_TYPE,
        Spelled::Decimal => DECIMAL_TYPE,
        Spelled::Integer => INTEGER_TYPE,
        Spelled::Smallint => SMALLINT_TYPE,
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
        | DataType::Bit { .. }
        | DataType::BitVarying { .. }
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Extension(_) => t,
    }
}

pub(super) fn data_type(r: &mut Reader<'_>) -> Result<DataType> {
    let tag = r.u8()?;
    let spelled = match tag {
        DECIMAL_TYPE => Some(Spelled::Decimal),
        INTEGER_TYPE => Some(Spelled::Integer),
        SMALLINT_TYPE => Some(Spelled::Smallint),
        REAL_TYPE => Some(Spelled::Real),
        DOUBLE_PRECISION_TYPE => Some(Spelled::DoublePrecision),
        _ => None,
    };
    let kind: &dyn SqlType = match tag {
        CHARACTER_TYPE => &CHARACTER,
        EXACT_TYPE | DECIMAL_TYPE | INTEGER_TYPE | SMALLINT_TYPE => &EXACT,
        APPROXIMATE_TYPE | REAL_TYPE | DOUBLE_PRECISION_TYPE => &APPROXIMATE,
        VARYING_TYPE => &VARYING,
        BIT_TYPE => &BIT,
        BIT_VARYING_TYPE => &BIT_VARYING,
        DATE_TYPE => &DATE,
        TIME_TYPE => &TIME,
        TIME_ZONED_TYPE => &TIME_ZONED,
        TIMESTAMP_TYPE => &TIMESTAMP,
        TIMESTAMP_ZONED_TYPE => &TIMESTAMP_ZONED,
        INTERVAL_TYPE => &INTERVAL_DAY_TIME,
        EXTENSION_TYPE => {
            let name = r.str()?;
            r.kind(&name)?
        }
        _ => return Err(r.corrupt("not a data type")),
    };
    let t = kind.params(r)?;
    Ok(spelled.map_or(t, |s| spelled_as(t, s)))
}

// Unit tests: each number type's name read back as written, and an image
// of before the names read as the same type (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::Reader;

    /// `t` written and read back.
    fn again(t: DataType) -> DataType {
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
            approximate(Spelled::Float),
            approximate(Spelled::Real),
            approximate(Spelled::DoublePrecision),
        ] {
            assert_eq!(again(t).to_string(), t.to_string());
        }
        assert_eq!(again(exact(Spelled::Integer)), exact(Spelled::Numeric), "one type, whatever its name");
        let mut before = vec![EXACT_TYPE];
        before.put_u32(10);
        before.put_u32(0);
        let read = data_type(&mut Reader::new(&before, "type")).unwrap();
        assert_eq!(read.to_string(), "NUMERIC(10,0)");
    }
}
