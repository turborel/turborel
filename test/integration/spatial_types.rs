//! SQL/MM Spatial's type family [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §4.2]: its 27 types as SQL type names, subtyping, Table 4 casts, TREAT
//! and IS OF, the ordering EQUALS ONLY BY RELATIVE, values of every type
//! through well-known binary, collections of geometry values, and their
//! life across reopening and recovery (RULES.md D-SPATIAL-MODEL,
//! D-SPATIAL-FAMILY).

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{Scratch, after_crash, exec, rows, state};

fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2] [ISO/IEC
/// 9075-2:2023 §9.2 SR2] every geometry type is a column's type; a value
/// of a subtype is stored to a column of a supertype, a value of a
/// supertype to one of a subtype only through TREAT, which fails at run
/// time of a value of another type (SQLSTATE 0D000).
#[test]
fn geometry_types_are_column_types_with_subtyping() {
    let db = database();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, P ST_POINT, C ST_CURVE, S ST_SURFACE(4326), G ST_GEOMETRY, O GEOMETRY)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T (ID, P, G, O) VALUES (1, ST_POINT(1, 2), ST_POINT(3, 4), ST_POINT(5, 6))").unwrap();
    assert_eq!(exec(&a, "INSERT INTO T (ID, P) VALUES (2, ST_GEOMFROMTEXT('POINT (1 2)'))").err(), Some(Code::SyntaxRule));
    a.run("INSERT INTO T (ID, P) VALUES (2, TREAT(ST_GEOMFROMTEXT('POINT (7 8)') AS ST_POINT))").unwrap();
    assert_eq!(state(&a, "VALUES ST_X(TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 1 1)') AS ST_POINT))"), "0D000");
    let insert = a.prepare("INSERT INTO T (ID, P) VALUES (3, ?)").unwrap();
    let line = turborel::geometry::Geometry::from_wkt("LINESTRING (0 0, 1 1)", 0).unwrap().to_value();
    let e = a.execute_prepared(&insert, &[line]).unwrap_err();
    assert_eq!((e.code, e.sqlstate().as_str()), (Code::Data, "2FF08"));
    a.run("INSERT INTO T (ID, C) VALUES (4, TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 1 1)') AS ST_LINESTRING))").unwrap();
    a.run("INSERT INTO T (ID, S) VALUES (5, ST_MAKEENVELOPE(0, 0, 1, 1, 4326))").unwrap();
    assert_eq!(exec(&a, "INSERT INTO T (ID, S) VALUES (6, ST_MAKEENVELOPE(0, 0, 1, 1, 0))").err(), Some(Code::Data));
    assert_eq!(
        rows(&a, "SELECT ID, ST_GEOMETRYTYPE(P), ST_GEOMETRYTYPE(C), ST_GEOMETRYTYPE(S) FROM T ORDER BY ID").unwrap(),
        ["1,ST_Point,NULL,NULL", "2,ST_Point,NULL,NULL", "4,NULL,ST_LineString,NULL", "5,NULL,NULL,ST_Polygon"]
    );
    assert_eq!(rows(&a, "SELECT ID FROM T WHERE G IS OF (ST_POINT) OR C IS OF (ST_CURVE) ORDER BY ID").unwrap(), ["1", "4"]);
    assert_eq!(rows(&a, "SELECT ID FROM T WHERE C IS OF (ONLY ST_CURVE)").unwrap(), Vec::<String>::new());
    assert_eq!(rows(&a, "SELECT ID FROM T WHERE P IS OF (ST_CURVE)").unwrap_err(), Code::SyntaxRule);
    assert_eq!(rows(&a, "SELECT TREAT(P AS ST_CURVE) FROM T").unwrap_err(), Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.5 Table 4,
/// §6.20 SR2] CAST within the hierarchy, as Table 4 takes it, refused with
/// SQLSTATE 2FF08 where it does not; NEW of an instantiable type alone.
#[test]
fn casts_and_new_follow_the_hierarchy() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE ONE (X INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO ONE VALUES (1)").unwrap();
    assert_eq!(rows(&a, "VALUES ST_GEOMETRYTYPE(CAST(ST_POINT(1, 2) AS ST_MULTIPOINT))").unwrap(), ["ST_MultiPoint"]);
    assert_eq!(rows(&a, "VALUES ST_GEOMETRYTYPE(CAST(CAST(ST_POINT(1, 2) AS ST_GEOMCOLLECTION) AS ST_POINT))").unwrap(), ["ST_Point"]);
    assert_eq!(rows(&a, "VALUES ST_GEOMETRYTYPE(CAST(ST_POINT(1, 2) AS ST_GEOMETRY))").unwrap(), ["ST_Point"]);
    assert_eq!(rows(&a, "VALUES ST_GEOMETRYTYPE(CAST('POINT EMPTY' AS ST_TIN))").unwrap(), ["ST_TIN"]);
    let triangle = "VALUES ST_GEOMETRYTYPE(CAST(ST_GEOMFROMTEXT('POLYGON ((0 0, 0 1, 1 0, 0 0))') AS ST_TRIANGLE))";
    assert_eq!(rows(&a, triangle).unwrap(), ["ST_Triangle"]);
    assert_eq!(state(&a, "VALUES CAST(ST_POINT(1, 2) AS ST_LINESTRING)"), "2FF08");
    assert_eq!(rows(&a, "VALUES ST_X(NEW ST_POINT(3, 4))").unwrap(), ["3E0"]);
    assert_eq!(rows(&a, "VALUES ST_POINT(3, 4).ST_Y()").unwrap(), ["4E0"]);
    assert_eq!(rows(&a, "VALUES CAST(ST_POINT(3, 4) AS ST_GEOMETRY).ST_GEOMETRYTYPE()").unwrap(), ["ST_Point"]);
    assert_eq!(rows(&a, "VALUES NEW ST_GEOMETRY()").unwrap_err(), Code::SyntaxRule);
    assert_eq!(rows(&a, "VALUES NEW ST_CURVE()").unwrap_err(), Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.65] [ISO/IEC
/// 9075-2:2023 §9.10 to §9.14] geometry values compare for equality by
/// ST_OrderingEquals and are neither ordered, grouped, made distinct nor
/// a constraint's key (SQLCODE -103).
#[test]
fn geometry_values_are_equals_only_by_relative() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, G ST_GEOMETRY)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, ST_POINT(1, 2)), (2, ST_GEOMFROMTEXT('POINT (-0 2)')), (3, ST_POINT(0, 2)), (4, NULL)").unwrap();
    assert_eq!(rows(&a, "SELECT ID FROM T WHERE G = ST_POINT(0, 2) ORDER BY ID").unwrap(), ["2", "3"]);
    assert_eq!(rows(&a, "SELECT ID FROM T WHERE G <> ST_POINT(0, 2) ORDER BY ID").unwrap(), ["1"]);
    assert_eq!(
        rows(&a, "VALUES ST_ORDERINGEQUALS(ST_POINT(1, 1), ST_POINT(1, 1)), ST_ORDERINGEQUALS(ST_POINT(1, 1), ST_POINT(1, 2))").unwrap(),
        ["0", "1"]
    );
    for refused in ["SELECT G FROM T ORDER BY G", "SELECT G FROM T GROUP BY G", "SELECT DISTINCT G FROM T", "SELECT ID FROM T WHERE G < G"]
    {
        assert_eq!(rows(&a, refused).unwrap_err(), Code::SyntaxRule, "{refused}");
    }
    assert_eq!(exec(&a, "CREATE TABLE K (G ST_POINT PRIMARY KEY)").err(), Some(Code::SyntaxRule));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68] a value of
/// any type and coordinate dimension through its well-known binary
/// representation, its type kept.
#[test]
fn values_of_every_dimension_through_wkb() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, G ST_GEOMETRY)").unwrap();
    let a = db.session("A");
    // POINT Z (1 2 3), big-endian, and a circular string, little-endian.
    a.run("INSERT INTO T VALUES (1, ST_GEOMFROMWKB(X'00000003E93FF000000000000040000000000000004008000000000000', 4326))").unwrap();
    let arc = "01080000000300000000000000000000000000000000000000000000000000F03F000000000000F03F00000000000000400000000000000000";
    a.run(&format!("INSERT INTO T VALUES (2, ST_GEOMFROMWKB(X'{arc}'))")).unwrap();
    assert_eq!(
        rows(&a, "SELECT ID, ST_GEOMETRYTYPE(G), ST_SRID(G) FROM T ORDER BY ID").unwrap(),
        ["1,ST_Point,4326", "2,ST_CircularString,0"]
    );
    assert_eq!(rows(&a, "SELECT ST_X(G), ST_MAXY(G) FROM T WHERE ID = 1").unwrap(), ["1E0,2E0"]);
    // An arc's envelope comes from its linear form.
    assert_eq!(rows(&a, "SELECT ST_MINX(G), ST_MAXX(G) FROM T WHERE ID = 2").unwrap(), ["0E0,2E0"]);
    assert_eq!(
        rows(&a, "SELECT ST_ASBINARY(G) FROM T WHERE ID = 1").unwrap(),
        ["01E9030000000000000000F03F00000000000000400000000000000840"]
    );
    assert_eq!(rows(&a, "SELECT ST_ASTEXT(G) FROM T WHERE ID = 1").unwrap_err(), Code::Unsupported);
    assert_eq!(state(&a, "VALUES ST_GEOMFROMWKB(X'01FF000000')"), "2FF23");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2] [ISO/IEC
/// 9075-2:2023 §6.1 SR43, SR48] arrays and multisets of geometry values,
/// stored and read back.
#[test]
fn collections_of_geometry_values() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, PTS ST_POINT ARRAY[4], ALL_OF ST_GEOMETRY MULTISET)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, ARRAY[ST_POINT(1, 2), ST_POINT(3, 4)], MULTISET[ST_POINT(5, 6)])").unwrap();
    assert_eq!(rows(&a, "SELECT CARDINALITY(PTS), ST_X(PTS[2]), CARDINALITY(ALL_OF) FROM T").unwrap(), ["2,3E0,1"]);
    assert_eq!(exec(&a, "INSERT INTO T (ID, PTS) VALUES (2, ARRAY[ST_GEOMFROMTEXT('POINT (1 1)')])").err(), Some(Code::SyntaxRule));
}

