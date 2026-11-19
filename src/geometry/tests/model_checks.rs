//! The checks a value of the model passes when it is made: each part of
//! each type well formed, each refusal its SQLSTATE.

use crate::error::SqlState;
use crate::geometry::model::{
    Body, Curve, Dims, Geom, Kind, Knot, Nurbs, NurbsPoint, Surface, Tin, TinElement, read_wkb, read_wkb_typed, write_wkb,
};

fn refused(dims: Dims, body: Body) -> Option<SqlState> {
    Geom::new(0, dims, body).unwrap_err().state
}

const INVALID: Option<SqlState> = Some(SqlState::SPATIAL_INVALID_ARGUMENT);
const EMPTY_ELEMENT: Option<SqlState> = Some(SqlState::SPATIAL_ELEMENT_IS_AN_EMPTY_SET);

fn triangle() -> Vec<f64> {
    vec![0.0_f64, 0.0_f64, 1.0_f64, 0.0_f64, 0.0_f64, 1.0_f64, 0.0_f64, 0.0_f64]
}

fn tin(elements: Vec<TinElement>, max_side_length: Option<f64>) -> Body {
    Body::Surface(Surface::Tin(Tin { triangles: vec![triangle()], elements, max_side_length }))
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §6.1, §7.10, §8.2,
/// §8.5, §8.7, §9.2] a point of other than one ordinate per dimension is
/// 2FF02; a compound curve, a polyhedral surface, a compound surface of
/// an empty element 2FF06; a curve polygon of an empty ring 2FF07 and of
/// a ring not closed 2FF02; a solid of an empty shell 2FF98.
#[test]
fn malformed_parts_are_refused_with_their_states() {
    assert_eq!(refused(Dims::Xy, Body::Point(vec![1.0_f64, 2.0_f64, 3.0_f64])), INVALID);
    let compound = Curve::Compound(vec![Curve::LineString(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64]), Curve::LineString(Vec::new())]);
    assert_eq!(refused(Dims::Xy, Body::Curve(compound)), EMPTY_ELEMENT);
    assert_eq!(refused(Dims::Xy, Body::Surface(Surface::PolyhdrlSurface(vec![vec![triangle()], Vec::new()]))), EMPTY_ELEMENT);
    let compound_surface = Surface::Compound(vec![Surface::Polygon(vec![triangle()]), Surface::Polygon(Vec::new())]);
    assert_eq!(refused(Dims::Xy, Body::Surface(compound_surface)), EMPTY_ELEMENT);
    let empty_ring = Surface::CurvePolygon(vec![Curve::LineString(Vec::new())]);
    assert_eq!(refused(Dims::Xy, Body::Surface(empty_ring)), Some(SqlState::SPATIAL_NULL_EXTERIOR_RING));
    let open = Surface::CurvePolygon(vec![Curve::LineString(vec![0.0_f64, 0.0_f64, 1.0_f64, 0.0_f64, 0.0_f64, 1.0_f64, 0.5_f64, 0.5_f64])]);
    assert_eq!(refused(Dims::Xy, Body::Surface(open)), INVALID);
    assert_eq!(refused(Dims::Xyz, Body::Solid(vec![Surface::PolyhdrlSurface(Vec::new())])), Some(SqlState::SPATIAL_NULL_EXTERIOR_SHELL));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.7, §8.6, §17.1]
