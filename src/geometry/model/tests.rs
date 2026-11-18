//! Unit tests: the geometry model, its checks and its stored form
//! (RULES.md R-TEST-1).

use super::*;
use crate::error::SqlState;

/// A placement at `location`, of no reference direction.
fn at(location: &[f64]) -> Placement {
    Placement { location: location.to_vec(), directions: Vec::new() }
}

/// A curve of each type given by parameters, of `planar` coordinates and
/// the measures `m`.
fn parametric(planar: usize, m: Option<Measures>) -> Vec<Body> {
    let loc = |x: f64| -> Vec<f64> {
        let mut c = vec![x, 0.5_f64];
        c.resize(planar, 0.0_f64);
        c
    };
    vec![
        Body::Curve(Curve::Elliptical(Some(Elliptical {
            placement: at(&loc(1.0_f64)),
            u_axis: 2.0_f64,
            v_axis: 1.0_f64,
            start_angle: 0.0_f64,
            end_angle: 1.5_f64,
            measures: m,
        }))),
        Body::Curve(Curve::Nurbs(Some(Nurbs {
            degree: 1,
            points: vec![
                NurbsPoint { point: loc(0.0_f64), weight: Some(1.0_f64) },
                NurbsPoint { point: loc(2.0_f64), weight: Some(2.0_f64) },
            ],
            knots: vec![Knot { value: 0.0_f64, multiplicity: 2 }, Knot { value: 1.0_f64, multiplicity: 2 }],
            measures: m,
        }))),
        Body::Curve(Curve::Clothoid(Some(Clothoid {
            placement: at(&loc(0.0_f64)),
            scale_factor: 1.0_f64,
            start_distance: 0.0_f64,
            end_distance: 2.0_f64,
            measures: m,
        }))),
        Body::Curve(Curve::Spiral(Some(Spiral {
            placement: Placement { location: loc(0.0_f64), directions: loc(1.0_f64) },
            length: 3.0_f64,
            start_curvature: 0.0_f64,
            end_curvature: 0.5_f64,
            spiral_type: SpiralType::Bloss,
            measures: m,
        }))),
    ]
}

/// A TIN of the triangle `tri` and a breakline element at `point`.
fn tin(tri: Coords, point: Coords) -> Tin {
    let element = TinElement { element_type: "breakline".into(), id: -7, tag: "a b".into(), geometry: Box::new(Body::Point(point)) };
    Tin { triangles: vec![tri], elements: vec![element], max_side_length: Some(4.0_f64) }
}