/// Verifies: [DESIGN] RULES.md D25, D-SPATIAL-MODEL: values of the new
/// types and the types of their columns survive a checkpoint, a reopen and
/// a crash, an uncommitted change lost.
#[test]
fn geometry_types_survive_reopen_and_recovery() {
    let scratch = Scratch::new("spatial-types-recovery");
    let dir = scratch.path("db");
    let open = || {
        let db = database();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, P ST_POINT(4326), C ST_CURVE)").unwrap();
    let a = db.session("A");
    let arc = "01080000000300000000000000000000000000000000000000000000000000F03F000000000000F03F00000000000000400000000000000000";
    a.run(&format!("INSERT INTO T VALUES (1, ST_POINT(1, 2, 4326), TREAT(ST_GEOMFROMWKB(X'{arc}') AS ST_CURVE))")).unwrap();
    a.commit().unwrap();
    db.checkpoint().unwrap();
    a.run("INSERT INTO T (ID) VALUES (2)").unwrap();
    a.commit().unwrap();
    a.run("UPDATE T SET P = ST_POINT(9, 9, 4326) WHERE ID = 1").unwrap();
    let crashed = a;
    drop(db);
    let db = open();
    let a = db.session("A");
    assert_eq!(rows(&a, "SELECT ID, ST_X(P), ST_GEOMETRYTYPE(C) FROM T ORDER BY ID").unwrap(), ["1,1E0,ST_CircularString", "2,NULL,NULL"]);
    assert_eq!(exec(&a, "INSERT INTO T (ID, P) VALUES (3, ST_POINT(1, 2, 0))").err(), Some(Code::Data));
    assert_eq!(
        exec(&a, "INSERT INTO T (ID, P) VALUES (3, TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 1 1)', 4326) AS ST_GEOMETRY))").err(),
        Some(Code::SyntaxRule)
    );
    a.rollback().unwrap();
    db.close().unwrap();
    drop((a, db, crashed));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.62, §5.1.63]
