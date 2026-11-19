//! SQL/MM Spatial's support types and routines [ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §15.13, §17, §18]: ST_Vector, ST_AffinePlacement,
//! ST_NURBSPoint and ST_Knot as SQL types with their
//! routines, ST_Vector's transform groups as CASTs, the support routines
//! of arrays, ST_VectorOffsetExp of ST_Vector values, and stored values
//! of every type across a reopen (RULES.md D123).

use std::fmt::Write as _;

use turborel::db::{Database, OpenOptions, Session};
use turborel::geometry::model::write_wkb;
use turborel::spatial::formats::wkt::read_geom;
use turborel::value::Value;

use crate::common::{Scratch, after_crash, rows, state};

fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db
}

/// The rows of `sql`, each its columns joined by commas, joined by `;`.
fn one(a: &Session, sql: &str) -> String {
    rows(a, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}")).join(";")
}

/// The constructor of the value `text` writes, through its WKB.
fn geo(text: &str) -> String {
    let hex = write_wkb(&read_geom(text, 0).unwrap()).iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02X}");
        out
    });
    format!("ST_GEOMFROMWKB(X'{hex}')")
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.2.2 to §17.2.19]
/// ST_Vector's constructors of coordinates, text, WKB and GML; its
/// coordinate, dimension, SRID and emptiness observers and mutators; and
/// its text, binary and GML functions and methods.
#[test]
fn vector_routines_17_2() {
    let db = database();
    let a = db.session("A");
    assert_eq!(one(&a, "VALUES (ST_VECTOR(1e0, 2e0), ST_VECTOR(1e0, 2e0, 3e0).ST_ASTEXT())"), "VECTOR (1 2),VECTOR Z (1 2 3)");
    assert_eq!(one(&a, "VALUES (ST_VECTOR(1e0, 2e0, 4326).ST_SRID(), ST_VECTOR(1e0, 2e0, 3e0, 4326).ST_IS3D())"), "4326,1");
    assert_eq!(one(&a, "VALUES (NEW ST_VECTOR().ST_ISEMPTY(), NEW ST_VECTOR().ST_X(), ST_VECTOR(1e0, 2e0).ST_ISEMPTY())"), "1,NULL,0");
    let v = "ST_VECTOR(1e0, 2e0)";
    assert_eq!(one(&a, &format!("VALUES ({v}.ST_X(), {v}.ST_Y(), {v}.ST_Z(), {v}.ST_X(5e0).ST_X())")), "1E0,2E0,NULL,5E0");
    assert_eq!(one(&a, &format!("VALUES ({v}.ST_Z(3e0).ST_ASTEXT(), {v}.ST_SRID(4326).ST_SRID())")), "VECTOR Z (1 2 3),4326");
    assert_eq!(one(&a, &format!("VALUES CARDINALITY({v}.ST_COORDINATES())")), "2");
    assert_eq!(state(&a, "VALUES NEW ST_VECTOR().ST_Z(1e0)"), "2FF03");
    assert_eq!(state(&a, &format!("VALUES {v}.ST_Z(3e0).ST_Z(CAST(NULL AS DOUBLE PRECISION))")), "2FF16");
    assert_eq!(state(&a, &format!("VALUES {v}.ST_X(CAST(NULL AS DOUBLE PRECISION))")), "2FF03");
    assert_eq!(one(&a, "VALUES ST_VECTORFROMTEXT('VECTOR Z (1 2 3)', 4326).ST_SRID()"), "4326");
    assert_eq!(one(&a, "VALUES NEW ST_VECTOR(CAST('VECTOR (4 5)' AS CLOB(1K))).ST_Y()"), "5E0");
    assert_eq!(one(&a, "VALUES NEW ST_VECTOR().ST_WKTTOSQL(CAST('VECTOR (4 5)' AS CLOB(1K))).ST_X()"), "4E0");
    assert_eq!(state(&a, "VALUES ST_VECTORFROMTEXT('VECTOR (1 2 3)')"), "2FF25");
    let wkb = one(&a, &format!("VALUES {v}.ST_ASBINARY()"));
    assert_eq!(wkb, "0165000000000000000000F03F0000000000000040");
    assert_eq!(one(&a, &format!("VALUES ST_VECTORFROMWKB(X'{wkb}', 4326).ST_SRID()")), "4326");
    assert_eq!(one(&a, &format!("VALUES NEW ST_VECTOR(X'{wkb}').ST_X()")), "1E0");
    assert_eq!(one(&a, &format!("VALUES NEW ST_VECTOR().ST_WKBTOSQL(X'{wkb}').ST_Y()")), "2E0");
    assert_eq!(state(&a, "VALUES ST_VECTORFROMWKB(X'0165')"), "2FF23");
    let gml = one(&a, "VALUES ST_VECTOR(1e0, 2e0, 3e0, 4326).ST_ASGML()");
    assert!(gml.contains("srsName=\"EPSG:4326\">1 2 3</gml:vector>"), "{gml}");
    assert_eq!(one(&a, &format!("VALUES (ST_VECTORFROMGML('{gml}').ST_SRID(), ST_VECTORFROMGML('{gml}', 4326).ST_SRID())")), "0,4326");
    assert_eq!(one(&a, &format!("VALUES NEW ST_VECTOR().ST_GMLTOSQL('{gml}').ST_Z()")), "3E0");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.2, §17.2.20,
/// §17.2.21; ISO/IEC 9075-2:2023 §4.13] ST_Vector is EQUALS ONLY BY
/// RELATIVE WITH ST_OrderingEquals, so `=` compares and ORDER BY and `<`
/// are refused; it CASTs to and from its well-known text and binary.
#[test]
fn vector_ordering_and_casts() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, V ST_VECTOR)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, ST_VECTOR(1e0, 2e0)), (2, ST_VECTOR(1e0, 2e0, 4326))").unwrap();
    assert_eq!(one(&a, "SELECT ID FROM T WHERE V = ST_VECTOR(1e0, 2e0)"), "1");
    assert_eq!(
        one(
            &a,
            "VALUES (ST_VECTOR(1e0, 2e0).ST_EQUALS(ST_VECTOR(1e0, 2e0, 4326)), ST_ORDERINGEQUALS(ST_VECTOR(1e0, 2e0), ST_VECTOR(1e0, 2e0)))"
        ),
        "0,0"
    );
    assert_eq!(state(&a, "SELECT V FROM T ORDER BY V"), "2A000");
    assert_eq!(state(&a, "VALUES ST_VECTOR(1e0, 2e0) < ST_VECTOR(1e0, 2e0)"), "2A000");
    assert_eq!(one(&a, "VALUES CAST(ST_VECTOR(1e0, 2e0) AS VARCHAR(64))"), "VECTOR (1 2)");
    assert_eq!(one(&a, "VALUES CAST(ST_VECTOR(1e0, 2e0) AS VARBINARY(29))"), "0165000000000000000000F03F0000000000000040");
    assert_eq!(one(&a, "VALUES CAST(X'0165000000000000000000F03F0000000000000040' AS ST_VECTOR)"), "VECTOR (1 2)");
    assert_eq!(one(&a, "VALUES CAST('VECTOR Z (1 2 3)' AS ST_VECTOR).ST_Z()"), "3E0");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.3 to §17.5]
