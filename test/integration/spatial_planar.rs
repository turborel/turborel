//! The planar engine's routines through SQL [ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §5.1, §7.1, §8.1, §8.3, §10.3, §10.5, §10.6]: the
//! relationships and ST_Relate, the set operations and their result
//! types, ST_Buffer, ST_ConvexHull, ST_Boundary, validity, the measures
//! with their unit forms, the polygons built from rings, curves through
//! their linear form, and R-tree plans for the new relationships
//! (RULES.md D-SPATIAL-PLANAR).

use turborel::db::Database;
use turborel::params::Params;
use turborel::value::Value;

use crate::common::{rows, state};

fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, G ST_GEOMETRY)").unwrap();
    db
}

fn one(db: &Database, sql: &str) -> String {
    let a = db.session("A");
    let mut found = rows(&a, &format!("VALUES {sql}")).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    assert_eq!(found.len(), 1, "{sql}");
    found.remove(0)
}

const SQUARE: &str = "ST_GEOMFROMTEXT('POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))')";
const INNER: &str = "ST_GEOMFROMTEXT('POLYGON ((1 1, 2 1, 2 2, 1 2, 1 1))')";
const SHIFTED: &str = "ST_GEOMFROMTEXT('POLYGON ((2 2, 6 2, 6 6, 2 6, 2 2))')";
const EMPTY: &str = "ST_GEOMFROMTEXT('POINT EMPTY')";
/// CIRCULARSTRING (0 0, 1 1, 2 0) in well-known binary.
const ARC: &str =
    "ST_GEOMFROMWKB(X'01080000000300000000000000000000000000000000000000000000000000F03F000000000000F03F00000000000000400000000000000000')";

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.43, §5.1.45, §5.1.50 to §5.1.54]
/// the named relationships of the DE-9IM, as functions and as methods;
/// ST_Relate's matrix, its invalid characters (2FF04) checked only of
/// nonempty values; the null value of an empty operand, but two empty
/// sets are equal; mixed SRIDs refused.
#[test]
fn relationships_follow_the_de9im() {
    let db = database();
    let cases = [
        (format!("ST_CONTAINS({SQUARE}, {INNER})"), "1"),
        (format!("ST_WITHIN({INNER}, {SQUARE})"), "1"),
        (format!("ST_WITHIN({SQUARE}, {INNER})"), "0"),
        (format!("ST_OVERLAPS({SQUARE}, {SHIFTED})"), "1"),
        (format!("ST_TOUCHES({SQUARE}, ST_POINT(4, 2))"), "1"),
        ("ST_TOUCHES(ST_POINT(4, 2), ST_POINT(4, 2))".to_owned(), "NULL"),
        (format!("ST_CROSSES(ST_GEOMFROMTEXT('LINESTRING (-1 2, 5 2)'), {SQUARE})"), "1"),
        (format!("ST_EQUALS({SQUARE}, ST_GEOMFROMTEXT('POLYGON ((4 4, 0 4, 0 0, 2 0, 4 0, 4 4))'))"), "1"),
        (format!("ST_EQUALS({EMPTY}, ST_GEOMFROMTEXT('LINESTRING EMPTY'))"), "1"),
        (format!("ST_CONTAINS({SQUARE}, {EMPTY})"), "NULL"),
        (format!("ST_INTERSECTS({SQUARE}, {EMPTY})"), "NULL"),
        (format!("ST_DISJOINT({SQUARE}, {EMPTY})"), "NULL"),
        (format!("ST_RELATE({SQUARE}, {SHIFTED}, '212101212')"), "1"),
        (format!("ST_RELATE({SQUARE}, {SHIFTED}, 'T*T***T**')"), "1"),
        (format!("ST_RELATE({SQUARE}, {INNER}, 'FF*FF****')"), "0"),
        (format!("ST_RELATE({SQUARE}, {EMPTY}, 'XYZ')"), "NULL"),
        (format!("({SQUARE}).ST_CONTAINS({INNER})"), "1"),
        (format!("({SQUARE}).ST_RELATE({INNER}, 'T*****FF*')"), "1"),
    ];
    for (sql, expected) in cases {
        assert_eq!(one(&db, &sql), expected, "{sql}");
    }
    let a = db.session("A");
    assert_eq!(state(&a, &format!("VALUES ST_RELATE({SQUARE}, {INNER}, 'TTTTTTTTX')")), "2FF04");
    assert_eq!(state(&a, &format!("VALUES ST_RELATE({SQUARE}, {INNER}, 'TT')")), "2FF04");
    assert!(rows(&a, &format!("VALUES ST_WITHIN({INNER}, ST_SETSRID({SQUARE}, 4326))")).is_err());
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.31 to §5.1.39, Tables 8 to 11]
/// the set operations as point sets, their types by the tables (R00's
/// empty point, R04's multipoint, R07's collection), an empty operand
/// giving the other operand, and the result in SELF's system.
#[test]
fn set_operations_and_their_result_types() {
    let db = database();
    let text = |sql: &str| one(&db, &format!("ST_ASTEXT({sql})"));
    let kind = |sql: &str| one(&db, &format!("ST_GEOMETRYTYPE({sql})"));
    assert_eq!(one(&db, &format!("ST_AREA(ST_INTERSECTION({SQUARE}, {SHIFTED}))")), "4E0");
    assert_eq!(one(&db, &format!("ST_AREA(ST_UNION({SQUARE}, {SHIFTED}))")), "2.8E1");
    assert_eq!(one(&db, &format!("ST_AREA(ST_DIFFERENCE({SQUARE}, {SHIFTED}))")), "1.2E1");
    assert_eq!(one(&db, &format!("ST_AREA(ST_SYMDIFFERENCE({SQUARE}, {SHIFTED}))")), "2.4E1");
    assert_eq!(kind(&format!("ST_SYMDIFFERENCE({SQUARE}, {SHIFTED})")), "ST_MultiPolygon");
    assert_eq!(text(&format!("ST_INTERSECTION({SQUARE}, ST_POINT(9, 9))")), "POINT EMPTY");
    assert_eq!(kind("ST_UNION(ST_POINT(1, 1), ST_GEOMFROMTEXT('MULTIPOINT ((1 1))'))"), "ST_MultiPoint");
    assert_eq!(kind("ST_UNION(ST_POINT(1, 1), ST_GEOMFROMTEXT('GEOMETRYCOLLECTION (POINT (1 1))'))"), "ST_GeomCollection");
    assert_eq!(kind(&format!("ST_UNION({EMPTY}, ST_GEOMFROMTEXT('MULTIPOLYGON (((0 0, 1 0, 0 1, 0 0)))'))")), "ST_MultiPolygon");
    assert_eq!(kind("ST_DIFFERENCE(ST_GEOMFROMTEXT('MULTIPOLYGON (((0 0, 1 0, 0 1, 0 0)))'), ST_POINT(0, 0))"), "ST_MultiPolygon");
    assert_eq!(text("ST_INTERSECTION(ST_GEOMFROMTEXT('LINESTRING (0 0, 2 2)'), ST_GEOMFROMTEXT('LINESTRING (0 2, 2 0)'))"), "POINT (1 1)");
    assert_eq!(one(&db, &format!("ST_SRID(ST_UNION(ST_SETSRID({SQUARE}, 7), ST_SETSRID({INNER}, 7)))")), "7");
    assert_eq!(one(&db, &format!("ST_AREA(({SQUARE}).ST_UNION({SHIFTED}))")), "2.8E1");
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.17, §5.1.29, §5.1.30] ST_Buffer
/// covers what lies within the distance and is null of an empty value,
/// its unit form converts the distance and refuses an unknown unit
/// (2FF14); ST_ConvexHull and ST_Boundary of values and the null value
/// of an empty one.
#[test]
fn buffers_hulls_and_boundaries() {
    let db = database();
    assert_eq!(one(&db, "ST_CONTAINS(ST_BUFFER(ST_POINT(0, 0), 2), ST_MAKEENVELOPE(0, 0, 1, 1))"), "1");
    assert_eq!(one(&db, "ST_CONTAINS(ST_BUFFER(ST_POINT(0, 0), 2), ST_POINT(2, 1))"), "0");
    assert_eq!(one(&db, &format!("ST_BUFFER({EMPTY}, 1)")), "NULL");
    assert_eq!(one(&db, &format!("ST_EQUALS(ST_BUFFER({SQUARE}, 0), {SQUARE})")), "1");
    let area = one(&db, "ST_AREA(ST_BUFFER(ST_GEOMFROMTEXT('LINESTRING (0 0, 10 0)'), 0.001, 'KILOMETRE'))");
    let area: f64 = area.parse().unwrap();
    assert!(area > 20.0 + 3.1 && area < 20.0 + std::f64::consts::PI, "{area}");
    let a = db.session("A");
    assert_eq!(state(&a, "VALUES ST_BUFFER(ST_POINT(0, 0), 1, 'FURLONG')"), "2FF14");
    assert_eq!(state(&a, "VALUES ST_BUFFER(ST_POINT(0, 0, 4326), 1, 'METRE')"), "2FF14");
    assert_eq!(
        one(&db, "ST_ASTEXT(ST_CONVEXHULL(ST_GEOMFROMTEXT('MULTIPOINT ((0 0), (2 0), (1 1), (0 2))')))"),
        "POLYGON ((0 0, 2 0, 0 2, 0 0))"
    );
    assert_eq!(one(&db, "ST_ASTEXT(ST_BOUNDARY(ST_GEOMFROMTEXT('LINESTRING (0 0, 1 0, 1 1)')))"), "MULTIPOINT ((0 0), (1 1))");
    assert_eq!(one(&db, "ST_ASTEXT(ST_BOUNDARY(ST_POINT(1, 1)))"), "POINT EMPTY");
    assert_eq!(one(&db, &format!("ST_CONVEXHULL({EMPTY})")), "NULL");
    assert_eq!(one(&db, &format!("ST_BOUNDARY({EMPTY})")), "NULL");
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.8, §5.1.10, §7.1.8] ST_IsValid
/// and ST_IsSimple, 1 of an empty value; ST_IsRing of curves only,
/// 0 of an empty one, as a method of ST_Curve and of its subtypes.
#[test]
fn validity_simplicity_and_rings() {
    let db = database();
    assert_eq!(one(&db, &format!("ST_ISVALID({SQUARE})")), "1");
    assert_eq!(one(&db, "ST_ISVALID(ST_GEOMFROMTEXT('POLYGON ((0 0, 2 2, 2 0, 0 2, 0 0))'))"), "0");
    assert_eq!(one(&db, &format!("ST_ISVALID({EMPTY})")), "1");
    assert_eq!(one(&db, "ST_ISSIMPLE(ST_GEOMFROMTEXT('LINESTRING (0 0, 2 2, 2 0, 0 2)'))"), "0");
    assert_eq!(one(&db, "ST_ISSIMPLE(ST_GEOMFROMTEXT('LINESTRING (0 0, 2 2)'))"), "1");
    assert_eq!(one(&db, "ST_ISRING(ST_GEOMFROMTEXT('LINESTRING (0 0, 1 0, 1 1, 0 0)'))"), "1");
    assert_eq!(one(&db, "ST_ISRING(ST_GEOMFROMTEXT('LINESTRING (0 0, 1 0, 1 1)'))"), "0");
    assert_eq!(one(&db, "ST_ISRING(ST_GEOMFROMTEXT('LINESTRING EMPTY'))"), "0");
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE L (C ST_LINESTRING)").unwrap();
    let b = db.session("B");
    b.run("INSERT INTO L VALUES (TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 1 0, 1 1, 0 0)') AS ST_LINESTRING))").unwrap();
    assert_eq!(rows(&b, "SELECT C.ST_ISRING(), C.ST_ISSIMPLE(), C.ST_LENGTH() FROM L").unwrap(), ["1,1,3.414213562373095E0"]);
    assert_eq!(state(&b, "VALUES ST_ISRING(ST_POINT(0, 0))"), "2FF02");
}

