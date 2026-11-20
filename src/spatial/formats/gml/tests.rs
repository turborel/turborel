use super::*;
use crate::geometry::model::Kind;
use crate::spatial::curves::types::{Coord, Curve, Dims};
use crate::spatial::formats::samples::values;
use crate::spatial::formats::tree::{Body, Surface};

mod truncated;

/// A catalog of two systems.
struct Catalog;

impl Srs for Catalog {
    fn by_text(&self, text: &str) -> Option<i32> {
        text.starts_with("GEOGCS[").then_some(4326)
    }

    fn by_code(&self, authority: &str, code: &str) -> Option<i32> {
        (authority == "EPSG").then(|| code.parse().ok()).flatten()
    }

    fn name(&self, srid: i32) -> Option<String> {
        Some(format!("EPSG:{srid}"))
    }
}

fn state(text: &str) -> String {
    read_geom(text, &Catalog).unwrap_err().sqlstate().as_str().to_owned()
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.60 §5.1.61 Table
/// 14] Every type, empty and not, in two and three dimensions reads back
/// from its GML, and the GML is stable.
#[test]
fn every_type_round_trips() {
    for dims in [Dims::Xy, Dims::Xyz] {
        for value in values(dims) {
            let text = write(&value, Some("EPSG:4326"));
            let (back, name) = read(&text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
            assert_eq!(back.body, value.body, "{text}");
            assert_eq!(name.as_deref(), Some("EPSG:4326"));
            assert_eq!(write(&back, Some("EPSG:4326")), text);
            let g = read_geom(&text, &Catalog).unwrap_or_else(|e| panic!("{text}: {e:?}"));
            assert_eq!(g.srid, 4326_i32);
            assert_eq!(write_geom(&g, &Catalog).unwrap(), text);
        }
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.61 4] Measures
/// are left out of GML, and z comes back.
#[test]
fn measures_are_left_out() {
    for value in values(Dims::Xyzm).into_iter().take(1) {
        let (back, _) = read(&write(&value, None)).unwrap();
        assert_eq!(back.dims, read_back_dims(Dims::Xyzm));
        assert_eq!(
            write(&value, None),
            "<gml:Point xmlns:gml=\"http://www.opengis.net/gml/3.2\"><gml:pos>1.5 -2.25 -3</gml:pos></gml:Point>"
        );
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.60 3-7 Table 14]
/// Positions in every form, z only when all have one, and the other
/// elements of Table 14.
#[test]
fn reads_table_14_forms() {
    let g = |text: &str| read(text).unwrap().0;
    let mixed = g("<LineString><pos>0 0 1</pos><pos>1 1</pos></LineString>");
    assert_eq!(mixed.dims, Dims::Xy);
    assert_eq!(mixed.body, Body::Curve(Curve::LineString(vec![Coord::xy(0.0, 0.0), Coord::xy(1.0, 1.0)])));
    let coords = g("<LineString><coordinates>0,0,1 1,1,2</coordinates></LineString>");
    assert_eq!(coords.dims, Dims::Xyz);
    let coord = g("<LineString><coord><X>0</X><Y>1</Y></coord><coord><X>2</X><Y>3</Y></coord></LineString>");
    assert_eq!(coord.body, Body::Curve(Curve::LineString(vec![Coord::xy(0.0, 1.0), Coord::xy(2.0, 3.0)])));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.60 Table 14] The
/// arcs, circles, geodesics, ellipses and TINs of Table 14's other
/// elements.
#[test]
fn reads_table_14_curves() {
    let g = |text: &str| read(text).unwrap().0;
    let arc = g(
        "<Curve><segments><ArcByCenterPoint><pos>0 0</pos><radius uom='m'>1</radius><startAngle uom='deg'>0</startAngle><endAngle>180</endAngle></ArcByCenterPoint></segments></Curve>",
    );
    let Body::Curve(Curve::CircularString(points)) = arc.body else { panic!("arc") };
    assert!((points[1].y - 1.0).abs() < 1e-12_f64 && (points[2].x + 1.0).abs() < 1e-12_f64);
    let circle = g("<CircleByCenterPoint><pos>1 1</pos><radius>2</radius></CircleByCenterPoint>");
    assert!(matches!(circle.body, Body::Curve(Curve::Circle(ref p)) if p.len() == 3));
    let bulge = g("<ArcByBulge><posList>0 0 2 0</posList><bulge>1</bulge><normal>1</normal></ArcByBulge>");
    assert!(matches!(bulge.body, Body::Curve(Curve::CircularString(ref p)) if p.len() == 3));
    let geodesic = g("<Geodesic><posList>0 0 1 1</posList></Geodesic>");
    assert!(matches!(geodesic.body, Body::Curve(Curve::GeodesicString(_))));
    let segments = g(
        "<Curve><segments><LineStringSegment><posList>0 0 1 1</posList></LineStringSegment><Arc><posList>1 1 2 2 3 1</posList></Arc></segments></Curve>",
    );
    assert!(matches!(segments.body, Body::Curve(Curve::Compound(ref c)) if c.len() == 2));
    let ellipse = g(
        "<Ellipse><refLocation><AffinePlacement><location><pos>0 0</pos></location><refDirection>1 0</refDirection><refDirection>0 1</refDirection></AffinePlacement></refLocation><uAxisLength>2</uAxisLength><vAxisLength>1</vAxisLength></Ellipse>",
    );
    let Body::Curve(Curve::Elliptical(Some(e))) = ellipse.body else { panic!("ellipse") };
    assert!((e.end - std::f64::consts::TAU).abs() < 1e-15_f64);
    let tin = g(
        "<Tin><trianglePatches><Triangle><exterior><LinearRing><posList>0 0 1 0 0 1 0 0</posList></LinearRing></exterior></Triangle></trianglePatches><stopLines><LineStringSegment><posList>0 0 1 1</posList></LineStringSegment></stopLines><breakLines/><maxLength uom='m'>5</maxLength><controlPoint><posList>0.2 0.2</posList></controlPoint></Tin>",
    );
    let Body::Surface(Surface::Tin(t)) = tin.body else { panic!("tin") };
    assert_eq!((t.triangles.len(), t.elements.len(), t.max_side), (1, 2, Some(5.0_f64)));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.60 2 7] Unknown
/// elements raise 2FF24 and unknown reference systems 2FF59; a URN and
/// reference system text resolve.
#[test]
fn errors_and_reference_systems() {
    assert_eq!(read_geom("<Point srsName='urn:ogc:def:crs:EPSG::3857'><pos>1 2</pos></Point>", &Catalog).unwrap().srid, 3857_i32);
    assert_eq!(read_geom("<Point srsName='GEOGCS[\"WGS 84\"]'><pos>1 2</pos></Point>", &Catalog).unwrap().srid, 4326_i32);
    assert_eq!(read_geom("<Point><pos>1 2</pos></Point>", &Catalog).unwrap().kind(), Kind::Point);
    assert_eq!(state("<Point srsName='XYZ:1'><pos>1 2</pos></Point>"), "2FF59");
    assert_eq!(state("<Point srsName='plain'><pos>1 2</pos></Point>"), "2FF59");
    for text in [
        "<Square/>",
        "<Point><pos>1</pos></Point>",
        "<Point><pos>1 x</pos></Point>",
        "<LineString><posList srsDimension='3'>1 2 3 4</posList></LineString>",
        "<LineString><posList srsDimension='x'>1 2</posList></LineString>",
        "<MultiPoint><pointMember><LineString/></pointMember></MultiPoint>",
        "<MultiLineString><lineStringMember><Curve/></lineStringMember></MultiLineString>",
        "<MultiCurve><curveMember/></MultiCurve>",
        "<Polygon><exterior><Ring/></exterior></Polygon>",
        "<Solid><exterior><Polygon/></exterior></Solid>",
        "<Tin><trianglePatches/><element type='wall'><Point/></element></Tin>",
        "<Tin><trianglePatches/><element type='hole'><Point/></element></Tin>",
        "<Tin><trianglePatches/><element type='hole' id='x'><Polygon/></element></Tin>",
        "<SpiralCurve><refLocation><AffinePlacement><location>0 0</location></AffinePlacement></refLocation><spiralType>wobble</spiralType></SpiralCurve>",
        "<Clothoid><scaleFactor>1</scaleFactor></Clothoid>",
        "<Clothoid><refLocation><AffinePlacement/></refLocation></Clothoid>",
        "<BSpline><posList>0 0 1 1</posList><degree>1.5</degree></BSpline>",
        "<BSpline><posList>0 0 1 1</posList><degree>1</degree><knot/></BSpline>",
        "<BSpline><posList>0 0 1 1</posList><degree>1</degree><weights>1</weights></BSpline>",
        "<ArcByBulge><posList>0 0 2 0</posList><bulge>1</bulge><normal>1 2</normal></ArcByBulge>",
        "<ArcByCenterPoint><radius>1</radius></ArcByCenterPoint>",
        "<EllipticalCurve><refLocation><AffinePlacement><location>0 0</location></AffinePlacement></refLocation><uAxisLength>1 2</uAxisLength></EllipticalCurve>",
    ] {
        assert_eq!(state(text), "2FF24", "{text}");
    }
}