/// ST_AffinePlacement, ST_NURBSPoint and ST_Knot: their constructors,
/// observers, mutators and ST_IsEmpty; a mutator refuses a null
/// argument; none of the three is compared.
#[test]
fn placements_nurbs_points_and_knots() {
    let db = database();
    let a = db.session("A");
    let placement = "NEW ST_AFFINEPLACEMENT(ST_POINT(1, 2), ARRAY[ST_VECTOR(1e0, 0e0), ST_VECTOR(0e0, 1e0)])";
    assert_eq!(
        one(&a, &format!("VALUES ({placement}.ST_INDIMENSION(), {placement}.ST_OUTDIMENSION(), {placement}.ST_LOCATION().ST_ASTEXT())")),
        "2,2,POINT (1 2)"
    );
    assert_eq!(one(&a, &format!("VALUES {placement}.ST_TRANSFORM(ST_POINT(3, 4)).ST_ASTEXT()")), "POINT (4 6)");
    assert_eq!(one(&a, &format!("VALUES CARDINALITY({placement}.ST_REFDIRECTIONS())")), "2");
    assert_eq!(one(&a, "VALUES NEW ST_AFFINEPLACEMENT().ST_ISEMPTY()"), "1");
    let nurbs = "NEW ST_NURBSPOINT(ST_POINT(1, 2), 0.5e0)";
    assert_eq!(
        one(&a, &format!("VALUES ({nurbs}.ST_WEIGHT(), {nurbs}.ST_WEIGHTEDPOINT().ST_ASTEXT(), {nurbs}.ST_WEIGHT(2e0).ST_WEIGHT())")),
        "5E-1,POINT (1 2),2E0"
    );
    assert_eq!(one(&a, "VALUES NEW ST_NURBSPOINT().ST_ISEMPTY()"), "1");
    assert_eq!(one(&a, "VALUES (NEW ST_KNOT(1e0, 2).ST_VALUE(), NEW ST_KNOT(1e0, 2).ST_MULTIPLICITY(3).ST_MULTIPLICITY())"), "1E0,3");
    assert_eq!(one(&a, "VALUES (NEW ST_KNOT(), NEW ST_KNOT().ST_VALUE(2e0))"), "NEW ST_KNOT(),(NEW ST_KNOT()).ST_VALUE(2e0)");
    assert_eq!(state(&a, "VALUES NEW ST_KNOT().ST_VALUE(CAST(NULL AS DOUBLE PRECISION))"), "2FF03");
    assert_eq!(state(&a, "VALUES NEW ST_KNOT(1e0, 2) = NEW ST_KNOT(1e0, 2)"), "2A000");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §18.1.1 to §18.1.26]
