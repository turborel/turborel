//! One value of every instantiable type, empty and not, in each
//! coordinate dimension, for the formats' round-trip tests.

use crate::spatial::curves::tin::ElementKind;
use crate::spatial::curves::types::{
    Clothoid, Coord, Curve, Dims, Elliptical, Knot, Nurbs, NurbsPoint, Placement, Spiral, SpiralKind, Vector,
};
use crate::spatial::formats::tree::{Body, Collection, Element, Geometry, Ring, Surface, Tin};

/// Every dimension.
pub const DIMS: [Dims; 4] = [Dims::Xy, Dims::Xyz, Dims::Xym, Dims::Xyzm];

/// A point of `dims` from `x` and `y`, z and m derived from them.
pub fn at(dims: Dims, x: f64, y: f64) -> Coord {
    let z = if dims.has_z() { x + 2.0_f64 * y } else { 0.0_f64 };
    let m = if dims.has_m() { 10.0_f64 + x } else { 0.0_f64 };
    Coord { x, y, z, m }
}

fn ring(dims: Dims, xy: &[(f64, f64)]) -> Ring {
    xy.iter().map(|(x, y)| at(dims, *x, *y)).collect()
}

fn square(dims: Dims, low: f64, high: f64) -> Ring {
    ring(dims, &[(low, low), (high, low), (high, high), (low, high), (low, low)])
}

fn placement(dims: Dims) -> Placement {
    let flat = Coord { m: 0.0, ..at(dims, 1.0, 2.0) };
    let z = if dims.has_z() { 0.25_f64 } else { 0.0_f64 };
    Placement { location: flat, directions: vec![Vector { x: 1.0, y: 0.0, z }, Vector { x: 0.0, y: 1.0, z: 0.0 }] }
}

fn measures(dims: Dims) -> Option<[f64; 2]> {
    dims.has_m().then_some([3.0, 7.5])
}

/// One curve of every type, not empty.
pub fn curves(dims: Dims) -> Vec<Curve> {
    let line = ring(dims, &[(0.0_f64, 0.0_f64), (1.0_f64, 1.0_f64), (2.0_f64, 0.5_f64)]);
    let nurbs = Nurbs {
        degree: 2,
        points: [(0.0_f64, 0.0_f64, 1.0_f64), (1.0_f64, 2.0_f64, 0.5_f64), (2.0_f64, 0.0_f64, 1.0_f64)]
            .iter()
            .map(|(x, y, w)| NurbsPoint { point: Coord { m: 0.0, ..at(dims, *x, *y) }, weight: Some(*w) })
            .collect(),
        knots: vec![Knot { value: 0.0, multiplicity: 3 }, Knot { value: 1.0, multiplicity: 3 }],
        measures: measures(dims),
    };
    vec![
        Curve::LineString(line.clone()),
        Curve::CircularString(ring(dims, &[(0.0, 0.0), (1.0, 1.0), (2.0, 0.0)])),
        Curve::Circle(ring(dims, &[(0.0, 0.0), (1.0, 1.0), (2.0, 0.0)])),
        Curve::GeodesicString(ring(dims, &[(10.0, 50.0), (11.0, 51.0)])),
        Curve::Elliptical(Some(Elliptical {
            placement: placement(dims),
            u_axis: 3.0,
            v_axis: 2.0,
            start: 0.0,
            end: 1.5,
            measures: measures(dims),
        })),
        Curve::Nurbs(Some(nurbs)),
        Curve::Clothoid(Some(Clothoid { placement: placement(dims), scale: 2.0, start: 0.0, end: 1.25, measures: measures(dims) })),
        Curve::Spiral(Some(Spiral {
            placement: placement(dims),
            length: 5.0,
            start_curvature: 0.0,
            end_curvature: 0.125,
            kind: SpiralKind::Bloss,
            measures: measures(dims),
        })),
        Curve::Compound(vec![Curve::LineString(line), Curve::CircularString(ring(dims, &[(2.0, 0.5), (3.0, 1.0), (4.0, 0.5)]))]),
    ]
}

