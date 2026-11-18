//! SQL/MM Spatial's types beside the geometry types [ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §14, §16]: ST_SpatialRefSys, ST_Angle and
//! ST_Direction as SQL types, each routine the standard gives them
//! through SQL, their orderings, their transform groups as CASTs, and
//! ST_DEFINESRS's check of its definition (RULES.md D-SPATIAL-SUPPORT).

use turborel::db::{Database, Session};
use turborel::value::Value;

use crate::common::{rows, state};

fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db
}

/// The rows of `sql`, each its columns joined by commas, joined by `;`.
fn one(a: &Session, sql: &str) -> String {
    rows(a, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}")).join(";")
}

/// The WKT of WGS 84, as the systems supplied hold it.
const WGS84: &str = "GEOGCS[\"WGS 84\",DATUM[\"WGS_1984\",SPHEROID[\"WGS 84\",6378137,298.257223563]],PRIMEM[\"Greenwich\",0],UNIT[\"degree\",0.0174532925199433]]";

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §16.1.2 to §16.1.18]
/// ST_Angle's constructors of radians, of a unit and a value, of degrees,
/// minutes and seconds, of three points and of two lines; its observers
/// and mutators of radians, degrees and gradians and of each degree
/// component; ST_String; the arithmetic methods; and its text and GML
/// functions and methods.
#[test]
fn angle_routines_16_1() {
    let db = database();
    let a = db.session("A");
    assert_eq!(one(&a, "VALUES ST_ANGLE(1e0)"), "ANGLE RADIANS(1)");
    assert_eq!(one(&a, "VALUES NEW ST_ANGLE('D', 90e0).ST_RADIANS() BETWEEN 1.5707 AND 1.5708"), "TRUE");
    assert_eq!(one(&a, "VALUES NEW ST_ANGLE('G', 100e0).ST_DEGREES() BETWEEN 89.999 AND 90.001"), "TRUE");
    assert_eq!(one(&a, "VALUES NEW ST_ANGLE(10, 30, 36e0).ST_DEGREES() BETWEEN 10.5099 AND 10.5101"), "TRUE");
    assert_eq!(
        one(&a, "VALUES NEW ST_ANGLE(ST_POINT(0, 0), ST_POINT(1, 0), ST_POINT(0, 1)).ST_DEGREES() BETWEEN 89.999 AND 90.001"),
        "TRUE"
    );
    let lines = "NEW ST_ANGLE(TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 1 0)') AS ST_LINESTRING), TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 1 1)') AS ST_LINESTRING))";
    assert_eq!(one(&a, &format!("VALUES {lines}.ST_DEGREES() BETWEEN 44.999 AND 45.001")), "TRUE");
    assert_eq!(one(&a, "VALUES ST_ANGLE(1e0).ST_DEGREES(180e0).ST_GRADIANS()"), "2E2");
    assert_eq!(one(&a, "VALUES ST_ANGLE(1e0).ST_GRADIANS(100e0).ST_RADIANS() BETWEEN 1.5707 AND 1.5708"), "TRUE");
    let dms = "NEW ST_ANGLE('D', 10.51e0)";
    assert_eq!(one(&a, &format!("VALUES ({dms}.ST_DEGREECOMPONENT(), {dms}.ST_MINUTECOMPONENT())")), "10,30");
    assert_eq!(one(&a, "VALUES ST_ANGLE(1e0).ST_STRING()"), "57 17 44.81");
    assert_eq!(one(&a, "VALUES ST_ANGLE(1e0).ST_STRING(0)"), "57 17 45");
    assert_eq!(one(&a, "VALUES ST_ANGLE(0e0).ST_STRING('57 17 45').ST_DEGREES() BETWEEN 57.29 AND 57.30"), "TRUE");
    assert_eq!(one(&a, "VALUES ST_ANGLE(1e0).ST_ADD(ST_ANGLE(2e0)).ST_RADIANS()"), "3E0");
    assert_eq!(one(&a, "VALUES ST_ANGLE(3e0).ST_SUBTRACT(ST_ANGLE(1e0)).ST_RADIANS()"), "2E0");
    assert_eq!(one(&a, "VALUES (ST_ANGLE(3e0).ST_MULTIPLY(2e0).ST_RADIANS(), ST_ANGLE(3e0).ST_DIVIDE(2e0).ST_RADIANS())"), "6E0,1.5E0");
    assert_eq!(one(&a, "VALUES ST_ANGLEFROMTEXT('ANGLE DEGREES(45)').ST_DEGREES()"), "4.5E1");
    assert_eq!(one(&a, "VALUES NEW ST_ANGLE('ANGLE GRADIANS(50)').ST_GRADIANS() BETWEEN 49.999 AND 50.001"), "TRUE");
    let gml = one(&a, "VALUES ST_ANGLE(1e0).ST_ASGML()");
    assert!(gml.contains(">1<"), "{gml}");
    assert_eq!(one(&a, &format!("VALUES ST_ANGLEFROMGML('{gml}').ST_RADIANS()")), "1E0");
    assert_eq!(one(&a, &format!("VALUES ST_ANGLE(0e0).ST_GMLTOSQL('{gml}').ST_RADIANS()")), "1E0");
    assert_eq!(state(&a, "VALUES ST_ANGLEFROMTEXT('ANGLE FURLONGS(1)')"), "2FF22");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §16.1, §16.2;
/// ISO/IEC 9075-2:2023 §4.13, §8.2] ST_Angle and ST_Direction are
/// ORDER FULL BY RELATIVE WITH ST_OrderingCompare: they compare, sort,
/// group, take MAX and MIN and key an index by radians, and
/// ST_OrderingCompare gives -1, 0 or 1.
#[test]
fn angles_and_directions_order_full() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, A ST_ANGLE, D ST_DIRECTION)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, ST_ANGLE(3e0), ST_DIRECTION(1e0)), (2, ST_ANGLE(1e0), ST_DIRECTION(2e0)), (3, ST_ANGLE(2e0), ST_DIRECTION(0.5e0)), (4, ST_ANGLE(1e0), ST_DIRECTION(2e0))").unwrap();
    assert_eq!(
        one(&a, "VALUES (ST_ANGLE(1e0) < ST_ANGLE(2e0), ST_ANGLE(1e0) = ST_ANGLE(1e0), ST_DIRECTION(1e0) >= ST_DIRECTION(2e0))"),
        "TRUE,TRUE,FALSE"
    );
    assert_eq!(one(&a, "SELECT ID FROM T ORDER BY A, ID"), "2;4;3;1");
    assert_eq!(one(&a, "SELECT ID FROM T ORDER BY D DESC, ID"), "2;4;1;3");
    assert_eq!(one(&a, "SELECT MAX(A), MIN(D) FROM T"), "ANGLE RADIANS(3),DIRECTION(N 0.5)");
    assert_eq!(one(&a, "SELECT A, COUNT(*) FROM T GROUP BY A ORDER BY A"), "ANGLE RADIANS(1),2;ANGLE RADIANS(2),1;ANGLE RADIANS(3),1");
    assert_eq!(one(&a, "SELECT COUNT(DISTINCT D) FROM T"), "3");
    a.commit().unwrap();
    a.run("CREATE INDEX TA ON T (A)").unwrap();
    assert_eq!(one(&a, "SELECT ID FROM T WHERE A > ST_ANGLE(1.5e0) ORDER BY A"), "3;1");
    assert_eq!(
        one(&a, "VALUES (ST_ORDERINGCOMPARE(ST_ANGLE(1e0), ST_ANGLE(2e0)), ST_ORDERINGCOMPARE(ST_ANGLE(1e0), ST_ANGLE(1e0)))"),
        "-1,0"
    );
    assert_eq!(one(&a, "VALUES ST_ORDERINGCOMPARE(ST_DIRECTION(1e0), ST_DIRECTION(0e0))"), "1");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §16.2.2 to §16.2.20]