/// ST_MaxDimension, ST_CheckSRID, ST_GetCoordDim, ST_GetIs3D and
/// ST_GetIsMeasured of arrays of geometries, of vectors and of NURBS
/// points; the procedures ST_CheckNulls and ST_CheckConsecDups; and the
/// cast functions ST_ToPointAry and its peers, each SQLSTATE its clause
/// raises.
#[test]
fn array_support_routines_18_1() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    let mixed = "ARRAY[ST_POINT(1, 2), ST_GEOMFROMTEXT('LINESTRING (0 0, 1 1)')]";
    assert_eq!(one(&a, &format!("VALUES ST_MAXDIMENSION({mixed})")), "1");
    assert_eq!(one(&a, "VALUES ST_MAXDIMENSION(CAST(ARRAY[] AS ST_GEOMETRY ARRAY[2]))"), "-1");
    assert_eq!(one(&a, "VALUES ST_CHECKSRID(ARRAY[ST_POINT(1, 2, 4326), ST_POINT(3, 4, 4326)])"), "4326");
    assert_eq!(state(&a, "VALUES ST_CHECKSRID(ARRAY[ST_POINT(1, 2, 4326), ST_POINT(3, 4)])"), "2FF10");
    assert_eq!(one(&a, "VALUES ST_CHECKSRID(ARRAY[ST_VECTOR(1e0, 2e0, 4326), ST_VECTOR(1e0, 3e0, 4326)])"), "4326");
    assert_eq!(state(&a, "VALUES ST_CHECKSRID(CAST(NULL AS ST_VECTOR ARRAY[2]))"), "2FF03");
    let z = geo("POINT Z (1 2 3)");
    assert_eq!(one(&a, &format!("VALUES (ST_GETCOORDDIM(ARRAY[{z}]), ST_GETIS3D(ARRAY[{z}]), ST_GETISMEASURED(ARRAY[{z}]))")), "3,1,0");
    assert_eq!(state(&a, &format!("VALUES ST_GETCOORDDIM(ARRAY[{z}, ST_POINT(1, 2)])")), "2FF25");
    assert_eq!(state(&a, &format!("VALUES ST_GETCOORDDIM(ST_POINT(1, 2), ARRAY[{z}])")), "2FF25");
    assert_eq!(one(&a, "VALUES ST_GETCOORDDIM(ARRAY[ST_VECTOR(1e0, 2e0, 3e0)])"), "3");
    assert_eq!(state(&a, "VALUES ST_GETCOORDDIM(ARRAY[ST_VECTOR(1e0, 2e0), ST_VECTOR(1e0, 2e0, 3e0)])"), "2FF25");
    assert_eq!(one(&a, "VALUES ST_GETCOORDDIM(ARRAY[NEW ST_NURBSPOINT(ST_POINT(1, 2), 1e0)])"), "2");
    a.run("CALL ST_CHECKNULLS(ARRAY[ST_POINT(1, 2)])").unwrap();
    let check = |sql: &str| a.run(sql).unwrap_err().sqlstate().as_str().to_owned();
    assert_eq!(check("CALL ST_CHECKNULLS(ARRAY[ST_POINT(1, 2), CAST(NULL AS ST_POINT)])"), "2FF09");
    assert_eq!(check("CALL ST_CHECKNULLS(ARRAY[ST_VECTOR(1e0, 2e0), NEW ST_VECTOR()])"), "2FF06");
    assert_eq!(check("CALL ST_CHECKNULLS(CAST(NULL AS ST_GEOMETRY ARRAY[2]))"), "2FF03");
    assert_eq!(check("CALL ST_CHECKCONSECDUPS(ARRAY[ST_POINT(1, 2), ST_POINT(1, 2)])"), "2FF05");
    a.run("CALL ST_CHECKCONSECDUPS(ARRAY[ST_POINT(1, 2), ST_POINT(2, 2), ST_POINT(1, 2)])").unwrap();
    assert_eq!(one(&a, "VALUES CARDINALITY(ST_TOPOINTARY(ARRAY[ST_POINT(1, 2), ST_POINT(3, 4)]))"), "2");
    assert_eq!(one(&a, &format!("VALUES CARDINALITY(ST_TOCURVEARY({}))", "ARRAY[ST_GEOMFROMTEXT('LINESTRING (0 0, 1 1)')]")), "1");
    assert_eq!(state(&a, &format!("VALUES ST_TOPOINTARY({mixed})")), "2FF08");
    assert_eq!(state(&a, &format!("VALUES ST_TOTRIANGLEARY({mixed})")), "2FF68");
    assert_eq!(state(&a, &format!("VALUES ST_TOPOLYHDRLARY({mixed})")), "2FF69");
    assert_eq!(state(&a, &format!("VALUES ST_TOTINARY({mixed})")), "2FF70");
    assert_eq!(state(&a, "VALUES ST_TOPOINTARY(ARRAY[ST_POINT(1, 2), ST_GEOMFROMTEXT('POINT EMPTY')])"), "2FF06");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.13.2, §15.13.3]
