//! The GML reader's own checks [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §15.1.12 and its peers]: documents the writer would not write, made by
//! changing one it wrote, raise 2FF24 or the constructor's exception.

use super::*;
use crate::spatial::formats::gml::write_geom;

/// The GML of `v`, with `from` replaced by `to` once or more.
fn changed(v: &LrValue, from: &str, to: &str) -> String {
    let gml = as_gml(v, &NoSrs).unwrap();
    assert!(gml.contains(from), "{gml} lacks {from}");
    gml.replace(from, to)
}

/// The SQLSTATE reading `gml` as a value of `kind` raises.
fn refused(gml: &str, kind: LrKind) -> SqlState {
    let all = units();
    state(from_gml(gml, kind, &Catalog::of_units(&all), &NoSrs))
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.13.1, §15.1.12
/// and its peers] A vector offset expression of four vectors, past the
/// three of §15.13.1, is invalid GML (2FF24).
#[test]
fn four_vectors_are_refused_in_gml() {
    let one = VectorOffsetExp::new(vec![Vector::xy(1.0_f64, 2.0_f64)]).unwrap();
    let gml = as_gml(&LrValue::VectorOffsetExp(one), &NoSrs).unwrap();
    let (head, rest) = gml.split_once("<Vector>").unwrap();
    let (inner, tail) = rest.split_once("</Vector>").unwrap();
    let four = format!("{head}{}{tail}", format!("<Vector>{inner}</Vector>").repeat(4));
    assert_eq!(refused(&four, LrKind::VectorOffsetExp), SqlState::SPATIAL_INVALID_GML);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.6.10, §15.2.1]
/// A position expression's linear element named for no subtype of
/// ST_LinearElement is invalid GML (2FF24).
#[test]
fn an_element_of_no_subtype_is_refused_in_gml() {
    let p = LrValue::PositionExp(PositionExp::of_element_and_id(LinearElement::Curve(road()), 1, DistanceExp::along(m(3.0_f64))));
    let gml = changed(&p, "LRCurve>", "LRRoad>");
    assert_eq!(refused(&gml, LrKind::PositionExp), SqlState::SPATIAL_INVALID_GML);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.10.1, §15.9.12]
/// A referent positioned at a geometry that is no point is invalid GML
/// (2FF24).
#[test]
fn a_referent_at_no_point_is_refused_in_gml() {
    let all = units();
    let r =
        Referent::new(Some("r".to_owned()), Some("t".to_owned()), Some(point(1.0_f64, 1.0_f64)), None, &Catalog::of_units(&all)).unwrap();
    let at = write_geom(&point(1.0_f64, 1.0_f64), &NoSrs).unwrap();
    let curve = write_geom(&line(&[0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64], Dims::Xy), &NoSrs).unwrap();
    let gml = changed(&LrValue::Referent(r), &at, &curve);
    assert_eq!(refused(&gml, LrKind::Referent), SqlState::SPATIAL_INVALID_GML);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.11.1, §15.11.2
/// Description 4] An offset with both a feature geometry and an offset
/// referent description is invalid GML (2FF24).
#[test]
fn an_offset_of_geometry_and_description_is_refused_in_gml() {
    let o = LrValue::LatOffsetExp(LatOffsetExp::with_description(m(2.0_f64), "kerb".to_owned()));
    let geometry =
        format!("<featureGeometry>{}</featureGeometry><offsetReferentDescription>", write_geom(&point(1.0_f64, 1.0_f64), &NoSrs).unwrap());
    let gml = changed(&o, "<offsetReferentDescription>", &geometry);
    assert_eq!(refused(&gml, LrKind::LatOffsetExp), SqlState::SPATIAL_INVALID_GML);
}
