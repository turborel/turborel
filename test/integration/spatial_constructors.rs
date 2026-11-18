//! The constructor methods of the geometry types through SQL [ISO/IEC
//! 13249-3:2016 (DIS, SC32 N2593) §6.1.2, §7.2.2 to §7.10.2, §8.2.2 to
//! §8.6.2, §10.1.2 to §10.6.2]: each type's representation forms and the
//! forms of its parts, `NEW ST_X(...)` (RULES.md D-SPATIAL-FAMILY).

use turborel::db::{Database, Session};
use turborel::value::Value;

use crate::common::state;

fn session() -> (Database, Session) {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE ONE (X INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO ONE VALUES (1)").unwrap();
    (db, a)
}

/// The one row `sql` selects, its values as text.
fn one(a: &Session, sql: &str) -> String {
    let query = format!("SELECT {sql} FROM ONE");
    let mut c = a.open_cursor(&query).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let row = c.fetch().unwrap_or_else(|e| panic!("{sql}: {e}")).unwrap_or_default();
    row.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
}

const NULL_DOUBLE: &str = "CAST(NULL AS DOUBLE PRECISION)";
const PLACED: &str = "NEW ST_AFFINEPLACEMENT(ST_POINT(0, 0), ARRAY[NEW ST_VECTOR(1E0, 0E0), NEW ST_VECTOR(0E0, 1E0)])";

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §6.1.2] ST_Point of
/// x, y, z and m with an SRID last: z and m add their dimensions, null x
/// and y the empty point, a null z then 2FF16, one of x and y null 2FF03,
/// as a null z of the form without m;
/// the forms without an SRID are null-call; ST_Point(1, 2, 3) is x, y and
/// the SRID 3.
#[test]
fn point_constructors_take_each_dimension() {
    let (_db, a) = session();
    assert_eq!(
        one(&a, "ST_ASTEXT(NEW ST_POINT(1E0, 2E0, 3E0)), ST_ASTEXT(NEW ST_POINT(1E0, 2E0, 3E0, 4E0))"),
        "POINT Z (1 2 3),POINT ZM (1 2 3 4)"
    );
    assert_eq!(one(&a, "NEW ST_POINT(1E0, 2E0, 3E0, 4E0, 4326).ST_SRID(), ST_POINT(1, 2, 3).ST_SRID()"), "4326,3");
    assert_eq!(one(&a, &format!("ST_ASTEXT(NEW ST_POINT(1E0, 2E0, {NULL_DOUBLE}, 4E0, 0))")), "POINT M (1 2 4)");
    let nulls = format!("{NULL_DOUBLE}, {NULL_DOUBLE}");
    assert_eq!(one(&a, &format!("ST_ASTEXT(NEW ST_POINT({nulls}, {nulls}, 0))")), "POINT EMPTY");
    assert_eq!(state(&a, &format!("VALUES NEW ST_POINT({nulls}, 3E0, {NULL_DOUBLE}, 0)")), "2FF16");
    assert_eq!(state(&a, &format!("VALUES NEW ST_POINT(1E0, {nulls}, {NULL_DOUBLE}, 0)")), "2FF03");
    assert_eq!(state(&a, &format!("VALUES NEW ST_POINT(1E0, 2E0, {NULL_DOUBLE}, 0)")), "2FF03");
    assert_eq!(one(&a, &format!("NEW ST_POINT(1E0, {NULL_DOUBLE}, 3E0) IS NULL")), "TRUE");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.2.2, §8.6.2,
/// §10.6.2] The representation forms of each type: text, binary or GML
/// with an SRID, 0 if not given.
#[test]
fn representation_forms_read_their_type() {
    let (_db, a) = session();
    assert_eq!(one(&a, "ST_ASTEXT(NEW ST_POINT('POINT Z (1 2 3)'))"), "POINT Z (1 2 3)");
    assert_eq!(one(&a, "NEW ST_LINESTRING('LINESTRING (0 0, 1 1)', 4326).ST_SRID()"), "4326");
    assert_eq!(one(&a, "ST_ASTEXT(NEW ST_LINESTRING(ST_LINEFROMTEXT('LINESTRING (0 0, 1 1)').ST_ASBINARY()))"), "LINESTRING (0 0, 1 1)");
    assert_eq!(one(&a, "ST_ASTEXT(NEW ST_MULTIPOLYGON('MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)))'))"), "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)))");
    assert_eq!(one(&a, "ST_ASTEXT(NEW ST_TIN('TIN Z (PATCHES ((0 0 0, 1 0 0, 0 1 0)))'))"), "TIN Z (PATCHES ((0 0 0, 1 0 0, 0 1 0)))");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.2.2, §7.3.2,
/// §7.4.2, §7.5.2, §7.10.2] The curves of points: an arc's points, of a
/// center, radius and angles or of bulges and their normals; a circle's
/// three or of a center, radius and normal; checked as ST_Points checks.
#[test]
fn curve_constructors_of_points() {
    let (_db, a) = session();
    let three = "ARRAY[ST_POINT(0, 0), ST_POINT(1, 1), ST_POINT(2, 0)]";
    assert_eq!(one(&a, &format!("ST_ASTEXT(NEW ST_LINESTRING({three}))")), "LINESTRING (0 0, 1 1, 2 0)");
    assert_eq!(one(&a, &format!("ST_ASTEXT(NEW ST_CIRCULARSTRING({three}))")), "CIRCULARSTRING (0 0, 1 1, 2 0)");
    let bulged = "NEW ST_CIRCULARSTRING(ARRAY[ST_POINT(0, 0), ST_POINT(2, 0)], ARRAY[1E0], ARRAY[NEW ST_VECTOR(0E0, 0E0, 1E0)])";
    assert_eq!(one(&a, &format!("{bulged}.ST_NUMPOINTS()")), "3");
    let centered = "NEW ST_CIRCULARSTRING(ST_POINT(0, 0), 1E0, NEW ST_ANGLE(0E0), NEW ST_ANGLE(1.5707963267948966E0))";
    assert_eq!(one(&a, &format!("ST_ASTEXT({centered}.ST_STARTPOINT()), {centered}.ST_NUMPOINTS()")), "POINT (1 0),3");
    assert_eq!(one(&a, "ST_ASTEXT(NEW ST_CIRCLE(ARRAY[ST_POINT(1, 0), ST_POINT(0, 1), ST_POINT(-1, 0)]))"), "CIRCLE (1 0, 0 1, -1 0)");
    assert_eq!(one(&a, "ST_ASTEXT(NEW ST_CIRCLE(ST_POINT(0, 0), 1E0, NEW ST_VECTOR(0E0, 0E0, 1E0)).ST_STARTPOINT())"), "POINT (0 1)");
    assert_eq!(state(&a, &format!("VALUES NEW ST_CIRCLE({three}, 0).ST_POINTS(ARRAY[ST_POINT(0, 0), ST_POINT(1, 1)])")), "2FF77");
    assert_eq!(one(&a, "NEW ST_GEODESICSTRING(ARRAY[ST_POINT(0, 0, 4326), ST_POINT(1, 1, 4326)], 4326).ST_SRID()"), "4326");
    let parts = "ARRAY[ST_LINEFROMTEXT('LINESTRING (0 0, 1 0)'), ST_LINEFROMTEXT('LINESTRING (1 0, 2 2)')]";
    assert_eq!(one(&a, &format!("ST_ASTEXT(NEW ST_COMPOUNDCURVE({parts}))")), "COMPOUNDCURVE ((0 0, 1 0), (1 0, 2 2))");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.6.2, §7.7.2,
/// §7.8.2, §7.9.2] The curves of parameters with measures, a unit and an
/// SRID: lengths converted from the unit; one measure without the other
/// 2FF84; a null parameter 2FF03 of the defining form, null of another.
#[test]
fn parametric_constructors_take_units_and_measures() {
    let (_db, a) = session();
    let angles = "NEW ST_ANGLE(0E0), NEW ST_ANGLE(1E0)";
    let e = format!("NEW ST_ELLIPTICALCURVE({PLACED}, 2E0, 1E0, {angles}, 1E0, 2E0, 'KILOMETRE', 0)");
    assert_eq!(one(&a, &format!("{e}.ST_UAXISLENGTH(), {e}.ST_ENDM()")), "2E3,2E0");
    assert_eq!(state(&a, &format!("VALUES NEW ST_ELLIPTICALCURVE({PLACED}, 2E0, 1E0, {angles}, 1E0, {NULL_DOUBLE}, 0)")), "2FF84");
    assert_eq!(state(&a, &format!("VALUES NEW ST_ELLIPTICALCURVE({PLACED}, {NULL_DOUBLE}, 1E0, {angles}, 1E0, 2E0, 0)")), "2FF03");
    assert_eq!(one(&a, &format!("NEW ST_ELLIPTICALCURVE({PLACED}, {NULL_DOUBLE}, 1E0, {angles}) IS NULL")), "TRUE");
    assert_eq!(one(&a, &format!("NEW ST_CLOTHOID({PLACED}, 10E0, 0E0, 5E0).ST_SCALEFACTOR()")), "1E1");
    assert_eq!(one(&a, &format!("NEW ST_SPIRALCURVE({PLACED}, 10E0, 0E0, 0.1E0, 'clothoid', 4326).ST_SRID()")), "4326");
    let points =
        "ARRAY[NEW ST_NURBSPOINT(ST_POINT(0, 0), 1E0), NEW ST_NURBSPOINT(ST_POINT(2, 2), 2E0), NEW ST_NURBSPOINT(ST_POINT(2, 0), 1E0)]";
    let knots = "ARRAY[NEW ST_KNOT(0E0, 3), NEW ST_KNOT(1E0, 3)]";
    assert_eq!(one(&a, &format!("NEW ST_NURBSCURVE(2, {points}, {knots}).ST_DEGREE()")), "2");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.2.2 to §8.5.2,
/// §10.1.2 to §10.6.2] The surfaces of rings and patches and the
/// collections of elements: a collection of one geometry takes its SRID,
/// a multipoint of an array the SRID 0 unless given.
#[test]
fn surface_and_collection_constructors() {
    let (_db, a) = session();
    let ring = "ST_LINEFROMTEXT('LINESTRING (0 0, 8 0, 8 8, 0 0)')";
    let hole = "ARRAY[ST_LINEFROMTEXT('LINESTRING (5 1, 7 1, 7 3, 5 1)')]";
    assert_eq!(one(&a, &format!("NEW ST_POLYGON({ring}, {hole}).ST_NUMINTERIORRING()")), "1");
    assert_eq!(one(&a, &format!("ST_ASTEXT(NEW ST_CURVEPOLYGON({ring}))")), "CURVEPOLYGON ((0 0, 8 0, 8 8, 0 0))");
    assert_eq!(
        one(&a, "ST_ASTEXT(NEW ST_TRIANGLE(ARRAY[ST_POINT(0, 0), ST_POINT(2, 0), ST_POINT(0, 2), ST_POINT(0, 0)]))"),
        "TRIANGLE (0 0, 2 0, 0 2)"
    );
    let patches =
        "ARRAY[ST_POLYFROMTEXT('POLYGON Z ((0 0 0, 1 0 0, 1 1 0, 0 0 0))'), ST_POLYFROMTEXT('POLYGON Z ((0 0 0, 1 1 0, 0 1 0, 0 0 0))')]";
    assert_eq!(one(&a, &format!("NEW ST_POLYHDRLSURFACE({patches}).ST_NUMPATCHES()")), "2");
    assert_eq!(one(&a, "NEW ST_GEOMCOLLECTION(ST_POINT(1, 2, 4326)).ST_SRID()"), "4326");
    assert_eq!(one(&a, "NEW ST_MULTIPOINT(ARRAY[ST_POINT(1, 2, 4326)], 4326).ST_NUMGEOMETRIES()"), "1");
    assert_eq!(state(&a, "VALUES NEW ST_MULTIPOINT(ARRAY[ST_POINT(1, 2, 4326)])"), "2FF10");
}
