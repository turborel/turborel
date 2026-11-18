//! The routines that ignore z, of a solid and of text CAST to a
//! geometry type, through SQL [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §5.1.8, §5.1.10, §5.1.29 to §5.1.54, §5.1.62]: a solid as the region
//! of x and y it covers, and text of any type and coordinate dimension
//! (RULES.md D119, D120).

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

/// The cube of side 3 at the origin, as a B-rep solid of its six faces.
fn cube() -> String {
    let faces = [
        "((0 0 0, 0 3 0, 3 3 0, 3 0 0, 0 0 0))",
        "((0 0 3, 3 0 3, 3 3 3, 0 3 3, 0 0 3))",
        "((0 0 0, 3 0 0, 3 0 3, 0 0 3, 0 0 0))",
        "((0 3 0, 0 3 3, 3 3 3, 3 3 0, 0 3 0))",
        "((0 0 0, 0 0 3, 0 3 3, 0 3 0, 0 0 0))",
        "((3 0 0, 3 3 0, 3 3 3, 3 0 3, 3 0 0))",
    ];
    let patches: Vec<String> = faces.iter().map(|f| format!("POLYGON {f}")).collect();
    format!("ST_GEOMFROMTEXT('BREPSOLID Z (POLYHEDRALSURFACE Z (PATCHES ({})))')", patches.join(", "))
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.31 to §5.1.54,
/// Tables 8 to 11] The relationships, set operations and distance of a
/// solid ignore z: the solid is the square its faces cover in x and y.
#[test]
fn a_solid_relates_as_its_projection() {
    let (_db, a) = session();
    let c = cube();
    assert_eq!(
        one(&a, &format!("ST_INTERSECTS({c}, ST_POINT(1, 1)), ST_CONTAINS({c}, ST_POINT(1, 1)), ST_TOUCHES({c}, ST_POINT(3, 1))")),
        "1,1,1"
    );
    assert_eq!(one(&a, &format!("ST_DISJOINT({c}, ST_POINT(5, 1)), ST_DISTANCE({c}, ST_POINT(5, 1)), ST_EQUALS({c}, {c})")), "1,2E0,1");
    let square = "ST_GEOMFROMTEXT('POLYGON ((0 0, 3 0, 3 3, 0 3, 0 0))')";
    assert_eq!(one(&a, &format!("ST_EQUALS({c}, {square}), ST_RELATE({c}, {square}, '2FFF1FFF2')")), "1,1");
    let over = "ST_GEOMFROMTEXT('POLYGON ((1 1, 5 1, 5 5, 1 5, 1 1))')";
    assert_eq!(one(&a, &format!("ST_AREA(ST_INTERSECTION({c}, {over})), ST_AREA(ST_DIFFERENCE({c}, {over}))")), "4E0,5E0");
    assert_eq!(one(&a, &format!("ST_ASTEXT(ST_BOUNDARY({c}))")), "LINESTRING (0 3, 0 0, 3 0, 3 3, 0 3)");
    assert_eq!(one(&a, &format!("ST_ASTEXT(ST_CONVEXHULL({c}))")), "POLYGON ((0 0, 3 0, 3 3, 0 3, 0 0))");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.8, §5.1.10,
/// §9.2.1 Description 8 to 10] A solid is valid when its shells are
/// closed and simple, and simple as its patches meet only along their
/// boundaries; an open shell is not valid.
#[test]
fn a_solid_is_valid_and_simple_in_space() {
    let (_db, a) = session();
    let c = cube();
    assert_eq!(one(&a, &format!("ST_ISVALID({c}), ST_ISSIMPLE({c})")), "1,1");
    let open = c.replace(", POLYGON ((3 0 0, 3 3 0, 3 3 3, 3 0 3, 3 0 0))", "");
    assert_eq!(one(&a, &format!("ST_ISVALID({open}), ST_ISSIMPLE({open})")), "0,1");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.62, §5.1.68]
/// D120: text CAST to a geometry type is read as ST_GeomFromText reads it,
/// of any type and coordinate dimension; a ring closed by -0 and 0 is
/// closed.
#[test]
fn text_cast_reads_every_type() {
    let (_db, a) = session();
    assert_eq!(one(&a, "ST_ASTEXT(CAST('POINT ZM (1 2 3 4)' AS GEOMETRY))"), "POINT ZM (1 2 3 4)");
    assert_eq!(one(&a, "ST_ASTEXT(CAST('CIRCULARSTRING (0 0, 1 1, 2 0)' AS ST_CURVE))"), "CIRCULARSTRING (0 0, 1 1, 2 0)");
    assert_eq!(state(&a, "VALUES CAST('POINT (1 2) x' AS GEOMETRY)"), "2FF22");
    assert_eq!(one(&a, "ST_ASTEXT(ST_GEOMFROMTEXT('POLYGON ((-0 0, 1 0, 1 1, 0 0))'))"), "POLYGON ((0 0, 1 0, 1 1, 0 0))");
}
