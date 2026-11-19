//! The casts of the type hierarchy [§4.2.1.5 Tables 2 to 4] and the
//! names, codes and dimensions each type of it has.

use crate::error::SqlState;
use crate::geometry::model::{ALL, Dims, Geom, Kind, cast, empty_body};
use crate::spatial::formats::wkt::{read_geom, write_geom};

fn geom(text: &str) -> Geom {
    read_geom(text, 0).unwrap()
}

fn cast_text(text: &str, target: Kind) -> Result<String, SqlState> {
    cast(&geom(text), target).map(|g| write_geom(&g).unwrap()).map_err(|e| e.state.unwrap())
}

const REFUSED: SqlState = SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE;

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.5 Table 3 E]
/// an empty set casts to the empty value of every instantiable type, and
/// to none of the types not instantiable.
#[test]
fn empty_sets_cast_to_every_instantiable_type() {
    let empty = geom("POINT EMPTY");
    for k in ALL {
        let got = cast(&empty, k);
        if k == Kind::Geometry {
            assert_eq!(got.unwrap(), empty);
        } else if k.is_instantiable() {
            let g = got.unwrap();
            assert_eq!(g.kind(), k);
            assert!(g.is_empty());
        } else {
            assert_eq!(got.unwrap_err().state, Some(REFUSED), "{k:?}");
            assert!(empty_body(k).is_none());
        }
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.5 Table 4
/// 1-DT] a collection, compound or polyhedral surface or TIN of one
/// element is unwrapped to it and cast by its own row; one of two is not.
#[test]
fn values_of_one_element_unwrap_table_4() {
    let cases = [
        ("MULTIPOINT ((1 2))", Kind::Point, "POINT (1 2)"),
        ("MULTICURVE ((0 0, 1 1))", Kind::LineString, "LINESTRING (0 0, 1 1)"),
        ("MULTILINESTRING ((0 0, 1 1))", Kind::LineString, "LINESTRING (0 0, 1 1)"),
        ("MULTISURFACE (((0 0, 1 0, 0 1, 0 0)))", Kind::Polygon, "POLYGON ((0 0, 1 0, 0 1, 0 0))"),
        ("MULTIPOLYGON (((0 0, 1 0, 0 1, 0 0)))", Kind::Polygon, "POLYGON ((0 0, 1 0, 0 1, 0 0))"),
        ("COMPOUNDCURVE ((0 0, 1 1))", Kind::LineString, "LINESTRING (0 0, 1 1)"),
        ("POLYHEDRALSURFACE (((0 0, 1 0, 0 1, 0 0)))", Kind::Polygon, "POLYGON ((0 0, 1 0, 0 1, 0 0))"),
        ("TIN (((0 0, 1 0, 0 1, 0 0)))", Kind::Triangle, "TRIANGLE (0 0, 1 0, 0 1)"),
    ];
    for (text, target, want) in cases {
        assert_eq!(cast_text(text, target).unwrap(), want, "{text}");
    }
    for (text, target) in [
        ("MULTIPOINT ((1 2), (3 4))", Kind::Point),
        ("MULTICURVE ((0 0, 1 1), (2 2, 3 3))", Kind::LineString),
        ("POLYHEDRALSURFACE (((0 0, 1 0, 0 1, 0 0)), ((0 0, 1 0, 0 1, 0 0)))", Kind::Polygon),
        ("TIN (((0 0, 1 0, 0 1, 0 0)), ((0 0, 1 0, 0 1, 0 0)))", Kind::Triangle),
        ("POINT (1 2)", Kind::LineString),
        ("LINESTRING (0 0, 1 1)", Kind::Point),
    ] {
        assert_eq!(cast_text(text, target).unwrap_err(), REFUSED, "{text}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.5 Table 4
/// n-DT] a curve, a surface or a collection of elements each of one type
/// casts to the multi type of it; a mixed collection does not.
#[test]
fn values_wrap_into_multi_types_table_4() {
    let cases = [
        ("LINESTRING (0 0, 1 1)", Kind::MultiCurve, "MULTICURVE ((0 0, 1 1))"),
        ("GEOMETRYCOLLECTION (LINESTRING (0 0, 1 1))", Kind::MultiCurve, "MULTICURVE ((0 0, 1 1))"),
        ("MULTICURVE ((0 0, 1 1))", Kind::MultiLineString, "MULTILINESTRING ((0 0, 1 1))"),
        ("GEOMETRYCOLLECTION (LINESTRING (0 0, 1 1))", Kind::MultiLineString, "MULTILINESTRING ((0 0, 1 1))"),
        ("POLYGON ((0 0, 1 0, 0 1, 0 0))", Kind::MultiSurface, "MULTISURFACE (((0 0, 1 0, 0 1, 0 0)))"),
        ("GEOMETRYCOLLECTION (POLYGON ((0 0, 1 0, 0 1, 0 0)))", Kind::MultiSurface, "MULTISURFACE (((0 0, 1 0, 0 1, 0 0)))"),
        ("POLYGON ((0 0, 1 0, 0 1, 0 0))", Kind::MultiPolygon, "MULTIPOLYGON (((0 0, 1 0, 0 1, 0 0)))"),
        ("TRIANGLE ((0 0, 1 0, 0 1, 0 0))", Kind::MultiPolygon, "MULTIPOLYGON (((0 0, 1 0, 0 1, 0 0)))"),
        ("MULTISURFACE (((0 0, 1 0, 0 1, 0 0)))", Kind::MultiPolygon, "MULTIPOLYGON (((0 0, 1 0, 0 1, 0 0)))"),
        ("GEOMETRYCOLLECTION (TRIANGLE ((0 0, 1 0, 0 1, 0 0)))", Kind::MultiPolygon, "MULTIPOLYGON (((0 0, 1 0, 0 1, 0 0)))"),
        ("POINT (1 2)", Kind::GeomCollection, "GEOMETRYCOLLECTION (POINT (1 2))"),
    ];
    for (text, target, want) in cases {
        assert_eq!(cast_text(text, target).unwrap(), want, "{text}");
    }
    for (text, target) in [
        ("GEOMETRYCOLLECTION (POINT (1 2), LINESTRING (0 0, 1 1))", Kind::MultiPoint),
        ("GEOMETRYCOLLECTION (POINT (1 2), LINESTRING (0 0, 1 1))", Kind::MultiCurve),
        ("GEOMETRYCOLLECTION (POINT (1 2), LINESTRING (0 0, 1 1))", Kind::MultiLineString),
        ("GEOMETRYCOLLECTION (POINT (1 2), POLYGON ((0 0, 1 0, 0 1, 0 0)))", Kind::MultiSurface),
        ("GEOMETRYCOLLECTION (POINT (1 2), POLYGON ((0 0, 1 0, 0 1, 0 0)))", Kind::MultiPolygon),
        ("LINESTRING (0 0, 1 1)", Kind::MultiPoint),
        ("POINT (1 2)", Kind::MultiCurve),
        ("POINT (1 2)", Kind::MultiLineString),
        ("POINT (1 2)", Kind::MultiSurface),
        ("POINT (1 2)", Kind::MultiPolygon),
        ("TIN (((0 0, 1 0, 0 1, 0 0)), ((1 0, 1 1, 0 1, 1 0)))", Kind::MultiPolygon),
        ("MULTIPOINT ((1 2), (3 4))", Kind::MultiPolygon),
    ] {
        assert_eq!(cast_text(text, target).unwrap_err(), REFUSED, "{text} {target:?}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.5 Table 4] a
/// curve linearizes to a linestring and wraps into a compound curve; a
/// polygon or triangle widens to a curve polygon, a polyhedral surface or
/// a compound surface; a TIN is a polyhedral surface as it is; a
/// polygon of one ring of four points is a triangle and a TIN of it.
#[test]
fn surfaces_and_curves_convert_table_4() {
    let cases = [
        ("CIRCULARSTRING (0 0, 1 1, 2 0)", Kind::CompoundCurve, "COMPOUNDCURVE (CIRCULARSTRING (0 0, 1 1, 2 0))"),
        ("TRIANGLE ((0 0, 1 0, 0 1, 0 0))", Kind::CurvePolygon, "TRIANGLE (0 0, 1 0, 0 1)"),
        ("TRIANGLE ((0 0, 1 0, 0 1, 0 0))", Kind::PolyhdrlSurface, "POLYHEDRALSURFACE (PATCHES (POLYGON ((0 0, 1 0, 0 1, 0 0))))"),
        ("TRIANGLE ((0 0, 1 0, 0 1, 0 0))", Kind::Tin, "TIN (PATCHES ((0 0, 1 0, 0 1)))"),
        ("POLYGON ((0 0, 1 0, 0 1, 0 0))", Kind::PolyhdrlSurface, "POLYHEDRALSURFACE (PATCHES (POLYGON ((0 0, 1 0, 0 1, 0 0))))"),
        ("TIN (((0 0, 1 0, 0 1, 0 0)), ((1 0, 1 1, 0 1, 1 0)))", Kind::PolyhdrlSurface, "TIN (PATCHES ((0 0, 1 0, 0 1), (1 0, 1 1, 0 1)))"),
        ("POLYGON ((0 0, 1 0, 0 1, 0 0))", Kind::CompoundSurface, "COMPOUNDSURFACE (((0 0, 1 0, 0 1, 0 0)))"),
        ("CURVEPOLYGON ((0 0, 1 0, 0 1, 0 0))", Kind::PolyhdrlSurface, "POLYHEDRALSURFACE (PATCHES (POLYGON ((0 0, 1 0, 0 1, 0 0))))"),
        ("CURVEPOLYGON ((0 0, 1 0, 0 1, 0 0))", Kind::Tin, "TIN (PATCHES ((0 0, 1 0, 0 1)))"),
        ("CURVEPOLYGON ((0 0, 1 0, 0 1, 0 0))", Kind::Triangle, "TRIANGLE (0 0, 1 0, 0 1)"),
        ("TRIANGLE ((0 0, 1 0, 0 1, 0 0))", Kind::Polygon, "TRIANGLE (0 0, 1 0, 0 1)"),
    ];
    for (text, target, want) in cases {
        assert_eq!(cast_text(text, target).unwrap(), want, "{text}");
    }
    assert_eq!(cast(&geom("CIRCULARSTRING (0 0, 1 1, 2 0)"), Kind::LineString).unwrap().kind(), Kind::LineString);
    for (text, target) in [
        ("POINT (1 2)", Kind::CompoundCurve),
        ("POINT (1 2)", Kind::Polygon),
        ("POINT (1 2)", Kind::Triangle),
        ("POINT (1 2)", Kind::CircularString),
        ("POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))", Kind::Triangle),
        ("POLYGON ((0 0, 4 0, 0 4, 0 0), (1 1, 2 1, 1 2, 1 1))", Kind::Triangle),
        ("TRIANGLE ((0 0, 1 0, 0 1, 0 0))", Kind::CompoundSurface),
        ("POINT (1 2)", Kind::BRepSolid),
    ] {
        assert_eq!(cast_text(text, target).unwrap_err(), REFUSED, "{text} {target:?}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2, §5.1.4,
/// §5.1.67] each type's supertype, standard name, well-known text key
/// word and dimension.
#[test]
fn every_type_has_its_names_and_dimension_4_2() {
    for k in ALL {
        assert_eq!(k.sql_name().to_ascii_uppercase(), k.type_name(), "{k:?}");
        assert!(k.is_subtype_of(Kind::Geometry));
        assert_eq!(k.supertype().is_none(), k == Kind::Geometry);
        assert_eq!(k.wkt_keyword().is_some(), k.is_instantiable());
        if let Some(word) = k.wkt_keyword() {
            let dims = if k == Kind::BRepSolid { Dims::Xyz } else { Dims::Xy };
            let empty = Geom::new(0, dims, empty_body(k).unwrap()).unwrap();
            let text = write_geom(&empty).unwrap_or_else(|e| panic!("{k:?} {e:?}"));
            assert!(text.starts_with(word), "{k:?} {text}");
        }
        let collection = k.is_subtype_of(Kind::GeomCollection) || k == Kind::Geometry;
        assert_eq!(k.dimension().is_none(), collection, "{k:?}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2, Annex C] a
/// solid is of dimension 3, a point is no curve, and the deprecated codes
/// 1000001 to 1000005 name their types, no others.
#[test]
fn deprecated_codes_and_dimensions_annex_c() {
    assert_eq!(Kind::BRepSolid.dimension(), Some(3));
    assert!(!Kind::Point.is_subtype_of(Kind::Curve));
    assert_eq!(Kind::of_wkb_code(1_000_001), Some(Kind::CircularString));
    assert_eq!(Kind::of_wkb_code(1_000_005), Some(Kind::MultiSurface));
    assert_eq!(Kind::of_wkb_code(1_000_006), None);
    assert_eq!(Kind::of_wkb_code(1_000_000), None);
}
