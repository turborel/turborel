//! \[DESIGN\] The stored form of a linear referencing value (RULES.md
//! D30): the version octet 1, the code of its most specific type
//! (`LrKind::code`), then its attributes [ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §15.1.1 to §15.13.1] in the order the struct of `lr/model.rs`
//! declares them.
//!
//! An integer is four octets little-endian, a DOUBLE PRECISION the eight
//! of its bits, a string its length then its UTF-8 (`crate::bytes`), an
//! attribute that may be the null value an octet 0 (null) or 1 then the
//! value, an ARRAY its cardinality then its elements, and a geometry its
//! payload (`Geom::encode`). A linear element is its own type's code then
//! its attributes. Bytes this did not write are SQLCODE -916, never a
//! panic, and a value nested past `MAX_DEPTH` is refused both ways.

use super::element::curve_value;
use super::model::{
    DistanceExp, ElementBase, LatOffsetExp, LinearElement, LrCurve, LrDirectedEdge, LrFeature, LrMeasure, Lrm, PositionExp, Referent,
    StartValue, Vector, VectorOffsetExp, VerOffsetExp,
};
use super::{LrKind, LrValue, deeper};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::geometry::model::Geom;

/// The version of the stored form this writes.
const VERSION: u8 = 1;
/// What a reader's errors name.
const WHAT: &str = "linear referencing value";
/// The octet of an attribute that is the null value.
const ABSENT: u8 = 0;
/// The octet of an attribute that is not.
const PRESENT: u8 = 1;

/// The stored form of `v`.
///
/// # Errors
///
/// SQLCODE -900 of a value nested past `MAX_DEPTH`.
pub(crate) fn encode(v: &LrValue) -> Result<Vec<u8>> {
    let mut out = vec![VERSION, v.kind().code()];
    match v {
        LrValue::Lrm(l) => put_lrm(&mut out, l),
        LrValue::LinearElement(e) => put_element_body(&mut out, e, 0)?,
        LrValue::PositionExp(p) => put_position(&mut out, p, 0)?,
        LrValue::LrMeasure(m) => put_measure(&mut out, m),
        LrValue::StartValue(s) => put_start(&mut out, s),
        LrValue::DistanceExp(d) => put_distance(&mut out, d),
        LrValue::Referent(r) => put_referent(&mut out, r, 0)?,
        LrValue::LatOffsetExp(o) => put_offset(&mut out, o.distance.as_ref(), o.geometry.as_ref(), o.description.as_deref()),
        LrValue::VerOffsetExp(o) => put_offset(&mut out, o.distance.as_ref(), o.geometry.as_ref(), o.description.as_deref()),
        LrValue::VectorOffsetExp(o) => put_vectors(&mut out, o),
    }
    Ok(out)
}

/// The value `encode` wrote.
///
/// # Errors
///
/// SQLCODE -916 of bytes `encode` did not write.
pub(crate) fn decode(bytes: &[u8]) -> Result<LrValue> {
    let mut r = Reader::new(bytes, WHAT);
    if r.u8()? != VERSION {
        return Err(r.corrupt("its version is not 1"));
    }
    let kind = kind_of(&mut r)?;
    let v = match kind {
        LrKind::Lrm => LrValue::Lrm(get_lrm(&mut r)?),
        LrKind::LinearElement => return Err(r.corrupt("ST_LinearElement is not instantiable")),
        LrKind::LrFeature | LrKind::LrCurve | LrKind::LrDirectedEdge => LrValue::LinearElement(get_element_body(&mut r, kind, 0)?),
        LrKind::PositionExp => LrValue::PositionExp(get_position(&mut r, 0)?),
        LrKind::LrMeasure => LrValue::LrMeasure(get_measure(&mut r)?),
        LrKind::StartValue => LrValue::StartValue(get_start(&mut r)?),
        LrKind::DistanceExp => LrValue::DistanceExp(get_distance(&mut r)?),
        LrKind::Referent => LrValue::Referent(get_referent(&mut r, 0)?),
        LrKind::LatOffsetExp => {
            let (distance, geometry, description) = get_offset(&mut r)?;
            LrValue::LatOffsetExp(LatOffsetExp { distance, geometry, description })
        }
        LrKind::VerOffsetExp => {
            let (distance, geometry, description) = get_offset(&mut r)?;
            LrValue::VerOffsetExp(VerOffsetExp { distance, geometry, description })
        }
        LrKind::VectorOffsetExp => LrValue::VectorOffsetExp(get_vectors(&mut r)?),
    };
    r.finish()?;
    Ok(v)
}

