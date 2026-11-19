use super::*;
use crate::spatial::curves::types::Curve;
use crate::spatial::formats::samples::{DIMS, values};
use crate::spatial::formats::wkt::{read_geom, write, write_geom};

fn state(text: &str) -> String {
    read(text).unwrap_err().sqlstate().as_str().to_owned()
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.67] Every type,
/// empty and not, in every coordinate dimension reads back from its
/// canonical text, and that text is stable, through the model too.
#[test]
fn every_type_round_trips() {
    for dims in DIMS {
        for value in values(dims) {
            let text = write(&value);
            let back = read(&text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
            assert_eq!(back, value, "{text}");
            assert_eq!(write(&back), text);
            let g = read_geom(&text, 4326).unwrap_or_else(|e| panic!("{text}: {e:?}"));
            assert_eq!(write_geom(&g).unwrap(), text);
        }
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.67] The canonical
/// text of a few values, keyword by keyword.
#[test]
fn canonical_text() {
    let point = Geometry { dims: Dims::Xyzm, body: Body::Point(Some(Coord::xyzm(1.0, 2.5, -3.0, 4.0))) };
    assert_eq!(write(&point), "POINT ZM (1 2.5 -3 4)");
    let empty = Geometry { dims: Dims::Xym, body: Body::Curve(Curve::LineString(Vec::new())) };
    assert_eq!(write(&empty), "LINESTRING M EMPTY");
    let compound = read("compoundcurve ((0 0, 1 1), CIRCULARSTRING (1 1, 2 2, 3 1))").unwrap();
    assert_eq!(write(&compound), "COMPOUNDCURVE ((0 0, 1 1), CIRCULARSTRING (1 1, 2 2, 3 1))");
    assert!(read("TRIANGLE ((0 0, 1 0, 0 1))").is_err());
    assert_eq!(write(&read("TRIANGLE (0 0, 1 0, 0 1)").unwrap()), "TRIANGLE (0 0, 1 0, 0 1)");
    let tin =
        read("TIN Z (PATCHES ((0 0 0, 1 0 0, 0 1 0)) ELEMENTS (STOPLINE TAG \"x\" LINESTRING Z (0 0 0, 1 1 0)) MAXSIDELENGTH 3)").unwrap();
    assert_eq!(
        write(&tin),
        "TIN Z (PATCHES ((0 0 0, 1 0 0, 0 1 0)) ELEMENTS (STOPLINE ID 0 TAG \"x\" LINESTRING Z (0 0 0, 1 1 0)) MAXSIDELENGTH 3)"
    );
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.67 at, §16.1.21]
/// Angles read in degrees, gradians or radians, with or without ANGLE.
#[test]
fn angles_in_every_unit() {
    let text = |start: &str| {
        format!(
            "ELLIPTICALCURVE (REFERENCELOCATION AFFINEPLACEMENT (LOCATION (0 0), REFERENCEDIRECTIONS (VECTOR (1 0), VECTOR (0 1))), UAXISLENGTH 2, VAXISLENGTH 1, STARTANGLE {start}, ENDANGLE RADIANS (1))"
        )
    };
    for (start, want) in
        [("ANGLE DEGREES (90)", std::f64::consts::FRAC_PI_2), ("GRADIANS (100)", std::f64::consts::FRAC_PI_2), ("RADIANS (0.5)", 0.5_f64)]
    {
        let Body::Curve(Curve::Elliptical(Some(e))) = read(&text(start)).unwrap().body else { panic!("{start}") };
        assert!((e.start - want).abs() < 1e-15_f64, "{start}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.67 ai ak ao at]
/// Mixed dimensions raise 2FF25, missing measures 2FF84, bad text 2FF22.
#[test]
fn errors_carry_their_states() {
    let clothoid =
        "(REFERENCELOCATION AFFINEPLACEMENT (LOCATION (0 0), REFERENCEDIRECTIONS EMPTY), SCALEFACTOR 1, STARTDISTANCE 0, ENDDISTANCE 1";
    assert_eq!(state("POINT (1 2 3)"), "2FF25");
    assert_eq!(state("POINT Z (1 2)"), "2FF25");
    assert_eq!(state("GEOMETRYCOLLECTION Z (POINT M (1 2 3))"), "2FF25");
    assert_eq!(state("BREPSOLID EMPTY"), "2FF25");
    assert_eq!(state(&format!("CLOTHOID Z {clothoid})")), "2FF25");
    assert_eq!(state(&format!("CLOTHOID M {clothoid})")), "2FF84");
    assert_eq!(state(&format!("CLOTHOID {clothoid}, STARTM 0, ENDM 1)")), "2FF25");
    assert_eq!(
        state("SPIRALCURVE (REFERENCELOCATION AFFINEPLACEMENT EMPTY, LENGTH 1, STARTCURVATURE 0, ENDCURVATURE 1, SPIRALTYPE wobble)"),
        "2FF02"
    );
    assert_eq!(
        state("SPIRALCURVE (REFERENCELOCATION AFFINEPLACEMENT EMPTY, LENGTH 1, STARTCURVATURE 0, ENDCURVATURE 1, SPIRALTYPE 7)"),
        "2FF02"
    );
    assert_eq!(state("NURBSCURVE (DEGREE 1, CONTROLPOINTS (NURBSPOINT EMPTY), KNOTS EMPTY)"), "2FF02");
    assert_eq!(state("NURBSCURVE (DEGREE 1, CONTROLPOINTS EMPTY, KNOTS (KNOT EMPTY))"), "2FF02");
    for text in [
        "POINT (1 2",
        "POINT (1 2) x",
        "SQUARE EMPTY",
        "MULTISURFACE (POINT (1 2))",
        "COMPOUNDCURVE (POLYGON EMPTY)",
        "BREPSOLID Z (TIN EMPTY)",
        "POLYHEDRALSURFACE (PATCHES (LINESTRING EMPTY))",
        "TIN (PATCHES ((0 0, 1 0, 0 1)) ELEMENTS (HOLE LINESTRING EMPTY))",
        "TIN (PATCHES ((0 0, 1 0, 0 1)) ELEMENTS (WALL LINESTRING EMPTY))",
        "TIN (PATCHES ((0 0, 1 0, 0 1)) ELEMENTS (HOLE TAG 7 POLYGON EMPTY))",
        "TIN (PATCHES ((0 0, 1 0, 0 1)) ELEMENTS (7))",
        "TRIANGLE (0 0, 1 1)",
        "7",
    ] {
        assert_eq!(state(text), "2FF22", "{text}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.67] A multipoint
/// reads bare points too, NURBS weights of one make no rational curve,
/// and empty numbers read as NaN and write back as EMPTY.
#[test]
fn lenient_forms() {
    let read_back = read("MULTIPOINT (1 2, (3 4), EMPTY)").unwrap();
    assert_eq!(read_back.body, Body::Collection(Collection::MultiPoint(vec![Some(Coord::xy(1.0, 2.0)), Some(Coord::xy(3.0, 4.0)), None])));
    let nurbs = read("NURBSCURVE (DEGREE 1, CONTROLPOINTS (NURBSPOINT (WEIGHTEDPOINT (0 0), WEIGHT 1), NURBSPOINT (WEIGHTEDPOINT (1 0), WEIGHT 1)), KNOTS (KNOT (VALUE 0, MULTIPLICITY 2), KNOT (VALUE 1, MULTIPLICITY 2)))").unwrap();
    let Body::Curve(Curve::Nurbs(Some(n))) = nurbs.body else { panic!("nurbs") };
    assert!(n.points.iter().all(|p| p.weight.is_none()));
    let empty = read("CLOTHOID (REFERENCELOCATION AFFINEPLACEMENT EMPTY, SCALEFACTOR EMPTY, STARTDISTANCE 0, ENDDISTANCE 1)").unwrap();
    let Body::Curve(Curve::Clothoid(Some(k))) = &empty.body else { panic!("clothoid") };
    assert!(k.scale.is_nan() && k.placement.location.x.is_nan());
    assert!(write(&empty).contains("AFFINEPLACEMENT EMPTY, SCALEFACTOR EMPTY"));
}

/// Verifies: [EXTENSION] [OGC 06-103r4 §7.2.2] A triangle, polyhedral
/// surface and TIN in Simple feature access's text are the values of
/// their text of ISO/IEC 13249-3, and `write_simple` writes that form,
/// read back alike; a TIN's elements are not written there.
#[test]
fn simple_feature_access_forms_read_as_the_standards() {
    use crate::spatial::formats::wkt::write_simple;
    let pairs = [
        ("TRIANGLE ((0 0, 1 0, 0 1, 0 0))", "TRIANGLE (0 0, 1 0, 0 1)"),
        ("TRIANGLE Z EMPTY", "TRIANGLE Z EMPTY"),
        (
            "POLYHEDRALSURFACE Z (((0 0 0, 0 1 0, 1 1 0, 0 0 0)), EMPTY)",
            "POLYHEDRALSURFACE Z (PATCHES (POLYGON Z ((0 0 0, 0 1 0, 1 1 0, 0 0 0)), POLYGON Z EMPTY))",
        ),
        ("TIN (((0 0, 1 0, 0 1, 0 0)), ((1 0, 1 1, 0 1, 1 0)))", "TIN (PATCHES ((0 0, 1 0, 0 1), (1 0, 1 1, 0 1)))"),
        ("TIN M EMPTY", "TIN M EMPTY"),
    ];
    for (simple, standard) in pairs {
        let value = read(simple).unwrap_or_else(|e| panic!("{simple}: {e:?}"));
        assert_eq!(value, read(standard).unwrap(), "{simple}");
        assert_eq!(write_simple(&value), simple);
        assert_eq!(write(&value), standard);
    }
    let tin = read("TIN (PATCHES ((0 0, 1 0, 0 1)) MAXSIDELENGTH 3)").unwrap();
    assert_eq!(write_simple(&tin), "TIN (((0 0, 1 0, 0 1, 0 0)))");
    for bad in ["TRIANGLE ((0 0, 1 0, 0 1))", "TRIANGLE ((0 0, 1 0, 0 1, 0 0), (0 0, 1 0, 0 1, 0 0))", "TIN ((0 0, 1 0, 0 1, 0 0))"] {
        assert_eq!(state(bad), "2FF22", "{bad}");
    }
}