/// A value of every instantiable type, in `dims`, each not empty.
pub(crate) fn samples(dims: Dims) -> Vec<Geom> {
    let s = dims.stride();
    let p = |x: f64, y: f64| -> Coords {
        let mut c = vec![x, y];
        c.resize(s, 1.0_f64);
        c
    };
    let line = |pts: &[(f64, f64)]| -> Coords { pts.iter().flat_map(|(x, y)| p(*x, *y)).collect() };
    let ring = line(&[(0.0_f64, 0.0_f64), (0.0_f64, 1.0_f64), (1.0_f64, 1.0_f64), (1.0_f64, 0.0_f64), (0.0_f64, 0.0_f64)]);
    let tri = line(&[(0.0_f64, 0.0_f64), (0.0_f64, 1.0_f64), (1.0_f64, 0.0_f64), (0.0_f64, 0.0_f64)]);
    let planar = dims.planar();
    let m = dims.has_m().then_some(Measures { start: 0.0_f64, end: 10.0_f64 });
    let arc = line(&[(0.0_f64, 0.0_f64), (1.0_f64, 1.0_f64), (2.0_f64, 0.0_f64)]);
    let mut bodies = vec![
        Body::Point(p(1.0_f64, 2.0_f64)),
        Body::Curve(Curve::LineString(line(&[(0.0_f64, 0.0_f64), (1.0_f64, 1.0_f64)]))),
        Body::Curve(Curve::CircularString(arc.clone())),
        Body::Curve(Curve::Circle(arc.clone())),
        Body::Curve(Curve::GeodesicString(line(&[(10.0_f64, 50.0_f64), (20.0_f64, 55.0_f64)]))),
    ];
    bodies.extend(parametric(planar, m));
    bodies.extend([
        Body::Curve(Curve::Compound(vec![Curve::CircularString(arc), Curve::LineString(line(&[(2.0_f64, 0.0_f64), (3.0_f64, 0.0_f64)]))])),
        Body::Surface(Surface::CurvePolygon(vec![Curve::LineString(ring.clone())])),
        Body::Surface(Surface::Polygon(vec![ring.clone()])),
        Body::Surface(Surface::Triangle(tri.clone())),
        Body::Surface(Surface::PolyhdrlSurface(vec![vec![ring.clone()], vec![tri.clone()]])),
        Body::Surface(Surface::Tin(tin(tri.clone(), p(0.5_f64, 0.5_f64)))),
        Body::Surface(Surface::Compound(vec![Surface::Polygon(vec![ring.clone()]), Surface::Triangle(tri.clone())])),
        Body::Collection(Collection::Geometry(vec![Body::Point(Vec::new()), Body::Point(p(1.0_f64, 1.0_f64))])),
        Body::Collection(Collection::MultiPoint(vec![p(1.0_f64, 1.0_f64), Vec::new()])),
        Body::Collection(Collection::MultiCurve(vec![
            Curve::Circle(Vec::new()),
            Curve::LineString(line(&[(0.0_f64, 0.0_f64), (1.0_f64, 1.0_f64)])),
        ])),
        Body::Collection(Collection::MultiLineString(vec![line(&[(0.0_f64, 0.0_f64), (1.0_f64, 1.0_f64)])])),
        Body::Collection(Collection::MultiSurface(vec![Surface::Triangle(tri.clone())])),
        Body::Collection(Collection::MultiPolygon(vec![vec![ring.clone()]])),
    ]);
    let mut out: Vec<Geom> = bodies.into_iter().map(|b| Geom::new(4326, dims, b).unwrap()).collect();
    if dims == Dims::Xyz {
        out.push(Geom::new(0, dims, Body::Solid(vec![Surface::PolyhdrlSurface(vec![vec![ring]])])).unwrap());
    }
    out
}

/// The empty value of each instantiable type.
fn empties(dims: Dims) -> Vec<Geom> {
    let bodies = vec![
        Body::Point(Vec::new()),
        Body::Curve(Curve::LineString(Vec::new())),
        Body::Curve(Curve::Elliptical(None)),
        Body::Curve(Curve::Nurbs(None)),
        Body::Curve(Curve::Clothoid(None)),
        Body::Curve(Curve::Spiral(None)),
        Body::Curve(Curve::Compound(Vec::new())),
        Body::Surface(Surface::Triangle(Vec::new())),
        Body::Surface(Surface::Tin(Tin { triangles: Vec::new(), elements: Vec::new(), max_side_length: None })),
        Body::Surface(Surface::Polygon(Vec::new())),
        Body::Collection(Collection::Geometry(Vec::new())),
    ];
    bodies.into_iter().map(|b| Geom::new(0, dims, b).unwrap()).collect()
}

