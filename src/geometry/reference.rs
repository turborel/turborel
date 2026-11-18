//! Optional column SRID constraints use the existing extension type
//! parameters (D53). Values still carry their own signed reference label;
//! a constrained descriptor permits one nonnegative label only.

use super::invalid;
use super::kind::{data_type_of, kind_of, of_data_type};
use super::model::Kind;
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::value::DataType;
use crate::value::types::{ExtensionType, TypeParams};

/// The descriptor encoding is independent of the geometry value payload.
const VERSION: u8 = 1;

/// An ST_GEOMETRY column constrained to one spatial reference identifier.
/// This does not transform coordinates or resolve the identifier to a CRS.
///
/// # Errors
///
/// SQLCODE -103 if `srid` exceeds the nonnegative signed 32-bit range.
pub fn data_type_with_srid(srid: u32) -> Result<DataType> {
    check_params(&[srid])?;
    let params = TypeParams::of(&[srid]).ok_or_else(bad_params)?;
    Ok(DataType::Extension(ExtensionType { kind: kind_of(Kind::Geometry), params }))
}

pub(super) fn parameters(t: &DataType) -> &[u32] {
    if let DataType::Extension(e) = t { e.params.values() } else { &[] }
}

pub(crate) fn fixed_srid(t: &DataType) -> Option<i32> {
    of_data_type(t)?;
    let DataType::Extension(e) = t else { return None };
    let [srid] = e.params.values() else { return None };
    i32::try_from(*srid).ok()
}

pub(super) fn check_params(params: &[u32]) -> Result<()> {
    match params {
        [] => Ok(()),
        [srid] => i32::try_from(*srid).map(|_| ()).map_err(|_| bad_params()),
        _ => Err(bad_params()),
    }
}

fn bad_params() -> SqlError {
    SqlError::new(Code::SyntaxRule, "[EXTENSION] GEOMETRY accepts at most one parameter: an SRID from 0 to 2147483647")
}

pub(super) fn check(srid: i32, t: &DataType) -> Result<()> {
    check_params(parameters(t))?;
    if let Some(fixed) = fixed_srid(t)
        && srid != fixed
    {
        return Err(invalid(&format!("SRID {srid} does not match the column's required SRID {fixed}")));
    }
    Ok(())
}

pub(super) fn put(t: &DataType, out: &mut Vec<u8>) {
    let params = parameters(t);
    out.put_u8(VERSION);
    out.put_u8(u8::try_from(params.len()).unwrap_or_default());
    for param in params {
        out.put_u32(*param);
    }
}

pub(super) fn read(r: &mut Reader<'_>, kind: Kind) -> Result<DataType> {
    if r.u8()? != VERSION {
        return Err(r.corrupt("unknown GEOMETRY descriptor version"));
    }
    match r.u8()? {
        0 => Ok(data_type_of(kind)),
        1 => {
            let srid = r.u32()?;
            check_params(&[srid]).map_err(|_| r.corrupt("invalid geometry SRID parameter"))?;
            let params = TypeParams::of(&[srid]).ok_or_else(|| r.corrupt("invalid geometry SRID parameter"))?;
            Ok(DataType::Extension(ExtensionType { kind: kind_of(kind), params }))
        }
        _ => Err(r.corrupt("invalid GEOMETRY parameter count")),
    }
}
