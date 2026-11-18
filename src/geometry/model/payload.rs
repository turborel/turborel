//! \[DESIGN\] A geometry value as it is stored (RULES.md D119):
//! the payload version, 1, its SRID as four bytes little-endian, then its
//! canonical well-known binary representation [ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §5.1.68]. Of the seven kinds D85 stored, of x and y, these
//! are the bytes D85 wrote, so a value stored before reads as it was.

use super::{Geom, decode, encode};
use crate::bytes::Reader;
use crate::error::{Code, Result, SqlError};
use crate::value::Value;

/// The payload's version: one format, unchanged while turborel is alpha.
pub const PAYLOAD_VERSION: u8 = 1;
/// The bytes of a stored SRID.
const SRID_BYTES: usize = 4;

/// A payload's SRID and its well-known binary bytes, checked no further.
///
/// # Errors
///
/// SQLCODE -916 of bytes that are no payload of version 1.
pub fn header(bytes: &[u8]) -> Result<(i32, &[u8])> {
    let mut r = Reader::new(bytes, "geometry");
    if r.u8()? != PAYLOAD_VERSION {
        return Err(r.corrupt("unknown geometry version"));
    }
    let srid: [u8; SRID_BYTES] = r.take(SRID_BYTES)?.try_into().map_err(|_| r.corrupt("short SRID"))?;
    Ok((i32::from_le_bytes(srid), r.rest()))
}

pub(super) fn encode(g: &Geom) -> Vec<u8> {
    let mut bytes = vec![PAYLOAD_VERSION];
    bytes.extend_from_slice(&g.srid.to_le_bytes());
    bytes.extend_from_slice(&encode::write_wkb(g));
    bytes
}

pub(super) fn decode(bytes: &[u8]) -> Result<Geom> {
    let (srid, wkb) = header(bytes)?;
    let (dims, body) = decode::read_canonical(wkb)
        .map_err(|e| SqlError::new(Code::Corrupt, format!("[DESIGN] an invalid or noncanonical stored geometry: {}", e.msg)))?;
    Ok(Geom { srid, dims, body })
}

pub(super) fn value(g: &Geom) -> Value {
    crate::geometry::kind::wrap(encode(g))
}

pub(super) fn geom(v: &Value) -> Result<Geom> {
    decode(crate::geometry::kind::payload(v).ok_or_else(|| crate::geometry::invalid("expected a geometry value"))?)
}
