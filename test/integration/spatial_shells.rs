//! The shells of a B-rep solid and the clip of a TIN [ISO/IEC
//! 13249-3:2016 (DIS, SC32 N2593) §8.6.6, §9.2.3 to §9.2.6]: the empty
//! solid's, and each shell or window the mutators refuse.

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
    rows(s, &format!("VALUES ({sql})")).unwrap_or_else(|e| panic!("{sql}: {e:?}")).join(";")
}

/// The four faces of the unit tetrahedron, facing out, in dimensions
/// `dims` with `extra` after each point's z.
fn tetrahedron(dims: &str, extra: &str) -> String {
    let face =
        |points: [&str; 3]| format!("POLYGON {dims} (({0}{extra}, {1}{extra}, {2}{extra}, {0}{extra}))", points[0], points[1], points[2]);
    let faces = [
        face(["0 0 0", "0 1 0", "1 0 0"]),
        face(["0 0 0", "1 0 0", "0 0 1"]),
        face(["0 0 0", "0 0 1", "0 1 0"]),
        face(["1 0 0", "0 1 0", "0 0 1"]),
    ];
    format!("POLYHEDRALSURFACE {dims} (PATCHES ({}))", faces.join(", "))
}

fn surface(text: &str, srid: i32) -> String {
    format!("TREAT(ST_GEOMFROMTEXT('{text}', {srid}) AS ST_SURFACE)")
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §9.2.3 to §9.2.6]
/// the empty solid has no exterior shell, interior shells, count or nth
/// shell, each the null value; ST_ExteriorShell(ST_Surface) gives it one;
/// ST_InteriorShellN out of range is the null value.
#[test]
fn shells_of_the_empty_solid_9_2() {
    let (_db, a) = session();
    let empty = "NEW ST_BREPSOLID()";
    assert_eq!(
        one(&a, &format!("{empty}.ST_EXTERIORSHELL() IS NULL, {empty}.ST_INTERIORSHELLS() IS NULL, {empty}.ST_NUMINTSHELLS() IS NULL")),
        "TRUE,TRUE,TRUE"
    );
    assert_eq!(one(&a, &format!("{empty}.ST_INTERIORSHELLN(1) IS NULL")), "TRUE");
    let solid = format!("{empty}.ST_EXTERIORSHELL({})", surface(&tetrahedron("Z", ""), 0));
    assert_eq!(one(&a, &format!("{solid}.ST_NUMINTSHELLS(), {solid}.ST_INTERIORSHELLN(0) IS NULL")), "0,TRUE");
    let again = format!("{solid}.ST_EXTERIORSHELL({})", surface(&tetrahedron("Z", ""), 0));
    assert_eq!(one(&a, &format!("{again}.ST_NUMINTSHELLS()")), "0");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §9.2.3 Description
/// 4c, 4d, §9.2.4 Description 4a, §18.1.1] a shell of another system is
/// 2FF10, without z 2FF73, with m 2FF97, not closed 2FF02; interior
/// shells of a solid with no exterior one 2FF98, an array holding the
/// null value 2FF03.
#[test]
fn shells_refused_9_2_3() {
    let (_db, a) = session();
    let empty = "NEW ST_BREPSOLID()";
    let cases = [
        (surface(&tetrahedron("Z", ""), 4326), "2FF10"),
        (surface("POLYHEDRALSURFACE (PATCHES (POLYGON ((0 0, 1 0, 0 1, 0 0))))", 0), "2FF73"),
        (surface(&tetrahedron("ZM", " 1"), 0), "2FF97"),
        (surface("POLYHEDRALSURFACE Z (PATCHES (POLYGON Z ((0 0 0, 1 0 0, 0 1 0, 0 0 0))))", 0), "2FF02"),
    ];
    for (shell, want) in cases {
        assert_eq!(state(&a, &format!("VALUES {empty}.ST_EXTERIORSHELL({shell})")), want, "{shell}");
    }
    let shell = surface(&tetrahedron("Z", ""), 0);
    assert_eq!(state(&a, &format!("VALUES {empty}.ST_INTERIORSHELLS(ARRAY[{shell}])")), "2FF98");
    let solid = format!("{empty}.ST_EXTERIORSHELL({shell})");
    assert_eq!(state(&a, &format!("VALUES {solid}.ST_INTERIORSHELLS(ARRAY[{shell}, CAST(NULL AS ST_SURFACE)])")), "2FF03");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.6] ST_Clip of a
/// window of another system is 2FF10, of an empty window 2FF02.
#[test]
fn clips_refused_8_6_6() {
    let (_db, a) = session();
    let tin = "TREAT(ST_GEOMFROMTEXT('TIN Z (PATCHES ((0 0 0, 1 0 0, 0 1 0)))') AS ST_TIN)";
    let other = "TREAT(ST_GEOMFROMTEXT('POLYGON ((0 0, 1 0, 1 1, 0 0))', 4326) AS ST_POLYGON)";
    assert_eq!(state(&a, &format!("VALUES {tin}.ST_CLIP({other})")), "2FF10");
    let empty = "TREAT(ST_GEOMFROMTEXT('POLYGON EMPTY') AS ST_POLYGON)";
    assert_eq!(state(&a, &format!("VALUES {tin}.ST_CLIP({empty})")), "2FF02");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.7.2, §8.7.3,
/// §9.2.2] ST_BRepSolid of an exterior shell, of one and an SRID; and
/// ST_CompoundSurface of a surface and an SRID; a null argument gives the
/// null value, a first argument of no surface is a syntax error.
#[test]
fn solid_and_compound_constructors_9_2_2() {
    let (_db, a) = session();
    let shell = surface(&tetrahedron("Z", ""), 0);
    assert_eq!(one(&a, &format!("ST_BREPSOLID({shell}).ST_NUMINTSHELLS()")), "0");
    let placed = format!("ST_BREPSOLID(TREAT(ST_GEOMFROMTEXT('{}', 4326) AS ST_SURFACE), 4326)", tetrahedron("Z", ""));
    assert_eq!(one(&a, &format!("ST_SRID({placed})")), "4326");
    assert_eq!(one(&a, "ST_BREPSOLID(CAST(NULL AS ST_SURFACE)) IS NULL"), "TRUE");
    assert_eq!(state(&a, "VALUES ST_BREPSOLID(ST_POINT(1, 2))"), "42000");
    let polygon = "TREAT(ST_GEOMFROMTEXT('POLYGON ((0 0, 1 0, 0 1, 0 0))', 4326) AS ST_POLYGON)";
    assert_eq!(one(&a, &format!("ST_SRID(ST_COMPOUNDSURFACE({polygon}, 4326))")), "4326");
    assert_eq!(one(&a, "ST_COMPOUNDSURFACE(CAST(NULL AS ST_POLYGON)) IS NULL"), "TRUE");
    assert_eq!(state(&a, "VALUES ST_COMPOUNDSURFACE(ST_POINT(1, 2))"), "42000");
    let surfaces = "ARRAY[ST_POLYFROMTEXT('POLYGON ((0 0, 1 0, 0 1, 0 0))')]";
    assert_eq!(one(&a, &format!("CAST(NULL AS ST_COMPOUNDSURFACE).ST_SURFACES({surfaces}) IS NULL")), "TRUE");
    assert_eq!(state(&a, &format!("VALUES ST_POINT(1, 2).ST_SURFACES({surfaces})")), "42000");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.16, §7.1.2]
/// the measures of ST_3DLocateBetween may be exact numbers; a routine in
/// space of a declared type it does not take is a syntax error.
#[test]
fn routines_in_space_take_their_types_5_1() {
    let (_db, a) = session();
    let line = "ST_GEOMFROMTEXT('LINESTRING ZM (0 0 0 0, 10 0 0 10)')";
    assert_eq!(one(&a, &format!("ST_ASTEXT(ST_3DLOCATEBETWEEN({line}, 2, 4))")), "MULTILINESTRING ZM ((2 0 0 2, 4 0 0 4))");
    assert_eq!(state(&a, "VALUES ST_3DLENGTH(ST_POINT(1, 2))"), "42000");
    assert_eq!(state(&a, &format!("VALUES ST_3DLENGTH({line}, 'metre', 1)")), "42000");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.5 Description
/// 4a] ST_TINTable reads a table named by a schema and an identifier, or
/// a delimited identifier; a chain of three names reads none (2FF02); a
/// null maximum side length is 2FF03, a null SELF gives the null value;
/// too few arguments are a syntax error.
#[test]
fn tins_from_tables_named_8_6_5() {
    let (_db, a) = session();
    a.run(
        "CREATE TABLE R (ITEM VARCHAR(30), ORDERNUMBER INTEGER, X DOUBLE PRECISION, Y DOUBLE PRECISION, Z DOUBLE PRECISION, \
         TRIANGLE INTEGER ARRAY[3], VISIBILITY INTEGER, ELEMENTID INTEGER, ELEMENTTAG VARCHAR(64), ELEMENT INTEGER ARRAY[100])",
    )
    .unwrap();
    let tin = "TREAT(ST_GEOMFROMTEXT('TIN Z (PATCHES ((0 0 0, 1 0 0, 0 1 0)))') AS ST_TIN)";
    a.run(&format!("INSERT INTO R SELECT * FROM TABLE(ST_TINTABLE({tin})) AS T")).unwrap();
    let columns = "'ITEM', 'ORDERNUMBER', 'X', 'Y', 'Z', 'TRIANGLE', 'VISIBILITY', 'ELEMENTID', 'ELEMENTTAG', 'ELEMENT'";
    for table in ["'A.R'", "'\"R\"'"] {
        assert_eq!(one(&a, &format!("ST_3DAREA({tin}.ST_TINTABLE({table}, {columns}, 7)) > 0")), "TRUE", "{table}");
    }
    assert_eq!(state(&a, &format!("VALUES {tin}.ST_TINTABLE('X.A.R', {columns}, 7)")), "2FF02");
    assert_eq!(state(&a, &format!("VALUES {tin}.ST_TINTABLE('R', {columns}, CAST(NULL AS DOUBLE PRECISION))")), "2FF03");
    assert_eq!(one(&a, &format!("CAST(NULL AS ST_TIN).ST_TINTABLE('R', {columns}, 7) IS NULL")), "TRUE");
    assert_eq!(state(&a, &format!("VALUES {tin}.ST_TINTABLE('R', 'ITEM')")), "42000");
}
