//! ST_LocateAlong and ST_LocateBetween of every kind of measured value
//! through SQL [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.13, §5.1.15]:
//! the rings of surfaces, the members of collections, curves other than
//! line strings, and results that mix runs and points.

use turborel::db::{Database, Session};
use turborel::value::Value;

fn session() -> (Database, Session) {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE ONE (X INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO ONE VALUES (1)").unwrap();
    (db, a)
}

/// The well-known text of the part of `text` between `start` and `end`.
fn between(a: &Session, text: &str, (start, end): (&str, &str)) -> String {
    let query = format!("SELECT CAST(ST_ASTEXT(ST_GEOMFROMTEXT('{text}').ST_LOCATEBETWEEN({start}, {end})) AS VARCHAR(4000)) FROM ONE");
    let mut c = a.open_cursor(&query).unwrap_or_else(|e| panic!("{text}: {e}"));
    let row = c.fetch().unwrap_or_else(|e| panic!("{text}: {e}")).unwrap_or_default();
    row.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.15 Description
/// 2e] A surface's measures lie on its boundary, so the runs between two
/// measures are taken from each ring of a polygon, a triangle, a
/// polyhedral surface's patches and a TIN's triangles, z left out.
#[test]
fn surfaces_are_located_along_their_rings() {
    let (_db, a) = session();
    let cases = [
        (
            "POLYGON M ((0 0 0, 4 0 4, 4 4 8, 0 0 0), (1 1 0, 2 1 1, 2 2 2, 1 1 0))",
            "MULTICURVE M ((1 0 1, 3 0 3), (1.5 1.5 3, 0.5 0.5 1), (2 1 1, 2 2 2, 1.5 1.5 1))",
        ),
        ("TRIANGLE M (0 0 0, 4 0 4, 0 4 8)", "MULTICURVE M ((1 0 1, 3 0 3), (0 1.5 3, 0 0.5 1))"),
        (
            "POLYHEDRALSURFACE ZM (PATCHES (POLYGON ((0 0 0 0, 1 0 0 2, 1 1 0 4, 0 0 0 0))))",
            "MULTICURVE M ((0.5 0 1, 1 0 2, 1 0.5 3), (0.75 0.75 3, 0.25 0.25 1))",
        ),
        ("TIN ZM (PATCHES ((0 0 0 0, 1 0 0 2, 0 1 0 4)))", "MULTICURVE M ((0.5 0 1, 1 0 2, 0.5 0.5 3), (0 0.75 3, 0 0.25 1))"),
    ];
    for (text, expected) in cases {
        assert_eq!(between(&a, text, ("1E0", "3E0")), expected, "{text}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.15 Description
/// 2e] A collection's runs are gathered from each member, for multi line
/// strings, multisurfaces and multipolygons; a member wholly outside the
/// range gives nothing.
#[test]
fn collections_are_located_member_by_member() {
    let (_db, a) = session();
    let cases = [
        ("MULTILINESTRING M ((0 0 0, 4 0 4), (0 1 4, 4 1 0))", "MULTICURVE M ((1 0 1, 3 0 3), (1 1 3, 3 1 1))"),
        ("MULTISURFACE M (((0 0 0, 4 0 4, 4 4 8, 0 0 0)))", "MULTICURVE M ((1 0 1, 3 0 3), (1.5 1.5 3, 0.5 0.5 1))"),
        (
            "MULTIPOLYGON M (((0 0 0, 4 0 4, 4 4 8, 0 0 0)), ((9 9 9, 9 8 9, 8 8 9, 9 9 9)))",
            "MULTICURVE M ((1 0 1, 3 0 3), (1.5 1.5 3, 0.5 0.5 1))",
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(between(&a, text, ("1E0", "3E0")), expected, "{text}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.15 Description
/// 2e ii 3] Runs and points apart make a geometry collection: one line
/// reaches the range only at its end, the other runs through it.
#[test]
fn runs_and_points_apart_make_a_collection() {
    let (_db, a) = session();
    let text = "MULTILINESTRING M ((0 0 0, 2 0 2), (0 1 2, 0 2 3))";
    assert_eq!(between(&a, text, ("2E0", "3E0")), "GEOMETRYCOLLECTION M (LINESTRING M (0 1 2, 0 2 3), POINT M (2 0 2))");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.15 Description
/// 2e] An arc's measure runs with its length: the half-circle from (0 0)
/// over (2 2) to (4 0), measured 0 to 4, is between measures 1 and 3 from
/// 135 to 45 degrees about (2 0). A circular string, a multicurve member
/// and a curve polygon's ring give the same run.
#[test]
fn arcs_are_located_by_their_length() {
    let (_db, a) = session();
    let root = 2f64.sqrt();
    for text in [
        "CIRCULARSTRING M (0 0 0, 2 2 2, 4 0 4)",
        "MULTICURVE M (CIRCULARSTRING M (0 0 0, 2 2 2, 4 0 4))",
        "CURVEPOLYGON M (CIRCULARSTRING M (0 0 0, 2 2 2, 4 0 4, 2 -2 6, 0 0 8))",
    ] {
        let got = between(&a, text, ("1E0", "3E0"));
        assert!(got.starts_with("MULTICURVE M (("), "{text}: {got}");
        let points = points_of(&got);
        assert!(near(points[0], [2.0 - root, root, 1.0]), "{text}: {got}");
        assert!(near(points[points.len() - 1], [2.0 + root, root, 3.0]), "{text}: {got}");
        assert!(points.iter().any(|&p| near(p, [2.0, 2.0, 2.0])), "{text}: {got}");
        assert!(points.iter().all(|&[x, y, _]| ((x - 2.0).hypot(y) - 2.0).abs() < 1e-9), "{text}: {got}");
    }
}

/// The X Y M points of the text of a multicurve, in order.
fn points_of(text: &str) -> Vec<[f64; 3]> {
    let inner = text.trim_start_matches("MULTICURVE M ((").trim_end_matches("))");
    inner
        .split(", ")
        .map(|point| {
            let v: Vec<f64> = point.split(' ').map(|n| n.parse().unwrap()).collect();
            [v[0], v[1], v[2]]
        })
        .collect()
}

/// Whether `a` and `b` agree to within the last digits that differ
/// between platforms' trigonometry.
fn near(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.15 Description
/// 2e] A compound surface is located through each of its patches.
#[test]
fn compound_surfaces_are_located_by_their_patches() {
    let (_db, a) = session();
    let compound = "COMPOUNDSURFACE M (POLYGON M ((0 0 0, 1 0 2, 1 1 4, 0 0 0)), POLYGON M ((0 0 0, 1 1 4, 0 1 6, 0 0 0)))";
    assert_eq!(between(&a, compound, ("5E0", "6E0")), "MULTICURVE M ((0.5 1 5, 0 1 6, 0 0.8333333333333334 5))");
}