/// the constructors take a CHARACTER LARGE OBJECT's well-known text and a
/// BINARY LARGE OBJECT's well-known binary representation.
#[test]
fn constructors_take_large_objects() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (WKT CLOB(1M), WKB BLOB(1M))").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES ('LINESTRING (0 0, 3 4)', X'0101000000000000000000F03F0000000000000040')").unwrap();
    assert_eq!(
        rows(&a, "SELECT ST_GEOMETRYTYPE(ST_GEOMFROMTEXT(WKT, 4326)), ST_ASTEXT(ST_GEOMFROMWKB(WKB)) FROM T").unwrap(),
        ["ST_LineString,POINT (1 2)"]
    );
    assert_eq!(rows(&a, "SELECT ST_GEOMFROMTEXT(WKB) FROM T").unwrap_err(), Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §19, §20] the
/// SQL/MM Spatial Information Schema: the geometry columns a user sees,
/// with the system each type names; the systems, units and sizings
/// supplied; the short name views; and a Definition Schema no statement
/// reads.
#[test]
fn the_spatial_information_schema() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, P ST_POINT(4326), G ST_GEOMETRY, U ST_POINT(7))").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE S (G ST_POLYGON(32633))").unwrap();
    let a = db.session("A");
    let columns =
        "SELECT TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, SRS_NAME, SRS_ID FROM ST_INFORMTN_SCHEMA.ST_GEOMETRY_COLUMNS ORDER BY COLUMN_NAME";
    assert_eq!(rows(&a, columns).unwrap(), ["A,T,G,NULL,NULL", "A,T,P,WGS 84,4326", "A,T,U,NULL,NULL"]);
    let short = "SELECT F_TABLE_NAME, F_GEOMETRY_COLUMN, SRID FROM ST_INFORMTN_SCHEMA.GEOMETRY_COLUMNS WHERE SRID IS NOT NULL";
    assert_eq!(rows(&a, short).unwrap(), ["T,P,4326"]);
    let b = db.session("B");
    assert_eq!(rows(&b, "SELECT SRS_NAME FROM ST_INFORMTN_SCHEMA.ST_GEOMETRY_COLUMNS").unwrap(), ["WGS 84 / UTM zone 33N"]);
    let systems = "SELECT SRS_ID, ORGANIZATION, ORGANIZATION_COORDSYS_ID FROM ST_INFORMTN_SCHEMA.ST_SPATIAL_REFERENCE_SYSTEMS WHERE SRS_NAME = 'WGS 84'";
    assert_eq!(rows(&a, systems).unwrap(), ["4326,EPSG,4326"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM ST_INFORMTN_SCHEMA.SPATIAL_REF_SYS WHERE SRTEXT LIKE 'PROJCS%'").unwrap(), ["121"]);
    assert_eq!(
        rows(&a, "SELECT UNIT_TYPE, CONVERSION_FACTOR FROM ST_INFORMTN_SCHEMA.ST_UNITS WHERE UNIT_NAME = 'FOOT'").unwrap(),
        ["LINEAR,3.048E-1"]
    );
    let units = "SELECT UNIT_NAME FROM ST_INFORMTN_SCHEMA.ST_UNITS_OF_MEASURE WHERE CONVERSION_FACTOR = 1 ORDER BY UNIT_NAME";
    assert_eq!(rows(&a, units).unwrap(), ["METRE", "RADIAN"]);
    let sizings = "SELECT SUPPORTED_VALUE FROM ST_INFORMTN_SCHEMA.ST_SIZINGS WHERE VARIABLE_NAME IN ('ST_MaxTypeNameLength', 'ST_MaxGeometryAsGML') ORDER BY VARIABLE_NAME";
    assert_eq!(rows(&a, sizings).unwrap(), ["NULL", "32"]);
    assert_eq!(rows(&a, "SELECT * FROM ST_DEFINITION_SCHEMA.ST_SIZINGS").unwrap_err(), Code::SyntaxRule);
    let listed = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'ST_INFORMTN_SCHEMA' ORDER BY TABLE_NAME";
    assert_eq!(rows(&a, listed).unwrap().len(), 9);
}