// ---------------------------------------------------------------------------
// Fields
// ---------------------------------------------------------------------------

/// An INTEGER.
fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.put_u32(u32::from_le_bytes(v.to_le_bytes()));
}

/// The INTEGER `put_i32` wrote.
fn get_i32(r: &mut Reader<'_>) -> Result<i32> {
    Ok(i32::from_le_bytes(r.u32()?.to_le_bytes()))
}

/// A DOUBLE PRECISION.
fn put_f64(out: &mut Vec<u8>, v: f64) {
    out.put_u64(v.to_bits());
}

/// The DOUBLE PRECISION `put_f64` wrote.
fn get_f64(r: &mut Reader<'_>) -> Result<f64> {
    Ok(f64::from_bits(r.u64()?))
}

/// Whether an attribute that may be the null value is not.
fn put_present(out: &mut Vec<u8>, present: bool) {
    out.put_u8(if present { PRESENT } else { ABSENT });
}

/// Whether `put_present` wrote that an attribute is not the null value.
fn get_present(r: &mut Reader<'_>) -> Result<bool> {
    match r.u8()? {
        ABSENT => Ok(false),
        PRESENT => Ok(true),
        _ => Err(r.corrupt("a presence octet is neither 0 nor 1")),
    }
}

/// A string that may be the null value.
fn put_opt_str(out: &mut Vec<u8>, v: Option<&str>) {
    put_present(out, v.is_some());
    if let Some(s) = v {
        out.put_str(s);
    }
}

/// The string `put_opt_str` wrote.
fn get_opt_str(r: &mut Reader<'_>) -> Result<Option<String>> {
    if get_present(r)? { r.str().map(Some) } else { Ok(None) }
}

/// A DOUBLE PRECISION that may be the null value.
fn put_opt_f64(out: &mut Vec<u8>, v: Option<f64>) {
    put_present(out, v.is_some());
    if let Some(f) = v {
        put_f64(out, f);
    }
}

/// The DOUBLE PRECISION `put_opt_f64` wrote.
fn get_opt_f64(r: &mut Reader<'_>) -> Result<Option<f64>> {
    if get_present(r)? { get_f64(r).map(Some) } else { Ok(None) }
}

/// A geometry that may be the null value.
fn put_opt_geom(out: &mut Vec<u8>, v: Option<&Geom>) {
    put_present(out, v.is_some());
    if let Some(g) = v {
        out.put_bytes(&g.encode());
    }
}

/// The geometry `put_opt_geom` wrote.
fn get_opt_geom(r: &mut Reader<'_>) -> Result<Option<Geom>> {
    if get_present(r)? { Geom::decode(r.bytes()?).map(Some) } else { Ok(None) }
}

/// The cardinality of an ARRAY.
fn put_count(out: &mut Vec<u8>, n: usize) {
    out.put_u32(u32::try_from(n).unwrap_or(u32::MAX));
}

/// The cardinality `put_count` wrote, bounded by the bytes left, as each
/// element takes at least one.
fn get_count(r: &mut Reader<'_>) -> Result<usize> {
    usize::try_from(r.u32()?).map_err(|_| r.corrupt("a cardinality is too large"))
}