/// Verifies: [ISO/IEC 13249-3:2016 §7.1.2, §8.1.2 to §8.1.8, §10.3.5,
/// §10.5.3 to §10.5.9] the measures of curves and surfaces with their
/// unit forms, the centroid and a point on the surface, the null value of
/// an empty value, another type refused (2FF02), and a geographic system
/// refusing a planar unit form (2FF14).
#[test]
fn measures_and_their_units() {
    let db = database();
    let holed = "ST_GEOMFROMTEXT('POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2))')";
    assert_eq!(one(&db, &format!("ST_AREA({holed})")), "9.6E1");
    assert_eq!(one(&db, &format!("ST_PERIMETER({holed})")), "4.8E1");
    assert_eq!(one(&db, &format!("ST_AREA({holed}, 'KILOMETRE')")), "9.6E-5");
    assert_eq!(one(&db, &format!("ST_PERIMETER({holed}, 'KILOMETRE')")), "4.8E-2");
    assert_eq!(one(&db, "ST_LENGTH(ST_GEOMFROMTEXT('MULTILINESTRING ((0 0, 3 4), (0 0, 0 1))'))"), "6E0");
    assert_eq!(one(&db, "ST_LENGTH(ST_GEOMFROMTEXT('LINESTRING (0 0, 1000 0)'), 'KILOMETRE')"), "1E0");
    assert_eq!(one(&db, &format!("ST_ASTEXT(ST_CENTROID({SQUARE}))")), "POINT (2 2)");
    assert_eq!(one(&db, &format!("ST_WITHIN(ST_POINTONSURFACE({holed}), {holed})")), "1");
    assert_eq!(one(&db, "ST_AREA(ST_GEOMFROMTEXT('POLYGON EMPTY'))"), "NULL");
    assert_eq!(one(&db, "ST_CENTROID(ST_GEOMFROMTEXT('MULTIPOLYGON EMPTY'))"), "NULL");
    assert_eq!(one(&db, "ST_DISTANCE(ST_POINT(0, 0), ST_POINT(3000, 4000), 'KILOMETRE')"), "5E0");
    let a = db.session("A");
    assert_eq!(state(&a, "VALUES ST_AREA(ST_POINT(0, 0))"), "2FF02");
    assert_eq!(state(&a, "VALUES ST_LENGTH(ST_POINT(0, 0, 4326), 'METRE')"), "2FF02");
    assert_eq!(state(&a, "VALUES ST_DISTANCE(ST_POINT(0, 0, 4326), ST_POINT(1, 1, 4326), 'METRE')"), "2FF14");
    assert_eq!(state(&a, "VALUES ST_DISTANCE(ST_POINT(0, 0), ST_POINT(1, 1), 'FURLONG')"), "2FF14");
}

