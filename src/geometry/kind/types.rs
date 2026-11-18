//! The geometry types through the SqlType contract (RULES.md D53), and
//! their family (RULES.md D-SPATIAL-FAMILY).

use std::cmp::Ordering;
use std::sync::Arc;

use super::super::model::{self, Geom, Kind};
use super::super::{Geometry, UNSPECIFIED_SRID, invalid, reference};
use super::{FAMILY, GEOMETRY, Geometries, GeometryType, check, geometry, geometry_kind, kind_of, kinds, most_specific, payload};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::value::types::family::Family;
use crate::value::types::{Descriptor, ExtensionValue, Kind as TypeKind};
use crate::value::{Bucket, DataType, SqlType, Value};

impl GeometryType {
    /// That `v`, a geometry value, is a value of this type and of the SRID
    /// `t` fixes, if it fixes one.
    fn fits(&self, v: &Value, t: &DataType) -> Result<()> {
        let bytes = payload(v).ok_or_else(|| invalid("expected a geometry value"))?;
        let (srid, _) = model::header(bytes)?;
        reference::check(srid, t)?;
        let kind = most_specific(bytes).ok_or_else(|| invalid("the value's type code names no geometry type"))?;
        if !kind.is_subtype_of(self.kind) {
            return Err(SqlError::new(
                Code::Data,
                format!(
                    "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2] a value of {} is not a value of {}",
                    kind.sql_name(),
                    self.kind.sql_name()
                ),
            )
            .with_state(crate::error::SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE));
        }
        Ok(())
    }

    /// A value of payload `bytes`, checked as stored.
    fn stored(&self, bytes: &[u8], t: &DataType) -> Result<Value> {
        check(bytes)?;
        let v = Value::Extension(ExtensionValue { kind: GEOMETRY, bytes: Arc::from(bytes) });
        self.fits(&v, t)?;
        Ok(v)
    }
}

/// SQLCODE -900: no key is of `t`.
fn unkeyed(t: &DataType) -> SqlError {
    SqlError::new(
        Code::Unsupported,
        format!(
            "[DESIGN] no key is of {t}, ordered by RELATIVE with ST_OrderingEquals: its values have no key of their own (RULES.md D81, D-SPATIAL-FAMILY)"
        ),
    )
}

impl SqlType for GeometryType {
    fn name(&self) -> &str {
        self.kind.type_name()
    }

    fn family(&self) -> Option<&'static dyn Family> {
        Some(&FAMILY)
    }

    fn written(&self, t: &DataType) -> String {
        reference::fixed_srid(t).map_or_else(|| self.kind.type_name().to_owned(), |srid| format!("{}({srid})", self.kind.type_name()))
    }

    fn comparable(&self, other: &DataType) -> bool {
        super::of_data_type(other).is_some()
    }

    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(payload(a)?.cmp(payload(b)?))
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Extension(payload(v)?.into()))
    }

    fn assign(&self, v: Value, t: &DataType, _: &str) -> Result<Value> {
        self.fits(&v, t)?;
        Ok(v)
    }

    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        if v.is_null() {
            return Ok(v);
        }
        reference::check_params(reference::parameters(t))?;
        let g = if let Value::Char(text) = &v {
            let srid = reference::fixed_srid(t).unwrap_or(UNSPECIFIED_SRID);
            Geom::from_value(&Geometry::from_wkt(text.trim_end_matches(' '), srid)?.to_value())?
        } else {
            Geom::from_value(&v)?
        };
        let cast = model::cast(&g, self.kind)?;
        reference::check(cast.srid, t)?;
        Ok(if cast == g && !matches!(v, Value::Char(_)) { v } else { cast.to_value() })
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
        Descriptor::of(self.kind.type_name())
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(payload(v).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let raw = r.bytes()?;
        check(raw).map_err(|_| r.corrupt("invalid geometry payload"))?;
        Ok(Value::Extension(ExtensionValue { kind: GEOMETRY, bytes: Arc::from(raw) }))
    }

    fn check_params(&self, params: &[u32]) -> Result<()> {
        reference::check_params(params)
    }

    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        reference::put(t, out);
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        reference::read(r, self.kind)
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        self.fits(v, t)?;
        Ok(payload(v).ok_or_else(|| invalid("expected a geometry value"))?.to_vec())
    }

    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        self.stored(field, t).ok()
    }

    /// None: values ordered by RELATIVE have no key of their own, so no
    /// B-tree, unique constraint or referential constraint is of them
    /// (RULES.md D81, D-SPATIAL-FAMILY); an R-tree indexes their
    /// envelopes (D85).
    fn encode_key(&self, _: &Value, t: &DataType, _: &mut Vec<u8>) -> Result<()> {
        Err(unkeyed(t))
    }

    fn decode_key(&self, _: &mut &[u8], t: &DataType) -> Result<Value> {
        Err(unkeyed(t))
    }

    fn anchor(&self, v: &Value, t: &DataType) -> Option<(Value, bool)> {
        self.fits(v, t).ok().map(|()| (v.clone(), true))
    }
}

impl Family for Geometries {
    fn is_subtype(&self, sub: &dyn SqlType, sup: &dyn SqlType) -> bool {
        geometry_kind(sub).zip(geometry_kind(sup)).is_some_and(|(a, b)| a.is_subtype_of(b))
    }

    fn named(&self, name: &str) -> Option<TypeKind> {
        Kind::named(name).map(kind_of)
    }

    fn most_specific(&self, v: &Value) -> Option<TypeKind> {
        most_specific(payload(v)?).map(kind_of)
    }

    fn supertype(&self, kind: &dyn SqlType) -> Option<TypeKind> {
        geometry_kind(kind)?.supertype().map(kind_of)
    }

    fn instantiable(&self, kind: &dyn SqlType) -> bool {
        geometry_kind(kind).is_some_and(Kind::is_instantiable)
    }

    fn relative_ordering(&self) -> &'static str {
        super::super::ORDERING_EQUALS
    }

    fn kinds(&self) -> &'static [TypeKind] {
        kinds()
    }
}