/// One surface of every type, not empty.
pub fn surfaces(dims: Dims) -> Vec<Surface> {
    let outer = square(dims, 0.0, 4.0);
    let inner = square(dims, 1.0, 2.0);
    let triangle = ring(dims, &[(0.0_f64, 0.0_f64), (1.0_f64, 0.0_f64), (0.0_f64, 1.0_f64), (0.0_f64, 0.0_f64)]);
    let other = ring(dims, &[(1.0_f64, 0.0_f64), (1.0_f64, 1.0_f64), (0.0_f64, 1.0_f64), (1.0_f64, 0.0_f64)]);
    let elements = vec![
        Element {
            kind: ElementKind::RandomPoints,
            id: 1,
            tag: "spots".into(),
            body: Body::Collection(Collection::MultiPoint(vec![Some(at(dims, 0.5, 0.5))])),
        },
        Element {
            kind: ElementKind::Breakline,
            id: -2,
            tag: String::new(),
            body: Body::Curve(Curve::LineString(ring(dims, &[(0.0_f64, 0.0_f64), (1.0_f64, 1.0_f64)]))),
        },
        Element {
            kind: ElementKind::Hole,
            id: 0,
            tag: "a_b-c.d".into(),
            body: Body::Surface(Surface::Polygon(vec![square(dims, 0.25, 0.5)])),
        },
    ];
    vec![
        Surface::CurvePolygon(vec![
            Curve::CircularString(ring(dims, &[(0.0, 0.0), (2.0, 2.0), (0.0, 0.0)])),
            Curve::LineString(inner.clone()),
        ]),
        Surface::Polygon(vec![outer.clone(), inner]),
        Surface::Triangle(triangle.clone()),
        Surface::Polyhedral(vec![vec![triangle.clone()], vec![other.clone()]]),
        Surface::Tin(Tin { triangles: vec![triangle.clone(), other.clone()], elements, max_side: Some(6.5) }),
        Surface::Tin(Tin { triangles: vec![triangle.clone()], elements: Vec::new(), max_side: None }),
        Surface::Compound(vec![Surface::Polygon(vec![outer]), Surface::Triangle(triangle)]),
    ]
}

/// The empty value of every type with one.
pub fn empties() -> Vec<Body> {
    let curves = [
        Curve::LineString(Vec::new()),
        Curve::CircularString(Vec::new()),
        Curve::Circle(Vec::new()),
        Curve::GeodesicString(Vec::new()),
        Curve::Elliptical(None),
        Curve::Nurbs(None),
        Curve::Clothoid(None),
        Curve::Spiral(None),
        Curve::Compound(Vec::new()),
    ];
    let surfaces = [
        Surface::CurvePolygon(Vec::new()),
        Surface::Polygon(Vec::new()),
        Surface::Triangle(Vec::new()),
        Surface::Polyhedral(Vec::new()),
        Surface::Tin(Tin { triangles: Vec::new(), elements: Vec::new(), max_side: None }),
        Surface::Compound(Vec::new()),
    ];
    let collections = [
        Collection::Geometry(Vec::new()),
        Collection::MultiPoint(Vec::new()),
        Collection::MultiCurve(Vec::new()),
        Collection::MultiLineString(Vec::new()),
        Collection::MultiSurface(Vec::new()),
        Collection::MultiPolygon(Vec::new()),
    ];
    let mut out = vec![Body::Point(None)];
    out.extend(curves.into_iter().map(Body::Curve));
    out.extend(surfaces.into_iter().map(Body::Surface));
    out.extend(collections.into_iter().map(Body::Collection));
    out
}

/// Every sample value of `dims`: each type, its empty value and the
/// collections of them.
pub fn values(dims: Dims) -> Vec<Geometry> {
    let curves = curves(dims);
    let surfaces = surfaces(dims);
    let mut bodies = vec![Body::Point(Some(at(dims, 1.5, -2.25)))];
    bodies.extend(curves.iter().cloned().map(Body::Curve));
    bodies.extend(surfaces.iter().cloned().map(Body::Surface));
    bodies.push(Body::Collection(Collection::MultiPoint(vec![Some(at(dims, 1.0, 2.0)), None])));
    bodies.push(Body::Collection(Collection::MultiCurve(curves.clone())));
    bodies.push(Body::Collection(Collection::MultiLineString(vec![square(dims, 0.0, 1.0), Vec::new()])));
    bodies.push(Body::Collection(Collection::MultiSurface(surfaces.clone())));
    bodies.push(Body::Collection(Collection::MultiPolygon(vec![vec![square(dims, 0.0, 1.0)], Vec::new()])));
    let members = bodies.clone();
    bodies.push(Body::Collection(Collection::Geometry(members)));
    bodies.extend(empties());
    if dims == Dims::Xyz {
        let shell = Surface::Polyhedral(vec![vec![square(dims, 0.0, 1.0)]]);
        bodies.push(Body::Solid(vec![shell, Surface::Compound(vec![Surface::Polyhedral(vec![vec![square(dims, 2.0, 3.0)]])])]));
        bodies.push(Body::Solid(Vec::new()));
    }
    bodies.into_iter().map(|body| Geometry { dims, body }).collect()
}