const ALL_DIMS: [Dims; 4] = [Dims::Xy, Dims::Xyz, Dims::Xym, Dims::Xyzm];

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68] every
/// instantiable type in every coordinate dimension round trips through its
/// stored form and its well-known binary representation, and its type and
/// code are kept.
#[test]
fn every_kind_round_trips() {
    for dims in ALL_DIMS {
        for g in samples(dims).into_iter().chain(empties(dims)) {
            let back = Geom::decode(&g.encode()).unwrap();
            assert_eq!(back, g, "{:?}", g.kind());
            assert_eq!(read_wkb(&write_wkb(&g), g.srid).unwrap(), g);
            let code = u32::from_le_bytes(write_wkb(&g)[1..5].try_into().unwrap());
            assert_eq!(Kind::of_wkb_code(code % 1000).unwrap(), g.kind());
        }
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68] a value of
/// the seven kinds D85 stored keeps the bytes D85 wrote, so old values
/// read in place.
#[test]
fn xy_kinds_keep_their_stored_bytes() {
    for text in [
        "POINT (1 2)",
        "POINT EMPTY",
        "LINESTRING (0 0, 1 1)",
        "POLYGON ((0 0, 0 1, 1 1, 0 0))",
        "MULTIPOINT ((1 1), EMPTY)",
        "GEOMETRYCOLLECTION (POINT (1 2), LINESTRING EMPTY)",
    ] {
        let old = crate::geometry::Geometry::from_wkt(text, 3857).unwrap().to_value();
        let Value::Extension(e) = &old else { panic!() };
        let g = Geom::from_value(&old).unwrap();
        assert_eq!(g.encode().as_slice(), e.bytes.as_ref(), "{text}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68] a value
/// given big-endian, with negative zero, or of a deprecated code reads as
/// its canonical form; stored bytes in those forms are refused.
#[test]
fn given_forms_read_canonically() {
    let mut be = vec![0u8];
    be.extend_from_slice(&1001u32.to_be_bytes());
    for n in [-0.0_f64, 2.0_f64, 3.0_f64] {
        be.extend_from_slice(&n.to_be_bytes());
    }
    let g = read_wkb(&be, 0).unwrap();
    assert_eq!(g, Geom::new(0, Dims::Xyz, Body::Point(vec![0.0_f64, 2.0_f64, 3.0_f64])).unwrap());
    assert!(g.encode().get(5).is_some_and(|b| *b == 1));
    let mut stored = vec![PAYLOAD_VERSION, 0, 0, 0, 0];
    stored.extend_from_slice(&be);
    assert!(Geom::decode(&stored).is_err());
    let mut deprecated = vec![1u8];
    deprecated.extend_from_slice(&1_000_001u32.to_le_bytes());
    deprecated.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(read_wkb(&deprecated, 0).unwrap().kind(), Kind::CircularString);
    let mut kept = vec![PAYLOAD_VERSION, 0, 0, 0, 0];
    kept.extend_from_slice(&deprecated);
    assert!(Geom::decode(&kept).is_err());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68] a value
/// given with its optional part left out at the end is empty.
#[test]
fn omitted_parts_are_empty() {
    for kind in [Kind::LineString, Kind::Tin, Kind::NurbsCurve, Kind::EllipticalCurve, Kind::Point] {
        let mut bytes = vec![1u8];
        bytes.extend_from_slice(&kind.wkb_code().unwrap().to_le_bytes());
        let g = read_wkb(&bytes, 0).unwrap();
        assert!(g.is_empty() && g.kind() == kind, "{kind:?}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §6 to §10, §22] the
/// model refuses what is no value of its type, each with its condition.
#[test]
fn malformed_values_are_refused() {
    let state = |b: Body, dims: Dims| Geom::new(0, dims, b).unwrap_err().state;
    let invalid = Some(SqlState::SPATIAL_INVALID_ARGUMENT);
    assert_eq!(state(Body::Curve(Curve::LineString(vec![0.0_f64, 0.0_f64])), Dims::Xy), invalid);
    assert_eq!(state(Body::Curve(Curve::CircularString(vec![0.0_f64; 8])), Dims::Xy), invalid);
    assert_eq!(state(Body::Curve(Curve::Circle(vec![0.0_f64; 4])), Dims::Xy), Some(SqlState::SPATIAL_EXACTLY_THREE_POINTS));
    assert_eq!(state(Body::Point(vec![f64::NAN, 0.0_f64]), Dims::Xy), Some(SqlState::SPATIAL_POINT_VALUE_NOT_WELL_FORMED));
    assert_eq!(
        state(
            Body::Surface(Surface::Polygon(vec![vec![0.0_f64, 0.0_f64, 1.0_f64, 0.0_f64, 1.0_f64, 1.0_f64, 0.0_f64, 1.0_f64]])),
            Dims::Xy
        ),
        invalid
    );
    assert_eq!(state(Body::Surface(Surface::Triangle(vec![0.0_f64; 10])), Dims::Xy), Some(SqlState::SPATIAL_EXTERIOR_RING_NOT_FOUR_POINTS));
    let gap = Curve::Compound(vec![
        Curve::LineString(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64]),
        Curve::LineString(vec![2.0_f64, 2.0_f64, 3.0_f64, 3.0_f64]),
    ]);
    assert_eq!(state(Body::Curve(gap), Dims::Xy), Some(SqlState::SPATIAL_NON_CONTIGUOUS_CURVES));
    let e = Elliptical {
        placement: at(&[0.0_f64, 0.0_f64]),
        u_axis: 1.0_f64,
        v_axis: 1.0_f64,
        start_angle: 0.0_f64,
        end_angle: 1.0_f64,
        measures: None,
    };
    assert_eq!(state(Body::Curve(Curve::Elliptical(Some(e))), Dims::Xym), Some(SqlState::SPATIAL_MISSING_MEASURE_VALUES));
    let shell = Surface::Polygon(vec![]);
    assert_eq!(state(Body::Solid(vec![shell]), Dims::Xyz), Some(SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE));
    assert_eq!(state(Body::Solid(Vec::new()), Dims::Xy), Some(SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS));
    let n = Nurbs {
        degree: 1,
        points: vec![NurbsPoint { point: vec![0.0_f64, 0.0_f64], weight: None }],
        knots: vec![Knot { value: 0.0_f64, multiplicity: 1 }],
        measures: None,
    };
    assert_eq!(state(Body::Curve(Curve::Nurbs(Some(n))), Dims::Xy), invalid);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68] well-known
/// binary bytes that are truncated, trailing, of mixed dimensions or of an
/// unknown code are refused with SQLSTATE 2FF23 or 2FF25.
#[test]
fn hostile_bytes_are_refused() {
    let g = samples(Dims::Xyzm).into_iter().find(|g| g.kind() == Kind::Tin).unwrap();
    let bytes = write_wkb(&g);
    for cut in 1..bytes.len() {
        assert!(read_wkb(&bytes[..cut], 0).is_err() || cut < 9_usize, "{cut}");
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(read_wkb(&trailing, 0).unwrap_err().state, Some(SqlState::SPATIAL_INVALID_WKB));
    let mut mixed = vec![1u8];
    mixed.extend_from_slice(&7u32.to_le_bytes());
    mixed.extend_from_slice(&1u32.to_le_bytes());
    mixed.push(1);
    mixed.extend_from_slice(&1001u32.to_le_bytes());
    mixed.extend_from_slice(&[0; 24]);
    assert_eq!(read_wkb(&mixed, 0).unwrap_err().state, Some(SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS));
    let mut unknown = vec![1u8];
    unknown.extend_from_slice(&13u32.to_le_bytes());
    assert_eq!(read_wkb(&unknown, 0).unwrap_err().state, Some(SqlState::SPATIAL_INVALID_WKB));
    let mut huge = vec![1u8];
    huge.extend_from_slice(&2u32.to_le_bytes());
    huge.extend_from_slice(&u32::MAX.to_le_bytes());
    assert!(read_wkb(&huge, 0).is_err());
}

/// Verifies: [DESIGN] RULES.md D85: a value's envelope holds every point
/// of it, the arc of a circular string and a geodesic string's bulge.
#[test]
fn envelopes_hold_curves() {
    let arc =
        Geom::new(0, Dims::Xy, Body::Curve(Curve::CircularString(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64, 2.0_f64, 0.0_f64]))).unwrap();
    let b = arc.bounds().unwrap().unwrap();
    assert!(b.lower()[1] <= -1.0_f64 + 1e-9_f64 && b.upper()[1] >= 1.0_f64 - 1e-9_f64);
    let geo = Geom::new(4326, Dims::Xy, Body::Curve(Curve::GeodesicString(vec![-60.0_f64, 60.0_f64, 60.0_f64, 60.0_f64]))).unwrap();
    assert!(geo.bounds().unwrap().unwrap().upper()[1] > 70.0_f64);
    for dims in ALL_DIMS {
        for g in samples(dims) {
            assert!(g.bounds().unwrap().is_some(), "{:?}", g.kind());
        }
        for g in empties(dims) {
            assert!(g.bounds().unwrap().is_none(), "{:?}", g.kind());
        }
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.6] the linear
/// and x-y forms of a value: a TIN as a multipolygon, a compound curve of
/// linestrings as one, z and m left out, and a curved one refused.
#[test]
fn linear_and_xy_forms() {
    let compound = Curve::Compound(vec![
        Curve::LineString(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64]),
        Curve::LineString(vec![1.0_f64, 1.0_f64, 2.0_f64, 0.0_f64]),
    ]);
    let joined = Geom::new(0, Dims::Xy, Body::Curve(compound)).unwrap().linear().unwrap();
    assert_eq!(joined.body, Body::Curve(Curve::LineString(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64, 2.0_f64, 0.0_f64])));
    let tin = samples(Dims::Xyzm).into_iter().find(|s| s.kind() == Kind::Tin).unwrap();
    let flat = tin.linear().unwrap().xy();
    assert_eq!((flat.kind(), flat.dims), (Kind::MultiPolygon, Dims::Xy));
    let arc = samples(Dims::Xy).into_iter().find(|s| s.kind() == Kind::CircularString).unwrap();
    assert!(arc.linear().is_err());
    for g in samples(Dims::Xyzm) {
        let xy = g.xy();
        assert_eq!(xy.kind(), g.kind());
        assert_eq!(Geom::new(xy.srid, xy.dims, xy.body.clone()).unwrap(), xy);
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2] the type
/// hierarchy: each type's supertypes, its instantiability, and its codes.
#[test]
fn the_hierarchy() {
    assert!(Kind::Tin.is_subtype_of(Kind::PolyhdrlSurface));
    assert!(Kind::Tin.is_subtype_of(Kind::Geometry));
    assert!(!Kind::Polygon.is_subtype_of(Kind::Triangle));
    assert!(Kind::MultiLineString.is_subtype_of(Kind::MultiCurve));
    assert_eq!(ALL.iter().filter(|k| k.is_instantiable()).count(), 23);
    for k in ALL {
        assert_eq!(Kind::named(k.sql_name()), Some(k));
        if let Some(code) = k.wkb_code() {
            assert_eq!(Kind::of_wkb_code(code), Some(k));
        }
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.5 Table 4] the
/// casts of the hierarchy: to a supertype as it is, empty to any type,
/// wrapped into a collection, unwrapped from one of one element, a polygon
/// of four points to a triangle, and a cast the table refuses.
#[test]
fn table_four_casts() {
    let p = Geom::new(0, Dims::Xy, Body::Point(vec![1.0_f64, 2.0_f64])).unwrap();
    assert_eq!(cast(&p, Kind::Geometry).unwrap(), p);
    let mp = cast(&p, Kind::MultiPoint).unwrap();
    assert_eq!(mp.kind(), Kind::MultiPoint);
    assert_eq!(cast(&mp, Kind::Point).unwrap(), p);
    assert_eq!(cast(&cast(&p, Kind::GeomCollection).unwrap(), Kind::Point).unwrap(), p);
    let empty = Geom::new(0, Dims::Xy, Body::Point(Vec::new())).unwrap();
    assert_eq!(cast(&empty, Kind::Tin).unwrap().kind(), Kind::Tin);
    assert_eq!(cast(&p, Kind::LineString).unwrap_err().state, Some(SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE));
    let tri = vec![0.0_f64, 0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64, 0.0_f64, 0.0_f64, 0.0_f64];
    let poly = Geom::new(0, Dims::Xy, Body::Surface(Surface::Polygon(vec![tri.clone()]))).unwrap();
    assert_eq!(cast(&poly, Kind::Triangle).unwrap().body, Body::Surface(Surface::Triangle(tri.clone())));
    assert_eq!(cast(&poly, Kind::Tin).unwrap().kind(), Kind::Tin);
    assert_eq!(cast(&poly, Kind::CurvePolygon).unwrap(), poly);
    let ls = Geom::new(0, Dims::Xy, Body::Curve(Curve::LineString(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64]))).unwrap();
    assert_eq!(cast(&cast(&ls, Kind::CompoundCurve).unwrap(), Kind::LineString).unwrap(), ls);
    assert_eq!(cast(&ls, Kind::MultiLineString).unwrap().kind(), Kind::MultiLineString);
    let both = Geom::new(0, Dims::Xy, Body::Collection(Collection::Geometry(vec![p.body.clone(), p.body.clone()]))).unwrap();
    assert_eq!(cast(&both, Kind::MultiPoint).unwrap().kind(), Kind::MultiPoint);
    assert!(cast(&both, Kind::Point).is_err());
}