/// ST_VectorOffsetExp of an ARRAY of ST_Vector values, ST_Vectors() its
/// vectors, ST_Vectors(ST_Vector ARRAY) a new value of them; a null
/// array is 2FF03 to the mutator, and more than three vectors 2202F.
#[test]
fn vector_offset_expressions_15_13() {
    let db = database();
    let a = db.session("A");
    let offset = "NEW ST_VECTOROFFSETEXP(ARRAY[ST_VECTOR(1e0, 2e0), ST_VECTOR(3e0, 4e0)])";
    assert_eq!(one(&a, &format!("VALUES {offset}")), "VECTOROFFSETS (VECTOR (1 2), VECTOR (3 4))");
    assert_eq!(one(&a, &format!("VALUES ({offset}.ST_VECTORS()[2].ST_X(), CARDINALITY({offset}.ST_VECTORS()))")), "3E0,2");
    let changed = format!("{offset}.ST_VECTORS(ARRAY[ST_VECTOR(5e0, 6e0, 7e0)])");
    assert_eq!(one(&a, &format!("VALUES {changed}.ST_VECTORS()[1].ST_ASTEXT()")), "VECTOR Z (5 6 7)");
    assert_eq!(state(&a, &format!("VALUES {offset}.ST_VECTORS(CAST(NULL AS ST_VECTOR ARRAY[3]))")), "2FF03");
    let four = "ARRAY[ST_VECTOR(1e0, 2e0), ST_VECTOR(1e0, 2e0), ST_VECTOR(1e0, 2e0), ST_VECTOR(1e0, 2e0)]";
    assert_eq!(state(&a, &format!("VALUES ST_VECTOROFFSETEXP({four})")), "2202F");
    assert_eq!(one(&a, "VALUES ST_VECTOROFFSETEXP(CAST(NULL AS ST_VECTOR ARRAY[3]))"), "NULL");
}