/// ST_Direction's constructors of radians, of a bearing, of an azimuth,
/// of two points and of a line; ST_Radians, ST_AngleNAzimuth, the nine
/// bearing and azimuth strings, ST_AddAngle and ST_SubtractAngle; and
/// its text and GML functions and methods.
#[test]
fn direction_routines_16_2() {
    let db = database();
    let a = db.session("A");
    assert_eq!(one(&a, "VALUES ST_DIRECTION(1e0).ST_ASTEXT()"), "DIRECTION(N 1)");
    let bearing = "NEW ST_DIRECTION('N', NEW ST_ANGLE('D', 30e0), 'E')";
    assert_eq!(one(&a, &format!("VALUES {bearing}.ST_DEGREESBEARING(2)")), "N 30.00 E");
    assert_eq!(one(&a, &format!("VALUES {bearing}.ST_DEGREESNAZIMUTH(1)")), "N 30.0");
    assert_eq!(one(&a, &format!("VALUES {bearing}.ST_DEGREESSAZIMUTH(1)")), "S 210.0");
    assert_eq!(one(&a, &format!("VALUES {bearing}.ST_DMSBEARING(0)")), "N 30 0 0 E");
    assert_eq!(one(&a, "VALUES NEW ST_DIRECTION('S', NEW ST_ANGLE('D', 90e0)).ST_DEGREESNAZIMUTH(0)"), "N 270");
    assert_eq!(one(&a, "VALUES ST_DIRECTION(ST_POINT(0, 0), ST_POINT(1, 1)).ST_DEGREESNAZIMUTH(1)"), "N 45.0");
    assert_eq!(one(&a, "VALUES ST_DIRECTION(TREAT(ST_GEOMFROMTEXT('LINESTRING (0 0, 0 1)') AS ST_LINESTRING)).ST_RADIANS()"), "0E0");
    assert_eq!(one(&a, "VALUES ST_DIRECTION(1e0).ST_ANGLENAZIMUTH().ST_RADIANS()"), "1E0");
    assert_eq!(one(&a, "VALUES ST_DIRECTION(1e0).ST_ANGLENAZIMUTH(ST_ANGLE(2e0)).ST_RADIANS()"), "2E0");
    assert_eq!(one(&a, "VALUES ST_DIRECTION(1e0).ST_ADDANGLE(ST_ANGLE(1e0)).ST_SUBTRACTANGLE(ST_ANGLE(0.5e0)).ST_RADIANS()"), "1.5E0");
    assert_eq!(one(&a, "VALUES ST_DIRECTIONFRMTXT('DIRECTION(N 2)').ST_RADIANS()"), "2E0");
    let gml = one(&a, "VALUES ST_DIRECTION(1e0).ST_ASGML()");
    assert!(gml.contains("DirectionVector"), "{gml}");
    assert_eq!(one(&a, &format!("VALUES ST_DIRECTIONFRMGML('{gml}').ST_RADIANS()")), "1E0");
    assert_eq!(one(&a, &format!("VALUES ST_DIRECTION(0e0).ST_GMLTOSQL('{gml}').ST_RADIANS()")), "1E0");
    assert_eq!(state(&a, "VALUES ST_DIRECTION(-1e0)"), "2FF02");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §16.1.20, §16.2.22;
/// ISO/IEC 9075-2:2023 §6.13] the transform groups as CASTs: an angle or
/// a direction to and from its well-known text, and to and from a number
/// as its radians.
#[test]
fn angle_and_direction_casts() {
    let db = database();
    let a = db.session("A");
    assert_eq!(one(&a, "VALUES CAST(ST_ANGLE(1e0) AS VARCHAR(64))"), "ANGLE RADIANS(1)");
    assert_eq!(one(&a, "VALUES CAST(ST_ANGLE(1e0) AS DOUBLE PRECISION)"), "1E0");
    assert_eq!(one(&a, "VALUES CAST('ANGLE RADIANS(2)' AS ST_ANGLE).ST_RADIANS()"), "2E0");
    assert_eq!(one(&a, "VALUES CAST(0.5e0 AS ST_ANGLE)"), "ANGLE RADIANS(0.5)");
    assert_eq!(one(&a, "VALUES CAST(ST_DIRECTION(2e0) AS DOUBLE PRECISION)"), "2E0");
    assert_eq!(one(&a, "VALUES CAST('DIRECTION(N 1)' AS ST_DIRECTION)"), "DIRECTION(N 1)");
    assert_eq!(one(&a, "VALUES CAST(1 AS ST_DIRECTION)"), "DIRECTION(N 1)");
    assert_eq!(state(&a, "VALUES CAST(10e0 AS ST_DIRECTION)"), "2FF02");
    assert_eq!(state(&a, "VALUES CAST('ANGLE' AS ST_ANGLE)"), "2FF22");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §14.1.2 to §14.1.8]
/// ST_SpatialRefSys's constructors of a definition and of an SRID,
/// ST_WKTSRSToSQL, ST_SRID, ST_AsWKTSRS and ST_Equals; a definition a
/// catalog defines takes its SRID; it CASTs to and from its well-known
/// text; a definition that is no `<spatial reference system>` is 2FF22.
#[test]
fn spatial_ref_sys_routines_14_1() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    assert_eq!(one(&a, "VALUES ST_SPATIALREFSYS(4326).ST_SRID()"), "4326");
    assert_eq!(one(&a, "VALUES ST_SPATIALREFSYS(4326).ST_ASWKTSRS()"), WGS84);
    assert_eq!(one(&a, &format!("VALUES ST_SPATIALREFSYS(CAST('{WGS84}' AS CLOB(8K))).ST_SRID()")), "4326");
    let grid = "PROJCS[\"grid\",GEOGCS[\"WGS 84\",DATUM[\"WGS_1984\",SPHEROID[\"WGS 84\",6378137,298.257223563]],PRIMEM[\"Greenwich\",0],UNIT[\"degree\",0.0174532925199433]],PROJECTION[\"Mercator_1SP\"],UNIT[\"metre\",1]]";
    assert_eq!(one(&a, &format!("VALUES ST_SPATIALREFSYS(CAST('{grid}' AS CLOB(8K))).ST_SRID()")), "NULL");
    a.run(&format!("CALL ST_DEFINESRS('GRID', 900001, 'APP', 1, '{grid}', 'a grid')")).unwrap();
    assert_eq!(one(&a, &format!("VALUES ST_SPATIALREFSYS(CAST('{grid}' AS CLOB(8K))).ST_SRID()")), "900001");
    assert_eq!(one(&a, "VALUES ST_SPATIALREFSYS(900001).ST_ASWKTSRS() IS NOT NULL"), "TRUE");
    assert_eq!(
        one(
            &a,
            "VALUES ST_SPATIALREFSYS(4326).ST_WKTSRSTOSQL(CAST('GEOGCS[\"x\",DATUM[\"d\",SPHEROID[\"s\",1,0]],PRIMEM[\"p\",0],UNIT[\"degree\",0.0174532925199433]]' AS CLOB(8K))).ST_SRID()"
        ),
        "NULL"
    );
    assert_eq!(
        one(
            &a,
            "VALUES (ST_SPATIALREFSYS(4326).ST_EQUALS(ST_SPATIALREFSYS(4326)), ST_SPATIALREFSYS(4326).ST_EQUALS(ST_SPATIALREFSYS(4269)))"
        ),
        "1,0"
    );
    assert_eq!(one(&a, "VALUES CAST(ST_SPATIALREFSYS(4326) AS VARCHAR(2048))"), WGS84);
    assert_eq!(one(&a, &format!("VALUES CAST('{WGS84}' AS ST_SPATIALREFSYS).ST_SRID()")), "4326");
    assert_eq!(state(&a, "VALUES ST_SPATIALREFSYS(CAST('GEOGCS[' AS CLOB(1K)))"), "2FF22");
    assert_eq!(state(&a, "VALUES ST_SPATIALREFSYS(123456789)"), "2FF59");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §14.1, §14.1.7;
/// ISO/IEC 9075-2:2023 §4.13] ST_SpatialRefSys is EQUALS ONLY BY RELATIVE
/// WITH ST_OrderingEquals: `=` and ST_OrderingEquals compare two values,
/// and ORDER BY, `<` and GROUP BY are refused.
#[test]
fn spatial_ref_sys_is_equals_only() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, S ST_SPATIALREFSYS)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, ST_SPATIALREFSYS(4326)), (2, ST_SPATIALREFSYS(4269))").unwrap();
    assert_eq!(one(&a, "SELECT ID FROM T WHERE S = ST_SPATIALREFSYS(4269)"), "2");
    assert_eq!(
        one(
            &a,
            "VALUES (ST_ORDERINGEQUALS(ST_SPATIALREFSYS(4326), ST_SPATIALREFSYS(4326)), ST_ORDERINGEQUALS(ST_SPATIALREFSYS(4326), ST_SPATIALREFSYS(4269)))"
        ),
        "0,1"
    );
    assert_eq!(state(&a, "SELECT S FROM T ORDER BY S"), "2A000");
    assert_eq!(state(&a, "VALUES ST_SPATIALREFSYS(4326) < ST_SPATIALREFSYS(4269)"), "2A000");
    assert_eq!(state(&a, "SELECT S FROM T GROUP BY S"), "2A000");
    assert_eq!(state(&a, "VALUES ST_ORDERINGEQUALS(ST_SPATIALREFSYS(4326), ST_VECTOR(1e0, 2e0))"), "2A000");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §14.1.9, §20.5]
/// ST_DEFINESRS refuses with 2FF22 a definition that is no `<spatial
/// reference system>`, when it is called, storing nothing; ST_SIZINGS
/// gives the sizes of the types' representations.
#[test]
fn definesrs_checks_its_definition() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    let e = a.run("CALL ST_DEFINESRS('BAD', 900002, 'APP', 2, 'garbage', 'x')").unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "2FF22");
    let count = "SELECT COUNT(*) FROM ST_INFORMTN_SCHEMA.ST_SPATIAL_REFERENCE_SYSTEMS WHERE SRS_ID = 900002";
    assert_eq!(one(&a, count), "0");
    let sizes = "SELECT VARIABLE_NAME, SUPPORTED_VALUE FROM ST_INFORMTN_SCHEMA.ST_SIZINGS WHERE VARIABLE_NAME IN ('ST_MaxVectorAsBinary', 'ST_MaxAngleString', 'ST_MaxDirectionString') ORDER BY 1";
    assert_eq!(one(&a, sizes), "ST_MaxAngleString,64;ST_MaxDirectionString,64;ST_MaxVectorAsBinary,29");
}