/// The type of the code read next.
fn kind_of(r: &mut Reader<'_>) -> Result<LrKind> {
    let code = r.u8()?;
    LrKind::of_code(code).ok_or_else(|| r.corrupt("a type code names no linear referencing type"))
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// [§15.1.1] An ST_LRM.
fn put_lrm(out: &mut Vec<u8>, l: &Lrm) {
    put_i32(out, l.id);
    out.put_str(&l.name);
    out.put_str(&l.lrm_type);
    out.put_str(&l.units);
    put_present(out, l.constraints.is_some());
    if let Some(cs) = &l.constraints {
        put_count(out, cs.len());
        for c in cs {
            out.put_str(c);
        }
    }
    put_opt_str(out, l.offset_units.as_deref());
    put_opt_str(out, l.lateral_direction.as_deref());
    put_opt_str(out, l.vertical_direction.as_deref());
}

/// The ST_LRM `put_lrm` wrote.
fn get_lrm(r: &mut Reader<'_>) -> Result<Lrm> {
    let (id, name, lrm_type, units) = (get_i32(r)?, r.str()?, r.str()?, r.str()?);
    let constraints = if get_present(r)? {
        let n = get_count(r)?;
        Some((0..n).map(|_| r.str()).collect::<Result<Vec<_>>>()?)
    } else {
        None
    };
    Ok(Lrm {
        id,
        name,
        lrm_type,
        units,
        constraints,
        offset_units: get_opt_str(r)?,
        lateral_direction: get_opt_str(r)?,
        vertical_direction: get_opt_str(r)?,
    })
}

/// [§15.7.1] An ST_LRMeasure.
fn put_measure(out: &mut Vec<u8>, m: &LrMeasure) {
    put_f64(out, m.measure);
    put_opt_str(out, m.units.as_deref());
}

/// The ST_LRMeasure `put_measure` wrote.
fn get_measure(r: &mut Reader<'_>) -> Result<LrMeasure> {
    Ok(LrMeasure { measure: get_f64(r)?, units: get_opt_str(r)? })
}

/// [§15.8.1] An ST_StartValue.
fn put_start(out: &mut Vec<u8>, s: &StartValue) {
    put_i32(out, s.lrm);
    put_measure(out, &s.measure);
}

/// The ST_StartValue `put_start` wrote.
fn get_start(r: &mut Reader<'_>) -> Result<StartValue> {
    Ok(StartValue { lrm: get_i32(r)?, measure: get_measure(r)? })
}

/// [§15.2.1 to §15.5.1] A linear element: its type's code, then its
/// attributes.
fn put_element_body(out: &mut Vec<u8>, e: &LinearElement, depth: usize) -> Result<()> {
    let next = deeper(depth)?;
    let b = e.base();
    put_i32(out, b.id);
    put_i32(out, b.default_lrm);
    put_measure(out, &b.default_measure);
    put_opt_str(out, b.element_type.as_deref());
    put_present(out, b.start_values.is_some());
    if let Some(svs) = &b.start_values {
        put_count(out, svs.len());
        for s in svs {
            put_start(out, s);
        }
    }
    match e {
        LinearElement::Feature(f) => {
            out.put_str(&f.feature_id);
            put_present(out, f.referents.is_some());
            if let Some(rs) = &f.referents {
                put_count(out, rs.len());
                for referent in rs {
                    put_referent(out, referent, next)?;
                }
            }
        }
        LinearElement::Curve(c) => out.put_bytes(&c.curve.encode()),
        LinearElement::Edge(d) => {
            out.put_u32(u32::from(d.topology_type));
            out.put_str(&d.topology_name);
            put_i32(out, d.edge_id);
        }
    }
    Ok(())
}

/// The linear element of type `kind` `put_element_body` wrote.
fn get_element_body(r: &mut Reader<'_>, kind: LrKind, depth: usize) -> Result<LinearElement> {
    let next = deeper(depth).map_err(|_| r.corrupt("it is nested too deep"))?;
    let (id, default_lrm, default_measure, element_type) = (get_i32(r)?, get_i32(r)?, get_measure(r)?, get_opt_str(r)?);
    let start_values = if get_present(r)? {
        let n = get_count(r)?;
        Some((0..n).map(|_| get_start(r)).collect::<Result<Vec<_>>>()?)
    } else {
        None
    };
    let base = ElementBase { id, default_lrm, default_measure, element_type, start_values };
    get_subtype(r, kind, base, next)
}

/// The linear element of type `kind` and attributes `base` whose own
/// attributes `put_element_body` wrote next.
fn get_subtype(r: &mut Reader<'_>, kind: LrKind, base: ElementBase, next: usize) -> Result<LinearElement> {
    match kind {
        LrKind::LrFeature => {
            let feature_id = r.str()?;
            let referents = if get_present(r)? {
                let n = get_count(r)?;
                Some((0..n).map(|_| get_referent(r, next)).collect::<Result<Vec<_>>>()?)
            } else {
                None
            };
            Ok(LinearElement::Feature(LrFeature { base, feature_id, referents }))
        }
        LrKind::LrCurve => {
            let curve = curve_value(Geom::decode(r.bytes()?)?).map_err(|_| r.corrupt("the curve is not an ST_Curve"))?;
            Ok(LinearElement::Curve(LrCurve { base, curve }))
        }
        LrKind::LrDirectedEdge => {
            let topology_type = char::from_u32(r.u32()?).ok_or_else(|| r.corrupt("a topology type is no character"))?;
            Ok(LinearElement::Edge(LrDirectedEdge { base, topology_type, topology_name: r.str()?, edge_id: get_i32(r)? }))
        }
        LrKind::Lrm
        | LrKind::LinearElement
        | LrKind::PositionExp
        | LrKind::LrMeasure
        | LrKind::StartValue
        | LrKind::DistanceExp
        | LrKind::Referent
        | LrKind::LatOffsetExp
        | LrKind::VerOffsetExp
        | LrKind::VectorOffsetExp => Err(r.corrupt("a linear element's type code names no instantiable subtype")),
    }
}

/// [§15.6.1] An ST_PositionExp.
fn put_position(out: &mut Vec<u8>, p: &PositionExp, depth: usize) -> Result<()> {
    let next = deeper(depth)?;
    put_i32(out, p.element_id);
    put_present(out, p.element.is_some());
    if let Some(e) = &p.element {
        out.put_u8(e.kind().code());
        put_element_body(out, e, next)?;
    }
    put_i32(out, p.lrm_id);
    put_present(out, p.lrm.is_some());
    if let Some(l) = &p.lrm {
        put_lrm(out, l);
    }
    put_distance(out, &p.distance);
    Ok(())
}

/// The ST_PositionExp `put_position` wrote.
fn get_position(r: &mut Reader<'_>, depth: usize) -> Result<PositionExp> {
    let next = deeper(depth).map_err(|_| r.corrupt("it is nested too deep"))?;
    let element_id = get_i32(r)?;
    let element = if get_present(r)? {
        let kind = kind_of(r)?;
        Some(Box::new(get_element_body(r, kind, next)?))
    } else {
        None
    };
    let lrm_id = get_i32(r)?;
    let lrm = if get_present(r)? { Some(get_lrm(r)?) } else { None };
    Ok(PositionExp { element_id, element, lrm_id, lrm, distance: get_distance(r)? })
}

/// [§15.9.1] An ST_DistanceExp.
fn put_distance(out: &mut Vec<u8>, d: &DistanceExp) {
    put_measure(out, &d.along);
    put_opt_str(out, d.from_feature.as_deref());
    put_opt_str(out, d.from_name.as_deref());
    put_opt_str(out, d.towards_feature.as_deref());
    put_opt_str(out, d.towards_name.as_deref());
    put_present(out, d.lateral.is_some());
    if let Some(o) = &d.lateral {
        put_offset(out, o.distance.as_ref(), o.geometry.as_ref(), o.description.as_deref());
    }
    put_present(out, d.vertical.is_some());
    if let Some(o) = &d.vertical {
        put_offset(out, o.distance.as_ref(), o.geometry.as_ref(), o.description.as_deref());
    }
    put_present(out, d.vector.is_some());
    if let Some(o) = &d.vector {
        put_vectors(out, o);
    }
}

/// The ST_DistanceExp `put_distance` wrote.
fn get_distance(r: &mut Reader<'_>) -> Result<DistanceExp> {
    let along = get_measure(r)?;
    let (from_feature, from_name, towards_feature, towards_name) = (get_opt_str(r)?, get_opt_str(r)?, get_opt_str(r)?, get_opt_str(r)?);
    let lateral = if get_present(r)? {
        let (distance, geometry, description) = get_offset(r)?;
        Some(LatOffsetExp { distance, geometry, description })
    } else {
        None
    };
    let vertical = if get_present(r)? {
        let (distance, geometry, description) = get_offset(r)?;
        Some(VerOffsetExp { distance, geometry, description })
    } else {
        None
    };
    let vector = if get_present(r)? { Some(get_vectors(r)?) } else { None };
    Ok(DistanceExp { along, from_feature, from_name, towards_feature, towards_name, lateral, vertical, vector })
}

/// [§15.10.1] An ST_Referent.
fn put_referent(out: &mut Vec<u8>, referent: &Referent, depth: usize) -> Result<()> {
    let next = deeper(depth)?;
    out.put_str(&referent.name);
    out.put_str(&referent.referent_type);
    put_opt_geom(out, referent.position.as_ref());
    put_present(out, referent.location.is_some());
    if let Some(p) = &referent.location {
        put_position(out, p, next)?;
    }
    Ok(())
}

/// The ST_Referent `put_referent` wrote.
fn get_referent(r: &mut Reader<'_>, depth: usize) -> Result<Referent> {
    let next = deeper(depth).map_err(|_| r.corrupt("it is nested too deep"))?;
    let (name, referent_type, position) = (r.str()?, r.str()?, get_opt_geom(r)?);
    let location = if get_present(r)? { Some(Box::new(get_position(r, next)?)) } else { None };
    Ok(Referent { name, referent_type, position, location })
}

/// [§15.11.1, §15.12.1] An ST_LatOffsetExp or ST_VerOffsetExp, of the
/// same attributes.
fn put_offset(out: &mut Vec<u8>, distance: Option<&LrMeasure>, geometry: Option<&Geom>, description: Option<&str>) {
    put_present(out, distance.is_some());
    if let Some(m) = distance {
        put_measure(out, m);
    }
    put_opt_geom(out, geometry);
    put_opt_str(out, description);
}

/// The attributes `put_offset` wrote.
fn get_offset(r: &mut Reader<'_>) -> Result<(Option<LrMeasure>, Option<Geom>, Option<String>)> {
    let distance = if get_present(r)? { Some(get_measure(r)?) } else { None };
    Ok((distance, get_opt_geom(r)?, get_opt_str(r)?))
}

/// [§15.13.1] An ST_VectorOffsetExp.
fn put_vectors(out: &mut Vec<u8>, o: &VectorOffsetExp) {
    put_count(out, o.vectors.len());
    for v in &o.vectors {
        put_opt_f64(out, v.x);
        put_opt_f64(out, v.y);
        put_opt_f64(out, v.z);
    }
}

/// The ST_VectorOffsetExp `put_vectors` wrote.
fn get_vectors(r: &mut Reader<'_>) -> Result<VectorOffsetExp> {
    let n = get_count(r)?;
    if n > super::MAX_VECTORS {
        return Err(r.corrupt("it has more than three vectors"));
    }
    let vectors = (0..n).map(|_| get_vector(r)).collect::<Result<Vec<_>>>()?;
    Ok(VectorOffsetExp { vectors })
}

/// One vector `put_vectors` wrote.
fn get_vector(r: &mut Reader<'_>) -> Result<Vector> {
    Ok(Vector { x: get_opt_f64(r)?, y: get_opt_f64(r)?, z: get_opt_f64(r)? })
}