/// Verifies: [DESIGN] RULES.md D25, D123: a value of each of
/// the seven types, and an index keyed by an angle, survive a reopen
/// after a crash, read back as written.
#[test]
fn support_values_survive_reopen() {
    let scratch = Scratch::new("spatial-support-reopen");
    let dir = scratch.path("db");
    let open = || {
        let db = database();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, S ST_SPATIALREFSYS, A ST_ANGLE, D ST_DIRECTION, \
         V ST_VECTOR, P ST_AFFINEPLACEMENT, N ST_NURBSPOINT, K ST_KNOT)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("CREATE INDEX TA ON T (A)").unwrap();
    a.run(
        "INSERT INTO T VALUES (1, ST_SPATIALREFSYS(4326), ST_ANGLE(2e0), ST_DIRECTION(1e0), ST_VECTOR(1e0, 2e0, 3e0, 4326), \
         NEW ST_AFFINEPLACEMENT(ST_POINT(1, 2), ARRAY[ST_VECTOR(1e0, 0e0)]), NEW ST_NURBSPOINT(ST_POINT(1, 2), 0.5e0), NEW ST_KNOT(1e0, 2)), \
         (2, NULL, ST_ANGLE(1e0), NULL, NULL, NULL, NULL, NULL)",
    )
    .unwrap();
    a.commit().unwrap();
    drop((a, db));
    let db = open();
    let a = db.session("A");
    let read = "SELECT S.ST_SRID(), A.ST_RADIANS(), D.ST_RADIANS(), V.ST_ASTEXT(), V.ST_SRID(), P.ST_OUTDIMENSION(), \
                N.ST_WEIGHT(), K.ST_MULTIPLICITY() FROM T WHERE ID = 1";
    assert_eq!(one(&a, read), "4326,2E0,1E0,VECTOR Z (1 2 3),4326,2,5E-1,2");
    assert_eq!(one(&a, "SELECT ID FROM T WHERE A < ST_ANGLE(1.5e0)"), "2");
    assert_eq!(one(&a, "SELECT ID FROM T ORDER BY A"), "2;1");
    drop((a, db));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §18.1.1, §18.1.7
/// to §18.1.26] ST_MaxDimension of collections is their elements'
/// greatest, -1 of the empty set; each cast function ST_To...Ary takes an
/// array of values of its type.
#[test]
fn array_casts_of_every_type_18_1() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    let dimensions = [
        ("MULTIPOINT ((1 2))", "0"),
        ("MULTILINESTRING ((0 0, 1 1))", "1"),
        ("MULTICURVE ((0 0, 1 1))", "1"),
        ("MULTIPOLYGON (((0 0, 1 0, 0 1, 0 0)))", "2"),
        ("MULTISURFACE (((0 0, 1 0, 0 1, 0 0)))", "2"),
        ("GEOMETRYCOLLECTION (POINT (1 2), LINESTRING (0 0, 1 1))", "1"),
        ("POINT EMPTY", "-1"),
    ];
    for (text, want) in dimensions {
        assert_eq!(one(&a, &format!("VALUES ST_MAXDIMENSION(ARRAY[{}])", geo(text))), want, "{text}");
    }
    let placed = "REFERENCELOCATION AFFINEPLACEMENT (LOCATION (0 0), REFERENCEDIRECTIONS (VECTOR (1 0), VECTOR (0 1)))";
    let casts = [
        ("ST_TOLINESTRINGARY", "LINESTRING (0 0, 1 1)".to_owned()),
        ("ST_TOCIRCULARARY", "CIRCULARSTRING (0 0, 1 1, 2 0)".to_owned()),
        ("ST_TOCIRCLEARY", "CIRCLE (0 0, 1 1, 2 0)".to_owned()),
        ("ST_TOGEODESICARY", "GEODESICSTRING (0 0, 1 1)".to_owned()),
        ("ST_TOELLIPTICALARY", format!("ELLIPTICALCURVE ({placed}, UAXISLENGTH 2, VAXISLENGTH 1, STARTANGLE RADIANS (0), ENDANGLE RADIANS (1.5))")),
        (
            "ST_TONURBSARY",
            "NURBSCURVE (DEGREE 1, CONTROLPOINTS (NURBSPOINT (WEIGHTEDPOINT (0 0), WEIGHT 1), NURBSPOINT (WEIGHTEDPOINT (1 0), WEIGHT 1)), \
             KNOTS (KNOT (VALUE 0, MULTIPLICITY 2), KNOT (VALUE 1, MULTIPLICITY 2)))"
                .to_owned(),
        ),
        ("ST_TOCLOTHOIDARY", format!("CLOTHOID ({placed}, SCALEFACTOR 10, STARTDISTANCE 0, ENDDISTANCE 5)")),
        ("ST_TOSPIRALARY", format!("SPIRALCURVE ({placed}, LENGTH 10, STARTCURVATURE 0, ENDCURVATURE 0.1, SPIRALTYPE clothoid)")),
        ("ST_TOCOMPOUNDARY", "COMPOUNDCURVE ((0 0, 1 1))".to_owned()),
        ("ST_TOSURFACEARY", "POLYGON ((0 0, 1 0, 0 1, 0 0))".to_owned()),
        ("ST_TOCURVEPOLYARY", "CURVEPOLYGON ((0 0, 1 0, 0 1, 0 0))".to_owned()),
        ("ST_TOPOLYGONARY", "POLYGON ((0 0, 1 0, 0 1, 0 0))".to_owned()),
        ("ST_TOCOMPSURFARY", "COMPOUNDSURFACE (POLYGON ((0 0, 1 0, 0 1, 0 0)))".to_owned()),
        (
            "ST_TOBREPSOLIDARY",
            "BREPSOLID Z (POLYHEDRALSURFACE Z (PATCHES (POLYGON Z ((0 0 0, 0 1 0, 1 0 0, 0 0 0)), POLYGON Z ((0 0 0, 1 0 0, 0 0 1, 0 0 0)), \
             POLYGON Z ((0 0 0, 0 0 1, 0 1 0, 0 0 0)), POLYGON Z ((1 0 0, 0 1 0, 0 0 1, 1 0 0)))))"
                .to_owned(),
        ),
    ];
    for (f, text) in casts {
        assert_eq!(one(&a, &format!("VALUES CARDINALITY({f}(ARRAY[ST_GEOMFROMTEXT('{text}')]))")), "1", "{f}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.3.3, §17.3.4,
/// §17.3.7] the mutators ST_Location and ST_RefDirections of an
/// ST_AffinePlacement: a location of m is 2FF97, one of another system
/// than the directions 2FF10, of another dimension 2FF25, a null argument
/// 2FF03; a null placement gives the null value; a point of fewer
/// coordinates than the directions does not transform (2FF02).
#[test]
fn placement_mutators_check_their_arguments_17_3() {
    let db = database();
    let a = db.session("A");
    let placement = "NEW ST_AFFINEPLACEMENT(ST_POINT(1, 2), ARRAY[ST_VECTOR(1e0, 0e0), ST_VECTOR(0e0, 1e0)])";
    assert_eq!(one(&a, &format!("VALUES {placement}.ST_LOCATION(ST_POINT(5, 6)).ST_LOCATION().ST_ASTEXT()")), "POINT (5 6)");
    let one_direction = format!("{placement}.ST_REFDIRECTIONS(ARRAY[ST_VECTOR(2e0, 0e0)])");
    assert_eq!(one(&a, &format!("VALUES CARDINALITY({one_direction}.ST_REFDIRECTIONS())")), "1");
    assert_eq!(one(&a, "VALUES NEW ST_AFFINEPLACEMENT().ST_LOCATION(ST_POINT(5, 6)).ST_ISEMPTY()"), "0");
    assert_eq!(state(&a, &format!("VALUES {placement}.ST_LOCATION(TREAT({} AS ST_POINT))", geo("POINT M (1 2 3)"))), "2FF97");
    assert_eq!(state(&a, &format!("VALUES {placement}.ST_LOCATION(ST_POINT(1, 2, 4326))")), "2FF10");
    assert_eq!(state(&a, &format!("VALUES {placement}.ST_LOCATION(TREAT({} AS ST_POINT))", geo("POINT Z (1 2 3)"))), "2FF25");
    assert_eq!(state(&a, &format!("VALUES {placement}.ST_REFDIRECTIONS(ARRAY[ST_VECTOR(1e0, 0e0, 0e0)])")), "2FF25");
    assert_eq!(state(&a, &format!("VALUES {placement}.ST_LOCATION(CAST(NULL AS ST_POINT))")), "2FF03");
    assert_eq!(one(&a, "VALUES CAST(NULL AS ST_AFFINEPLACEMENT).ST_LOCATION(ST_POINT(5, 6)) IS NULL"), "TRUE");
    let line = "NEW ST_AFFINEPLACEMENT(ST_POINT(1, 2), ARRAY[ST_VECTOR(1e0, 0e0), ST_VECTOR(0e0, 1e0), ST_VECTOR(1e0, 1e0)])";
    assert_eq!(state(&a, &format!("VALUES {line}.ST_TRANSFORM(ST_POINT(3, 4))")), "2FF02");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.2.2 to §17.2.6,
/// §17.4.3, §17.5.4] ST_Vector of a text or binary string and an SRID;
/// ST_Vector(x, y, z, srid) of null parts (2FF03 for x without y or z,
/// 2FF16 of z without x, the empty vector of none, null of a null SRID);
/// ST_Y and ST_Z as mutators; ST_Coordinates of the empty vector; the
/// mutators of ST_NURBSPoint and ST_Knot, and ST_IsEmpty of a knot.
#[test]
fn vector_and_element_routines_of_null_parts_17_2() {
    let db = database();
    let a = db.session("A");
    assert_eq!(one(&a, "VALUES ST_CHECKSRID(ARRAY[ST_VECTOR('VECTOR (1 2)', 4326)])"), "4326");
    assert_eq!(one(&a, "VALUES ST_VECTOR(X'0165000000000000000000F03F0000000000000040', 7).ST_ASTEXT()"), "VECTOR (1 2)");
    let null = "CAST(NULL AS DOUBLE PRECISION)";
    assert_eq!(state(&a, &format!("VALUES ST_VECTOR(1e0, {null}, {null}, 0)")), "2FF03");
    assert_eq!(state(&a, &format!("VALUES ST_VECTOR({null}, {null}, 1e0, 0)")), "2FF16");
    assert_eq!(state(&a, &format!("VALUES ST_VECTOR(1e0, 2e0, {null}, 0)")), "2FF03");
    assert_eq!(one(&a, "VALUES ST_VECTOR(1e0, 2e0, 3e0, CAST(NULL AS INTEGER)) IS NULL"), "TRUE");
    assert_eq!(one(&a, &format!("VALUES ST_VECTOR({null}, {null}, {null}, 4326).ST_ISEMPTY()")), "1");
    assert_eq!(one(&a, "VALUES ST_VECTOR(1e0, 2e0).ST_Y(5e0).ST_Y()"), "5E0");
    assert_eq!(state(&a, &format!("VALUES ST_VECTOR(1e0, 2e0).ST_Y({null})")), "2FF03");
    assert_eq!(one(&a, "VALUES CAST(NULL AS ST_VECTOR).ST_Y(5e0) IS NULL"), "TRUE");
    assert_eq!(state(&a, "VALUES NEW ST_VECTOR().ST_Z(5e0)"), "2FF03");
    assert_eq!(one(&a, "VALUES NEW ST_VECTOR().ST_COORDINATES() IS NULL"), "TRUE");
    let nurbs = "NEW ST_NURBSPOINT(ST_POINT(1, 2), 0.5e0).ST_WEIGHTEDPOINT(ST_POINT(3, 4))";
    assert_eq!(one(&a, &format!("VALUES {nurbs}.ST_WEIGHTEDPOINT().ST_ASTEXT()")), "POINT (3 4)");
    assert_eq!(one(&a, "VALUES (NEW ST_KNOT().ST_ISEMPTY(), NEW ST_KNOT(1e0, 2).ST_ISEMPTY())"), "1,0");
    assert_eq!(one(&a, "VALUES CAST(NULL AS ST_KNOT).ST_VALUE(1e0) IS NULL"), "TRUE");
}