/// Verifies: [ISO/IEC 13249-3:2016 §8.3.9, §8.3.10, §10.6.7, §10.6.8]
/// the polygon and multipolygon the rings of a multilinestring bound, of
/// text and of binary, with their SRID; a line that is not a ring is an
/// invalid argument (2FF02), and text of another type invalid (2FF22).
#[test]
fn polygons_from_rings() {
    let db = database();
    let rings = "'MULTILINESTRING ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 1, 2 2, 1 2, 1 1))'";
    assert_eq!(one(&db, &format!("ST_AREA(ST_BDPOLYFROMTEXT({rings}))")), "1.5E1");
    assert_eq!(one(&db, &format!("ST_GEOMETRYTYPE(ST_BDPOLYFROMTEXT({rings}, 7))")), "ST_Polygon");
    assert_eq!(one(&db, &format!("ST_SRID(ST_BDPOLYFROMTEXT({rings}, 7))")), "7");
    let apart = "'MULTILINESTRING ((0 0, 1 0, 1 1, 0 0), (5 5, 6 5, 6 6, 5 5))'";
    assert_eq!(one(&db, &format!("ST_GEOMETRYTYPE(ST_BDMPOLYFROMTEXT({apart}))")), "ST_MultiPolygon");
    assert_eq!(one(&db, &format!("ST_AREA(ST_BDMPOLYFROMWKB(ST_ASBINARY(ST_GEOMFROMTEXT({apart}))))")), "1E0");
    assert_eq!(one(&db, &format!("ST_AREA(ST_BDPOLYFROMWKB(ST_ASBINARY(ST_GEOMFROMTEXT({rings})), 3))")), "1.5E1");
    let a = db.session("A");
    assert_eq!(state(&a, "VALUES ST_BDPOLYFROMTEXT('MULTILINESTRING ((0 0, 1 0, 1 1))')"), "2FF02");
    assert_eq!(state(&a, "VALUES ST_BDPOLYFROMTEXT('POINT (1 1)')"), "2FF22");
}

