//! The routines in space of SQL/MM Spatial [ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §5.1, §7.1, §8.1] over each kind of collection: 3D
//! simplicity, measure locations, lengths, areas and points on surfaces,
//! and the arguments the routines refuse.

use turborel::db::{Database, Session};
use turborel::value::Value;

use crate::common::{rows, state};

fn session() -> (Database, Session) {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    (db, a)
}

fn one(s: &Session, sql: &str) -> String {
    let mut found = rows(s, &format!("VALUES {sql}")).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    assert_eq!(found.len(), 1, "{sql}");
    found.remove(0)
}

fn geo(text: &str) -> String {
    format!("ST_GEOMFROMTEXT('{text}')")
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.9]
/// ST_3DIsSimple: points not twice the same; curves that do not cross in
/// space; multicurves whose curves meet only where both end; patches that
/// meet only along their rims; collections whose elements are simple.
#[test]
fn simplicity_in_space_5_1_9() {
    let (_db, a) = session();
    let cases = [
        ("POINT Z (1 2 3)", "1"),
        ("LINESTRING Z (0 0 0, 1 0 0)", "1"),
        ("LINESTRING Z (0 0 0, 2 0 0, 1 -1 0, 1 1 0)", "0"),
        ("MULTIPOINT Z ((1 2 3), (1 2 4))", "1"),
        ("MULTIPOINT Z ((1 2 3), (1 2 3))", "0"),
        ("MULTILINESTRING Z ((0 0 0, 1 0 0), (1 0 0, 2 0 0))", "1"),
        ("MULTILINESTRING Z ((0 0 0, 2 0 0), (1 -1 0, 1 1 0))", "0"),
        ("MULTILINESTRING Z ((0 0 0, 2 0 0), (5 5 5, 6 6 6))", "1"),
        ("MULTILINESTRING Z ((0 0 0, 1 0 0, 0.5 -1 0, 0.5 1 0), (5 5 5, 6 6 6))", "0"),
        ("MULTICURVE Z ((0 0 0, 1 0 0), (0 0 1, 1 0 1))", "1"),
        ("MULTIPOLYGON Z (((0 0 0, 1 0 0, 0 1 0, 0 0 0)), ((5 5 5, 6 5 5, 5 6 5, 5 5 5)))", "1"),
        ("MULTISURFACE Z (((0 0 0, 1 0 0, 0 1 0, 0 0 0)))", "1"),
        ("POLYGON Z ((0 0 0, 1 0 0, 0 1 0, 0 0 0))", "1"),
        ("GEOMETRYCOLLECTION Z (POINT Z (1 2 3), LINESTRING Z (0 0 0, 1 0 0))", "1"),
        ("GEOMETRYCOLLECTION Z (POINT Z (1 2 3), LINESTRING Z (0 0 0, 2 0 0, 1 -1 0, 1 1 0))", "0"),
    ];
    for (text, want) in cases {
        assert_eq!(one(&a, &format!("ST_3DISSIMPLE({})", geo(text))), want, "{text}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.14, §5.1.16]
/// ST_3DLocateAlong and ST_3DLocateBetween keep the points of each kind
/// of collection whose measures lie in the range, and the pieces of its
/// lines; none found is the empty point.
#[test]
fn measure_locations_of_collections_5_1_16() {
    let (_db, a) = session();
    let cases = [
        ("POINT ZM (1 2 3 4)", 4.0_f64, 4.0_f64, "MULTIPOINT ZM ((1 2 3 4))"),
        ("POINT ZM (1 2 3 4)", 5.0_f64, 6.0_f64, "POINT ZM EMPTY"),
        ("MULTIPOINT ZM ((1 2 3 4), (5 6 7 8))", 3.0_f64, 5.0_f64, "MULTIPOINT ZM ((1 2 3 4))"),
        ("MULTILINESTRING ZM ((0 0 0 0, 10 0 0 10))", 2.0_f64, 2.0_f64, "MULTIPOINT ZM ((2 0 0 2))"),
        ("MULTICURVE ZM ((0 0 0 0, 10 0 0 10))", 2.0_f64, 4.0_f64, "MULTILINESTRING ZM ((2 0 0 2, 4 0 0 4))"),
        (
            "GEOMETRYCOLLECTION ZM (POINT ZM (1 1 1 3), LINESTRING ZM (0 0 0 0, 10 0 0 10))",
            2.0_f64,
            4.0_f64,
            "GEOMETRYCOLLECTION ZM (LINESTRING ZM (2 0 0 2, 4 0 0 4), POINT ZM (1 1 1 3))",
        ),
    ];
    for (text, from, to, want) in cases {
        let sql = format!("ST_ASTEXT(ST_3DLOCATEBETWEEN({}, {from:e}, {to:e}))", geo(text));
        assert_eq!(one(&a, &sql), want, "{text}");
    }
    assert_eq!(one(&a, &format!("ST_ASTEXT(ST_3DLOCATEALONG({}, 4E0))", geo("MULTIPOINT ZM ((1 2 3 4))"))), "MULTIPOINT ZM ((1 2 3 4))");
    assert_eq!(state(&a, &format!("VALUES ST_3DLOCATEBETWEEN({}, 4E0, 3E0)", geo("POINT ZM (1 2 3 4)"))), "2FF02");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.12, §8.1.2,
/// §8.1.5, §8.1.6] the lengths of multicurves, the areas of multisurfaces
/// and the point on a surface, in space; a value of a type a routine does
/// not take (2FF02), a null argument the null value.
#[test]
fn measures_of_collections_in_space_8_1() {
    let (_db, a) = session();
    let lines = ["MULTILINESTRING Z ((0 0 0, 0 0 3), (0 0 0, 4 0 0))", "MULTICURVE Z ((0 0 0, 0 0 3), (0 0 0, 4 0 0))"];
    for text in lines {
        assert_eq!(one(&a, &format!("ST_3DLENGTH({})", geo(text))), "7E0", "{text}");
    }
    let surfaces = ["MULTIPOLYGON Z (((0 0 0, 2 0 0, 0 0 2, 0 0 0)))", "MULTISURFACE Z (((0 0 0, 2 0 0, 0 0 2, 0 0 0)))"];
    for text in surfaces {
        assert_eq!(one(&a, &format!("ST_3DAREA({})", geo(text))), "2E0", "{text}");
    }
    let on = one(&a, &format!("ST_ASTEXT(ST_3DPOINTONSURF({}))", geo("POLYGON Z ((0 0 1, 4 0 1, 0 4 1, 0 0 1))")));
    assert!(on.starts_with("POINT Z (") && on.ends_with(" 1)"), "{on}");
    assert_eq!(one(&a, &format!("ST_3DPOINTONSURF({})", geo("POLYGON Z EMPTY"))), "NULL");
    assert_eq!(state(&a, &format!("VALUES ST_3DLENGTH({})", geo("POINT Z (1 2 3)"))), "2FF02");
    assert_eq!(one(&a, &format!("ST_3DDISTANCE({}, CAST(NULL AS ST_GEOMETRY))", geo("POINT Z (1 2 3)"))), "NULL");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1, §5.1.42
/// Description 4c] a pair of geometries in space of two reference
/// systems is 2FF10; a distance in space of a geographic system has no
/// linear unit (2FF14).
#[test]
fn pairs_in_space_share_a_system_5_1_42() {
    let (_db, a) = session();
    let p = "ST_GEOMFROMTEXT('POINT Z (1 2 3)', 4326)";
    let q = "ST_GEOMFROMTEXT('POINT Z (1 2 3)', 3857)";
    assert_eq!(state(&a, &format!("VALUES ST_3DINTERSECTS({p}, {q})")), "2FF10");
    assert_eq!(state(&a, &format!("VALUES ST_3DDISTANCE({p}, {p}, 'metre')")), "2FF14");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.2, §7.1.4,
/// §7.1.12, §8.1.2, §8.1.4, §8.1.5, §9.1.2 to §9.1.4] the measures in
/// space of the empty set are the null value, an empty curve is not
/// closed; the distance to an empty point is the null value, to a point
/// of another system 2FF10.
#[test]
fn measures_in_space_of_the_empty_set_8_1() {
    let (_db, a) = session();
    let null = [
        "ST_3DLENGTH(ST_GEOMFROMTEXT('LINESTRING Z EMPTY'))",
        "ST_3DAREA(ST_GEOMFROMTEXT('POLYGON Z EMPTY'))",
        "ST_3DPERIMETER(ST_GEOMFROMTEXT('POLYGON Z EMPTY'))",
        "ST_3DCENTROID(ST_GEOMFROMTEXT('POLYGON Z EMPTY'))",
        "ST_3DVOLUME(NEW ST_BREPSOLID())",
        "ST_3DSURFACEAREA(NEW ST_BREPSOLID())",
        "ST_3DPOINTONSOLID(NEW ST_BREPSOLID())",
        "ST_3DDISTANCETOPT(TREAT(ST_GEOMFROMTEXT('LINESTRING Z (0 0 0, 1 0 0)') AS ST_CURVE), TREAT(ST_GEOMFROMTEXT('POINT Z EMPTY') AS ST_POINT))",
    ];
    for sql in null {
        assert_eq!(one(&a, &format!("{sql} IS NULL")), "TRUE", "{sql}");
    }
    assert_eq!(one(&a, "ST_3DISCLOSED(ST_GEOMFROMTEXT('LINESTRING Z EMPTY'))"), "0");
    let other = "ST_3DDISTANCETOPT(TREAT(ST_GEOMFROMTEXT('LINESTRING Z (0 0 0, 1 0 0)') AS ST_CURVE), \
                 TREAT(ST_GEOMFROMTEXT('POINT Z (1 1 1)', 4326) AS ST_POINT))";
    assert_eq!(state(&a, &format!("VALUES {other}")), "2FF10");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.17, §5.1.18,
/// §5.1.19, §5.1.20] a set operation of an empty operand: the union of
/// both empty, or the intersection or difference of an empty first, is
/// the empty point; the union or difference of an empty second is the
/// first; a multicurve less a point stays a multicurve.
#[test]
fn set_operations_of_empty_operands_5_1_17() {
    let (_db, a) = session();
    let p = "ST_GEOMFROMTEXT('POINT (1 2)')";
    let e = "ST_GEOMFROMTEXT('POINT EMPTY')";
    let cases = [
        (format!("ST_UNION({e}, {e})"), "POINT EMPTY"),
        (format!("ST_INTERSECTION({e}, {p})"), "POINT EMPTY"),
        (format!("ST_DIFFERENCE({e}, {p})"), "POINT EMPTY"),
        (format!("ST_UNION({e}, {p})"), "POINT (1 2)"),
        (format!("ST_UNION({p}, {e})"), "POINT (1 2)"),
        (format!("ST_DIFFERENCE({p}, {e})"), "POINT (1 2)"),
        (format!("ST_INTERSECTION({p}, {e})"), "POINT EMPTY"),
        (
            "ST_DIFFERENCE(ST_GEOMFROMTEXT('MULTILINESTRING ((0 0, 1 0), (5 5, 6 6))'), ST_GEOMFROMTEXT('POINT (9 9)'))".to_owned(),
            "MULTILINESTRING ((0 0, 1 0), (5 5, 6 6))",
        ),
    ];
    for (sql, want) in cases {
        assert_eq!(one(&a, &format!("ST_ASTEXT({sql})")), want, "{sql}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1] a routine of
/// the geometry types called with too many arguments, or one not of its
/// parameter's type, is a syntax error; a null argument of a routine that
/// reads the statement's systems gives the null value.
#[test]
fn geometry_routines_check_their_arguments_5_1() {
    let (_db, a) = session();
    let p = "ST_GEOMFROMTEXT('POINT (1 2)')";
    assert_eq!(state(&a, &format!("VALUES ST_UNION({p}, {p}, {p})")), "42000");
    assert_eq!(state(&a, &format!("VALUES ST_DISTANCE({p}, 'x')")), "42000");
    assert_eq!(one(&a, &format!("ST_DISTANCE({p}, CAST(NULL AS ST_GEOMETRY)) IS NULL")), "TRUE");
    assert_eq!(one(&a, &format!("ST_BUFFER({p}, CAST(NULL AS DOUBLE PRECISION)) IS NULL")), "TRUE");
}
