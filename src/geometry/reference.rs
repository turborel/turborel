//! Optional column SRID constraints use the existing extension type
//! parameters (D53). Values still carry their own signed reference label;
//! a constrained descriptor permits one nonnegative label only.

use super::kind::{GEOMETRY_KIND, NAME};
use super::{Geometry, invalid};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::value::DataType;
use crate::value::types::{ExtensionType, TypeParams};

/// The descriptor encoding is independent of the geometry value payload.
const VERSION: u8 = 1;

/// A GEOMETRY column constrained to one spatial reference identifier.
/// This does not transform coordinates or resolve the identifier to a CRS.
///
/// # Errors
///
/// SQLCODE -103 if `srid` exceeds the nonnegative signed 32-bit range.
pub fn data_type_with_srid(srid: u32) -> Result<DataType> {
    check_params(&[srid])?;
    let params = TypeParams::of(&[srid]).ok_or_else(bad_params)?;
    Ok(DataType::Extension(ExtensionType { kind: &GEOMETRY_KIND, params }))
}

pub(super) fn parameters(t: &DataType) -> &[u32] {
    if let DataType::Extension(e) = t { e.params.values() } else { &[] }
}

pub(crate) fn fixed_srid(t: &DataType) -> Option<i32> {
    let DataType::Extension(e) = t else { return None };
    if e.kind.name() != NAME {
        return None;
    }
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

pub(super) fn check(g: &Geometry, t: &DataType) -> Result<()> {
    check_params(parameters(t))?;
    if let Some(srid) = fixed_srid(t)
        && g.srid() != srid
    {
        return Err(invalid(&format!("SRID {} does not match the column's required SRID {srid}", g.srid())));
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

pub(super) fn read(r: &mut Reader<'_>) -> Result<DataType> {
    if r.u8()? != VERSION {
        return Err(r.corrupt("unknown GEOMETRY descriptor version"));
    }
    match r.u8()? {
        0 => Ok(super::data_type()),
        1 => data_type_with_srid(r.u32()?).map_err(|_| r.corrupt("invalid GEOMETRY SRID parameter")),
        _ => Err(r.corrupt("invalid GEOMETRY parameter count")),
    }
}
