//! \[EXTENSION\] GEOMETRY through the existing SqlType contract (D53).
//! Its payload is version 1, an i32 SRID, then canonical little-endian
//! WKB. SQL comparison orders these bytes; it is not topological equality.

use std::cmp::Ordering;
use std::sync::Arc;

use super::{Geometry, UNSPECIFIED_SRID, invalid, reference};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::value::types::{Descriptor, ExtensionType, ExtensionValue, TypeParams};
use crate::value::{Binary, Bucket, DataType, SqlType, Value};

/// The registered SQL type name.
pub(super) const NAME: &str = "GEOMETRY";
/// Stored geometry payload version (RULES.md D85).
const VERSION: u8 = 1;
/// The bytes in a stored SRID.
const SRID_BYTES: usize = 4;

/// The GEOMETRY plug-in type. Register before opening files that use it.
/// An optional nonnegative SRID constrains values assigned to a column.
/// It does not imply SQL type inheritance.
#[derive(Debug)]
pub struct GeometryType;

/// The kind used by geometry values.
pub static GEOMETRY: GeometryType = GeometryType;
pub(super) static GEOMETRY_KIND: &dyn SqlType = &GEOMETRY;

/// The unparameterized GEOMETRY type descriptor.
pub fn data_type() -> DataType {
    DataType::Extension(ExtensionType { kind: &GEOMETRY_KIND, params: TypeParams::default() })
}

pub(super) fn value(g: &Geometry) -> Value {
    let mut bytes = vec![VERSION];
    bytes.extend_from_slice(&g.srid().to_le_bytes());
    bytes.extend_from_slice(&g.to_wkb());
    Value::Extension(ExtensionValue { kind: &GEOMETRY, bytes: bytes.into() })
}

fn bytes(v: &Value) -> Option<&[u8]> {
    let Value::Extension(e) = v else { return None };
    (e.kind.name() == NAME).then_some(e.bytes.as_ref())
}

pub(super) fn geometry(v: &Value) -> Result<Geometry> {
    decode(bytes(v).ok_or_else(|| invalid("expected a GEOMETRY value"))?)
}

fn decode(bytes: &[u8]) -> Result<Geometry> {
    let mut r = Reader::new(bytes, "geometry");
    if r.u8()? != VERSION {
        return Err(r.corrupt("unknown geometry version"));
    }
    let srid: [u8; SRID_BYTES] = r.take(SRID_BYTES)?.try_into().map_err(|_| r.corrupt("short SRID"))?;
    Geometry::from_canonical_wkb(r.rest(), i32::from_le_bytes(srid)).map_err(|_| r.corrupt("invalid or noncanonical geometry WKB"))
}

impl SqlType for GeometryType {
    fn name(&self) -> &'static str {
        NAME
    }

    fn written(&self, t: &DataType) -> String {
        reference::fixed_srid(t).map_or_else(|| NAME.to_owned(), |srid| format!("{NAME}({srid})"))
    }

    fn comparable(&self, other: &DataType) -> bool {
        if let DataType::Extension(e) = other { e.kind.name() == NAME } else { false }
    }

    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(bytes(a)?.cmp(bytes(b)?))
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Extension(bytes(v)?.into()))
    }

    fn assign(&self, v: Value, t: &DataType, _: &str) -> Result<Value> {
        reference::check(&geometry(&v)?, t)?;
        Ok(v)
    }

    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        if v.is_null() {
            return Ok(v);
        }
        if let Value::Char(text) = &v {
            reference::check_params(reference::parameters(t))?;
            let srid = reference::fixed_srid(t).unwrap_or(UNSPECIFIED_SRID);
            return Geometry::from_wkt(text.trim_end_matches(' '), srid).map(|g| value(&g));
        }
        reference::check(&geometry(&v)?, t)?;
        Ok(v)
    }

    fn literal(&self, v: &Value) -> String {
        geometry(v).map_or_else(|_| String::new(), |g| format!("ST_GEOMFROMTEXT('{}', {})", g.to_wkt(), g.srid()))
    }

    fn text(&self, v: &Value) -> String {
        geometry(v).map_or_else(|_| String::new(), |g| g.to_wkt())
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(NAME)
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(bytes(v).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let raw = r.bytes()?;
        decode(raw).map_err(|_| r.corrupt("invalid geometry payload"))?;
        Ok(Value::Extension(ExtensionValue { kind: &GEOMETRY, bytes: Arc::from(raw) }))
    }

    fn check_params(&self, params: &[u32]) -> Result<()> {
        reference::check_params(params)
    }

    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        reference::put(t, out);
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        reference::read(r)
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        reference::check(&geometry(v)?, t)?;
        Ok(bytes(v).ok_or_else(|| invalid("expected GEOMETRY"))?.to_vec())
    }

    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        reference::check(&decode(field).ok()?, t).ok()?;
        Some(Value::Extension(ExtensionValue { kind: &GEOMETRY, bytes: Arc::from(field) }))
    }

    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        reference::check(&geometry(v)?, t)?;
        Binary::new(bytes(v).ok_or_else(|| invalid("expected GEOMETRY"))?.to_vec()).put_key(out);
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], t: &DataType) -> Result<Value> {
        let b = Binary::take_key(rest).ok_or_else(|| SqlError::new(Code::Corrupt, "[DESIGN] invalid GEOMETRY index key"))?;
        reference::check(&decode(b.bytes())?, t)
            .map_err(|_| SqlError::new(Code::Corrupt, "[DESIGN] a GEOMETRY index key violates its SRID constraint"))?;
        Ok(Value::Extension(ExtensionValue { kind: &GEOMETRY, bytes: Arc::from(b.bytes()) }))
    }

    fn anchor(&self, v: &Value, t: &DataType) -> Option<(Value, bool)> {
        reference::check(&geometry(v).ok()?, t).ok().map(|()| (v.clone(), true))
    }
}