/// a NURBS curve has control points and positive weights; a TIN's
/// maximum side length is finite and positive; a TIN element's type is 1
/// to 30 letters and its tag at most 64 characters; each else 2FF02.
#[test]
fn nurbs_curves_and_tins_are_checked() {
    let knots = vec![Knot { value: 0.0_f64, multiplicity: 2 }, Knot { value: 1.0_f64, multiplicity: 2 }];
    let point = |x: f64, weight: Option<f64>| NurbsPoint { point: vec![x, 0.0_f64], weight };
    let nurbs =
        |points: Vec<NurbsPoint>| Body::Curve(Curve::Nurbs(Some(Nurbs { degree: 1, points, knots: knots.clone(), measures: None })));
    assert!(Geom::new(0, Dims::Xy, nurbs(vec![point(0.0_f64, None), point(1.0_f64, None)])).is_ok());
    assert_eq!(refused(Dims::Xy, nurbs(Vec::new())), INVALID);
    assert_eq!(refused(Dims::Xy, nurbs(vec![point(0.0_f64, Some(-1.0_f64)), point(1.0_f64, Some(1.0_f64))])), INVALID);
    assert!(Geom::new(0, Dims::Xy, tin(Vec::new(), Some(2.0_f64))).is_ok());
    assert_eq!(refused(Dims::Xy, tin(Vec::new(), Some(-1.0_f64))), INVALID);
    assert_eq!(refused(Dims::Xy, tin(Vec::new(), Some(f64::INFINITY))), INVALID);
    let element = |element_type: &str, tag: &str| TinElement {
        element_type: element_type.to_owned(),
        id: 1,
        tag: tag.to_owned(),
        geometry: Box::new(Body::Curve(Curve::LineString(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64]))),
    };
    assert!(Geom::new(0, Dims::Xy, tin(vec![element("stop line", "t")], None)).is_ok());
    assert_eq!(refused(Dims::Xy, tin(vec![element("", "t")], None)), INVALID);
    assert_eq!(refused(Dims::Xy, tin(vec![element("stop line", &"t".repeat(65))], None)), INVALID);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68] a value's
/// well-known binary read as a type it is of, or a supertype, is the
/// value; read as another type it is refused (2FF23), and so is an
/// ST_BRepSolid of a code without z.
#[test]
fn typed_well_known_binary_is_of_its_type_5_1_68() {
    let g = Geom::new(4326, Dims::Xy, Body::Point(vec![1.0_f64, 2.0_f64])).unwrap();
    let wkb = write_wkb(&g);
    assert_eq!(read_wkb_typed(&wkb, 4326, Kind::Point).unwrap(), g);
    assert_eq!(read_wkb_typed(&wkb, 4326, Kind::Geometry).unwrap(), g);
    assert_eq!(read_wkb_typed(&wkb, 4326, Kind::LineString).unwrap_err().state, Some(SqlState::SPATIAL_INVALID_WKB));
    let flat = Kind::BRepSolid.wkb_code().unwrap().to_le_bytes();
    let solid = [1_u8, flat[0], flat[1], flat[2], flat[3], 0, 0, 0, 0];
    assert_eq!(read_wkb(&solid, 0).unwrap_err().state, Some(SqlState::SPATIAL_INVALID_WKB));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.4, §7.10,
/// §8.2] a curve polygon's ring that is a circle ends where it starts; a
/// ring or a compound's curve given by parameters has its ends computed,
/// so its points are not compared when the value is made.
#[test]
fn rings_and_compounds_of_circles_and_parameters_8_2() {
    let circle = Curve::Circle(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64, 2.0_f64, 0.0_f64]);
    assert!(Geom::new(0, Dims::Xy, Body::Surface(Surface::CurvePolygon(vec![circle]))).is_ok());
    let knots = vec![Knot { value: 0.0_f64, multiplicity: 2 }, Knot { value: 1.0_f64, multiplicity: 2 }];
    let points =
        vec![NurbsPoint { point: vec![1.0_f64, 1.0_f64], weight: None }, NurbsPoint { point: vec![0.0_f64, 0.0_f64], weight: None }];
    let nurbs = Curve::Nurbs(Some(Nurbs { degree: 1, points, knots, measures: None }));
    let line = Curve::LineString(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64]);
    let compound = Curve::Compound(vec![line, nurbs]);
    assert!(Geom::new(0, Dims::Xy, Body::Surface(Surface::CurvePolygon(vec![compound]))).is_ok());
}