/// Verifies: [ISO/IEC 13249-3:2016 §4.2.1.6] a circular string relates
/// and measures through its linear form (`CURVE_LINEARIZER`).
#[test]
fn curves_through_their_linear_form() {
    let db = database();
    assert_eq!(one(&db, &format!("ST_INTERSECTS({ARC}, ST_GEOMFROMTEXT('LINESTRING (1 0, 1 2)'))")), "1");
    assert_eq!(one(&db, &format!("ST_DISJOINT({ARC}, ST_POINT(1, 0.5))")), "1");
    let length: f64 = one(&db, &format!("ST_LENGTH({ARC})")).parse().unwrap();
    assert!((length - std::f64::consts::PI).abs() < 0.01, "{length}");
    assert_eq!(one(&db, &format!("ST_ISSIMPLE({ARC})")), "1");
}

/// Verifies: [EXTENSION] D85: ST_Contains, ST_Within, ST_Touches,
/// ST_Crosses, ST_Overlaps and ST_Equals compared with 1 use a geometry
/// column's R-tree and return the rows a scan returns. ST_Equals of an
/// empty operand, true of every empty row, is planned as a scan.
#[test]
fn relationships_use_rtree_plans() {
    let db = database();
    let a = db.session("A");
    a.run("CREATE TABLE U (ID INTEGER, G GEOMETRY(0))").unwrap();
    a.commit().unwrap();
    for start in (0..2_000_i32).step_by(200) {
        let values: Vec<String> = (start..start + 200)
            .map(|id| {
                let (x, y) = (id % 50, id.div_euclid(50));
                format!("({id}, ST_GEOMFROMTEXT('POLYGON (({x} {y}, {x2} {y}, {x2} {y2}, {x} {y2}, {x} {y}))'))", x2 = x + 1, y2 = y + 1)
            })
            .collect();
        a.run(&format!("INSERT INTO U VALUES {}", values.join(", "))).unwrap();
    }
    a.run("INSERT INTO U VALUES (2000, ST_GEOMFROMTEXT('POINT EMPTY')), (2001, ST_POINT(2, 2))").unwrap();
    a.commit().unwrap();
    let probe = "ST_GEOMFROMTEXT('POLYGON ((1 1, 4 1, 4 3, 1 3, 1 1))')";
    let queries = [
        format!("SELECT ID FROM U WHERE ST_WITHIN(G, {probe}) = 1 ORDER BY ID"),
        format!("SELECT ID FROM U WHERE ST_CONTAINS({probe}, G) = 1 ORDER BY ID"),
        format!("SELECT ID FROM U WHERE ST_TOUCHES(G, {probe}) = 1 ORDER BY ID"),
        "SELECT ID FROM U WHERE ST_OVERLAPS(G, ST_GEOMFROMTEXT('POLYGON ((0.5 0.5, 1.5 0.5, 1.5 1.5, 0.5 1.5, 0.5 0.5))')) = 1 ORDER BY ID"
            .to_owned(),
        "SELECT ID FROM U WHERE ST_CROSSES(ST_GEOMFROMTEXT('LINESTRING (0.5 0.5, 2.5 0.5)'), G) = 1 ORDER BY ID".to_owned(),
        "SELECT ID FROM U WHERE ST_EQUALS(G, ST_GEOMFROMTEXT('POLYGON ((2 2, 3 2, 3 3, 2 3, 2 2))')) = 1 ORDER BY ID".to_owned(),
        "SELECT ID FROM U WHERE ST_EQUALS(G, ST_GEOMFROMTEXT('LINESTRING EMPTY')) = 1 ORDER BY ID".to_owned(),
    ];
    let scanned: Vec<Vec<String>> = queries.iter().map(|sql| rows(&a, sql).unwrap()).collect();
    assert_eq!(scanned[0], ["51", "52", "53", "101", "102", "103", "2001"]);
    assert_eq!(scanned[5], ["102"]);
    assert_eq!(scanned[6], ["2000"]);
    a.commit().unwrap();
    db.create_index("U_G", "A.U", &["G"], "rtree").unwrap();
    let a = db.session("A");
    for (at, (sql, expected)) in queries.iter().zip(&scanned).enumerate() {
        // ST_Equals of an empty value finds every row, which a scan reads.
        let plan = a.physical_plan(sql, &Params::new()).unwrap();
        assert_eq!(plan.contains("U_G"), at < 6, "{sql}: {plan}");
        assert_eq!(&rows(&a, sql).unwrap(), expected, "{sql}");
    }
    // An empty value probes every row for ST_Equals, so it finds itself.
    let joined = "SELECT COUNT(*) FROM U L, U R WHERE ST_EQUALS(L.G, R.G) = 1";
    assert_eq!(rows(&a, joined).unwrap(), ["2002"]);
}

/// Verifies: [DESIGN] D85: a constant polygon prepared for ST_Intersects
/// relates as the engine does; a bowtie, which is not valid, is left to
/// the engine, by whose depth one lobe is inside and the other is not.
#[test]
fn prepared_polygons_relate_as_the_engine_does() {
    let db = database();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, ST_POINT(0.5, 1)), (2, ST_POINT(1.5, 1))").unwrap();
    let bowtie = "ST_GEOMFROMTEXT('POLYGON ((0 0, 2 2, 2 0, 0 2, 0 0))')";
    let prepared = rows(&a, &format!("SELECT ID FROM T WHERE ST_INTERSECTS(G, {bowtie}) = 1")).unwrap();
    assert_eq!(prepared.len(), 1, "{prepared:?}");
    let each: Vec<String> = ["0.5", "1.5"].iter().map(|x| one(&db, &format!("ST_INTERSECTS(ST_POINT({x}, 1), {bowtie})"))).collect();
    assert_eq!(each.iter().filter(|value| *value == "1").count(), 1, "{each:?}");
}
