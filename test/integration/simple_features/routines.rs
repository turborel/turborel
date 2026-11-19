//! [OGC 06-104r4 §7.2] The routines of Simple feature access by the names
//! it writes them with, where they differ from SQL/MM's: GeometryType,
//! AsText and AsBinary, PolyhedSurface's, and the forms of OGC 06-103r4.

use turborel::error::Code;

use super::{database, run};
use crate::common::rows;

/// A polyhedral surface of two patches, in OGC 06-103r4's text.
const SURFACE: &str = "'POLYHEDRALSURFACE Z (((0 0 0, 0 1 0, 1 1 0, 0 0 0)), ((0 0 0, 0 1 0, 0 0 1, 0 0 0)))'";
/// A triangle and a TIN in OGC 06-103r4's text.
const TRIANGLE: &str = "'TRIANGLE ((0 0, 1 0, 0 1, 0 0))'";
const TIN: &str = "'TIN (((0 0, 1 0, 0 1, 0 0)))'";

fn one(sql: &str) -> Vec<String> {
    let db = database();
    rows(&db.session("A"), sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

/// Verifies: [OGC 06-104r4 §7.2.2.2 Note, §7.2.8 to §7.2.13] Each routine
/// of a value, and each constructor, is called by its name without ST_ as
/// SQL/MM's of the name with it: Z, M, CoordDim, Is3D, IsMeasured,
/// LocateAlong, LocateBetween, NumPatches and PatchN, the constructors of
/// text and binary, of each type, of polygons from boundaries, and
/// WKTToSQL and WKBToSQL.
#[test]
fn routines_are_called_without_st() {
    let z = "GeomFromText('POINT Z (1 2 3)')";
    let sql = format!("VALUES (CoordDim({z}), Z({z}), M(GeomFromText('POINT M (1 2 3)')), Is3D({z}), IsMeasured({z}))");
    assert_eq!(one(&sql), ["3,3E0,3E0,1,0"]);
    let m = "GeomFromText('LINESTRING M (0 0 0, 10 0 10)')";
    let sql = format!("VALUES (AsText(LocateAlong({m}, 5)), AsText(LocateBetween({m}, 2, 4)), NumPatches(GeomFromText({TIN})))");
    assert_eq!(one(&sql), ["MULTIPOINT M ((5 0 5)),MULTICURVE M ((2 0 2, 4 0 4)),1"]);
    let constructed = [
        ("PointFromText('POINT (1 1)')", "POINT (1 1)"),
        ("MPointFromText('MULTIPOINT (1 1)')", "MULTIPOINT ((1 1))"),
        ("MLineFromText('MULTILINESTRING ((0 0, 1 1))')", "MULTILINESTRING ((0 0, 1 1))"),
        ("MPolyFromText('MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)))')", "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)))"),
        ("GeomCollFromText('GEOMETRYCOLLECTION (POINT (1 1))')", "GEOMETRYCOLLECTION (POINT (1 1))"),
        ("GeomCollFromTxt('GEOMETRYCOLLECTION EMPTY')", "GEOMETRYCOLLECTION EMPTY"),
        ("PointFromWKB(AsBinary(GeomFromText('POINT (1 1)')))", "POINT (1 1)"),
        ("MPointFromWKB(AsBinary(GeomFromText('MULTIPOINT (1 1)')))", "MULTIPOINT ((1 1))"),
        ("MLineFromWKB(AsBinary(GeomFromText('MULTILINESTRING ((0 0, 1 1))')))", "MULTILINESTRING ((0 0, 1 1))"),
        ("MPolyFromWKB(AsBinary(GeomFromText('MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)))')))", "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)))"),
        ("GeomCollFromWKB(AsBinary(GeomFromText('GEOMETRYCOLLECTION (POINT (1 1))')))", "GEOMETRYCOLLECTION (POINT (1 1))"),
        ("GeomFromWKB(AsBinary(GeomFromText('POINT (1 1)')), 4326)", "POINT (1 1)"),
        ("BdPolyFromText('MULTILINESTRING ((0 0, 1 0, 1 1, 0 0))')", "POLYGON ((0 0, 1 0, 1 1, 0 0))"),
        ("BdMPolyFromText('MULTILINESTRING ((0 0, 1 0, 1 1, 0 0))')", "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)))"),
        ("BdPolyFromWKB(AsBinary(GeomFromText('MULTILINESTRING ((0 0, 1 0, 1 1, 0 0))')))", "POLYGON ((0 0, 1 0, 1 1, 0 0))"),
        ("BdMPolyFromWKB(AsBinary(GeomFromText('MULTILINESTRING ((0 0, 1 0, 1 1, 0 0))')))", "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)))"),
        ("WKTToSQL('POINT (1 1)')", "POINT (1 1)"),
        ("WKBToSQL(AsBinary(GeomFromText('POINT (1 1)')))", "POINT (1 1)"),
    ];
    for (value, text) in constructed {
        assert_eq!(one(&format!("VALUES (AsText({value}))")), [text], "{value}");
    }
}

/// Verifies: [OGC 06-104r4 §7.2.8, Annex C.3.3.3 T7] GeometryType names a
/// value's type as Simple feature access does, and ST_GeometryType as
/// SQL/MM does; a null is null.
#[test]
fn geometry_type_names_types_without_st() {
    let sql = format!(
        "VALUES (GeometryType(GeomFromText('GEOMETRYCOLLECTION EMPTY')), GeometryType(GeomFromText('CIRCULARSTRING (0 0, 1 1, 2 0)')), \
         GeometryType(GeomFromText({TRIANGLE})), GeometryType(GeomFromText({SURFACE})), GeometryType(GeomFromText({TIN})), \
         ST_GeometryType(GeomFromText('POINT (1 1)')), GeometryType(CAST(NULL AS POINT)))"
    );
    assert_eq!(one(&sql), ["GEOMCOLLECTION,CIRCULARSTRING,TRIANGLE,POLYHEDSURFACE,TIN,ST_Point,NULL"]);
}

/// Verifies: [OGC 06-104r4 §7.2.8] A routine of a value of a type that
/// does not declare it is that of the one type below that does, of
/// TREAT of the value as it, or SQL/MM's function of the name with ST_;
/// a value not of that type raises the routine's exception. Union,
/// Overlaps and Intersection of two values are routines' calls, and a
/// routine of a geometry is chosen before full-text's CONTAINS.
#[test]
fn routines_of_a_supertype_and_of_key_words() {
    let db = database();
    let a = db.session("A");
    run(
        &a,
        &[
            "CREATE TABLE T (ID INTEGER, G GEOMETRY)",
            "INSERT INTO T VALUES (1, GeomFromText('MULTIPOINT ((1 1), (2 2))')), (2, GeomFromText('POINT (1 1)')), \
             (3, GeomFromText('POLYGON ((0 0, 1 0, 1 1, 0 0))'))",
        ],
    );
    assert_eq!(rows(&a, "SELECT NumGeometries(G) FROM T WHERE ID = 1").unwrap(), ["2"]);
    assert_eq!(rows(&a, "SELECT Area(G), AsText(ExteriorRing(G)) FROM T WHERE ID = 3").unwrap(), ["5E-1,LINESTRING (0 0, 1 0, 1 1, 0 0)"]);
    assert_eq!(rows(&a, "SELECT NumGeometries(G) FROM T WHERE ID = 2").unwrap_err(), Code::Data);
    let p = "GeomFromText('POINT (1 1)')";
    let q = "GeomFromText('POINT (2 2)')";
    let sql = format!("VALUES (AsText(Union({p}, {q})), Overlaps({p}, {q}), AsText(Intersection({p}, {p})), AsText(\"UNION\"({p}, {q})))");
    assert_eq!(rows(&a, &sql).unwrap(), ["MULTIPOINT ((1 1), (2 2)),0,POINT (1 1),MULTIPOINT ((1 1), (2 2))"]);
    let contains = "VALUES (CONTAINS(GeomFromText('POLYGON ((0 0, 4 0, 4 4, 0 0))'), GeomFromText('POINT (3 1)')))";
    assert_eq!(rows(&a, contains).unwrap(), ["1"]);
    assert_eq!(rows(&a, "VALUES (Dimension(1))").unwrap_err(), Code::SyntaxRule);
}

/// Verifies: [OGC 06-104r4 §7.2.14] PolyhedSurface names
/// ST_PolyhdrlSurface; its constructors take the text, the binary
/// representation, or an array of surfaces, each with an SRID or none;
/// NumSurfaces and ST_NumGeometries count its patches, SURFACE and
/// ST_GeometryN return one, ST_Geometries all, as methods or functions,
/// and the routines of a surface and a geometry are its.
#[test]
fn polyhedral_surfaces_are_counted_by_their_patches() {
    let db = database();
    let a = db.session("A");
    run(&a, &["CREATE TABLE T (H POLYHEDSURFACE(4326))", &format!("INSERT INTO T VALUES (NEW PolyhedSurface({SURFACE}, 4326))")]);
    let methods = "SELECT H.NumSurfaces(), AsText(H.SURFACE(2)), H.ST_NumGeometries(), AsText(H.ST_GeometryN(1)), CARDINALITY(H.ST_Geometries()) FROM T";
    assert_eq!(rows(&a, methods).unwrap(), ["2,POLYGON Z ((0 0 0, 0 1 0, 0 0 1, 0 0 0)),2,POLYGON Z ((0 0 0, 0 1 0, 1 1 0, 0 0 0)),2"]);
    let functions =
        "SELECT NumSurfaces(H), AsText(Surface(H, 1)), NumGeometries(H), AsText(GeometryN(H, 2)), Area(H), Dimension(H), SRID(H) FROM T";
    assert_eq!(
        rows(&a, functions).unwrap(),
        ["2,POLYGON Z ((0 0 0, 0 1 0, 1 1 0, 0 0 0)),2,POLYGON Z ((0 0 0, 0 1 0, 0 0 1, 0 0 0)),5E-1,2,4326"]
    );
    let made = "SELECT AsText(NEW PolyhedSurface(AsBinary(H))) = AsText(H), SRID(NEW PolyhedSurface(AsBinary(H), 4326)), \
                SRID(NEW PolyhedSurface(H.ST_Geometries(), 4326)), SRID(NEW PolyhedSurface(";
    let made = format!("{made}{SURFACE})) FROM T");
    assert_eq!(rows(&a, &made).unwrap(), ["TRUE,4326,4326,0"]);
    assert_eq!(rows(&a, "SELECT GeometryType(H) FROM T").unwrap(), ["POLYHEDSURFACE"]);
}

/// Verifies: [OGC 06-104r4 §7.2.6, §7.2.7; OGC 06-103r4 §7.2, §8.2.8]
/// AsText and AsBinary write a triangle, polyhedral surface and TIN in
/// the forms of OGC 06-103r4, ST_AsText and ST_AsBinary in SQL/MM's; the
/// readers take both; a triangle's text of a ring not closed, or of two
/// rings, is none.
#[test]
fn part_one_forms_are_written_and_read() {
    let sql = format!(
        "VALUES (AsText(GeomFromText({TRIANGLE})), ST_AsText(GeomFromText({TRIANGLE})), AsText(GeomFromText({TIN})), \
         ST_AsText(GeomFromText({TIN})), AsText(GeomFromText({SURFACE})))"
    );
    assert_eq!(
        one(&sql),
        ["TRIANGLE ((0 0, 1 0, 0 1, 0 0)),TRIANGLE (0 0, 1 0, 0 1),TIN (((0 0, 1 0, 0 1, 0 0))),TIN (PATCHES ((0 0, 1 0, 0 1))),\
             POLYHEDRALSURFACE Z (((0 0 0, 0 1 0, 1 1 0, 0 0 0)), ((0 0 0, 0 1 0, 0 0 1, 0 0 0)))"]
    );
    let ring = "0400000000000000000000000000000000000000000000000000F03F00000000000000000000000000000000000000000000F03F00000000000000000000000000000000";
    let points = "00000000000000000000000000000000000000000000F03F00000000000000000000000000000000000000000000F03F";
    let sql = format!("VALUES (AsBinary(GeomFromText({TRIANGLE})), ST_AsBinary(GeomFromText({TRIANGLE})), AsBinary(GeomFromText({TIN})))");
    assert_eq!(
        one(&sql),
        [format!("011100000001000000{ring},0111000000{points},0110000000010000000103000000010000000{}", ring.get(1..).unwrap())]
    );
    let sql = format!("VALUES (AsText(GeomFromWKB(AsBinary(GeomFromText({TIN})))), AsText(GeomFromWKB(ST_AsBinary(GeomFromText({TIN})))))");
    assert_eq!(one(&sql), ["TIN (((0 0, 1 0, 0 1, 0 0))),TIN (((0 0, 1 0, 0 1, 0 0)))"]);
    let db = database();
    let a = db.session("A");
    for bad in ["TRIANGLE ((0 0, 1 0, 0 1, 0 1))", "TRIANGLE ((0 0, 1 0, 0 1, 0 0), (0 0, 1 0, 0 1, 0 0))", "TIN (((0 0, 1 0, 0 0)))"] {
        assert_eq!(rows(&a, &format!("VALUES (GeomFromText('{bad}'))")).unwrap_err(), Code::Data, "{bad}");
    }
}

/// Verifies: [OGC 06-104r4 §7.2.2.2] A structured type a schema defines of
/// a name of Simple feature access is the type that name names there, in
/// a column's type and in NEW, and its column no geometry column.
#[test]
fn a_structured_type_of_the_name_is_that_type() {
    let db = database();
    let a = db.session("A");
    run(
        &a,
        &["CREATE TYPE POINT AS (X INTEGER) NOT FINAL", "CREATE TABLE S (P POINT, Q ST_POINT)", "INSERT INTO S VALUES (NEW POINT(), NULL)"],
    );
    assert_eq!(rows(&a, "SELECT P.X, Q IS NULL FROM S").unwrap(), ["NULL,TRUE"]);
    assert_eq!(rows(&a, "SELECT F_GEOMETRY_COLUMN FROM GEOMETRY_COLUMNS").unwrap(), ["Q"]);
    let b = db.session("B");
    assert_eq!(rows(&b, "VALUES (AsText(NEW POINT(1, 2)))").unwrap(), ["POINT (1 2)"]);
}
