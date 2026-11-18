//! Linear referencing in SQL [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §15, §21]: the thirteen types, their constructors, observers, mutators
//! and methods, the functions of their text and GML, and the linear
//! referencing methods and columns of the §21 catalog, defined by the
//! procedures built in, in a transaction and through recovery.

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::Value;

use crate::common::{Scratch, after_crash, rows, state};

fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db
}

/// The LRM 1 in metres, of type absolute.
const KM: &str = "LRM LRMID 1, LRMNAME \"km\", LRMTYPE \"absolute\", LRMUOM \"metre\"";
/// A curve element 1 of default LRM 1 along (0 0) to (100 0).
const ROAD: &str = "NEW ST_LRCurve(1, 1, ST_LRMeasure(0), CAST(NULL AS VARCHAR(8)), CAST(NULL AS ST_STARTVALUE ARRAY[2]), \
                    ST_GEOMFROMTEXT('LINESTRING (0 0, 100 0)'))";
/// The distance 25 metres along.
const D25: &str = "ST_DistanceExp(ST_LRMeasure(25, 'metre'))";

const DEFINE: &str =
    "CALL ST_DEFINELRM('KM', 1, 'ACME', 7, 'LRM LRMID 1, LRMNAME \"km\", LRMTYPE \"absolute\", LRMUOM \"metre\"', 'posts')";
const LRMS: &str = "SELECT LRM_NAME, LRMID, AUTH_NAME, AUTH_ID, DESCRIPTION, LRM.ST_LRMTYPE() FROM ST_INFORMTN_SCHEMA.ST_LRMS";
const COLUMNS: &str = "SELECT TABLE_NAME, COLUMN_NAME, LRMID FROM ST_INFORMTN_SCHEMA.ST_LR_COLUMNS ORDER BY COLUMN_NAME";

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.2 to §15.1.10,
/// §15.7] ST_LRM and ST_LRMeasure: each constructor, as a function and as
/// NEW; the observers as methods and as functions; a mutator returns a
/// changed value; a null argument of a mutator raises 2FF03 and an
/// unsupported unit 2FF14; a dynamic parameter takes its parameter's type.
#[test]
fn lrm_and_measure_routines() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, L ST_LRM, M ST_LRMEASURE)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, NEW ST_LRM(1, 'km', 'absolute', 'metre', ARRAY['c1']), ST_LRMeasure(2.5, 'foot'))").unwrap();
    a.run(&format!("INSERT INTO T VALUES (2, '{}', ST_LRMeasure(3))", KM.replace('\'', "''"))).unwrap();
    let observed = "SELECT L.ST_LRMID(), L.ST_LRMNAME(), ST_LRMTYPE(L), L.ST_UNITOFMEASURE(), L.ST_CONSTRAINTS(), \
                    M.ST_MEASURE(), M.ST_UNITOFMEASURE() FROM T ORDER BY ID";
    assert_eq!(rows(&a, observed).unwrap(), ["1,km,absolute,metre,ARRAY['c1'],2.5E0,foot", "1,km,absolute,metre,NULL,3E0,NULL"]);
    assert_eq!(
        rows(&a, "SELECT L.ST_LRMNAME('posts').ST_LRMNAME(), M.ST_MEASURE(7).ST_MEASURE() FROM T WHERE ID = 2").unwrap(),
        ["posts,7E0"]
    );
    let offsets = "SELECT ST_LRM(1, 'k', 'relative', 'metre', CAST(NULL AS VARCHAR(5) ARRAY[1]), 'metre', 'left', 'down').ST_POSLATOFFSETDIR() FROM T WHERE ID = 1";
    assert_eq!(rows(&a, offsets).unwrap(), ["left"]);
    assert_eq!(state(&a, "SELECT L.ST_LRMID(CAST(NULL AS INTEGER)) FROM T"), "2FF03");
    assert_eq!(state(&a, "SELECT L.ST_UNITOFMEASURE('cubit') FROM T"), "2FF14");
    assert_eq!(rows(&a, "SELECT CAST(NULL AS ST_LRM).ST_LRMID() FROM T WHERE ID = 1").unwrap(), ["NULL"]);
    let query = a.prepare("SELECT ST_LRMEASURE(?, ?).ST_UNITOFMEASURE() FROM T WHERE ID = 1").unwrap();
    let mut cursor = a.open_prepared(&query, &[Value::Approx(1.0), Value::Char("foot".into())]).unwrap();
    assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), "foot");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.2 to §15.4,
/// §15.6, §15.9] a curve element, assigned to a column of ST_LRCurve and
/// of its supertype ST_LinearElement, whose methods it takes; ST_Point
/// of a position expression naming the curve and the LRM by value, and
/// ST_LRPosition back; ST_Point of a geometry still makes a point.
#[test]
fn curve_locates_a_position() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, C ST_LRCURVE, E ST_LINEARELEMENT)").unwrap();
    let a = db.session("A");
    a.run(&format!("INSERT INTO T VALUES (1, {ROAD}, {ROAD})")).unwrap();
    assert_eq!(
        rows(&a, "SELECT C.ST_LINEARELEMENTID(), E.ST_DEFAULTLRM(), E.ST_LINEARELEMENTID(5).ST_LINEARELEMENTID() FROM T").unwrap(),
        ["1,1,5"]
    );
    let at = format!("ST_PositionExp(C, ST_LRM('{}'), {D25})", KM.replace('\'', "''"));
    assert_eq!(rows(&a, &format!("SELECT ST_ASTEXT(C.ST_POINT({at})) FROM T")).unwrap(), ["POINT (25 0)"]);
    let back = "SELECT C.ST_LRPOSITION(ST_POINT(40, 0)).ST_DISTANCEEXP().ST_DISTANCEALONG().ST_MEASURE() FROM T";
    assert_eq!(state(&a, back), "2FF81");
    assert_eq!(rows(&a, "SELECT ST_ASTEXT(ST_POINT(1, 2)) FROM T").unwrap(), ["POINT (1 2)"]);
    assert_eq!(rows(&a, "SELECT ST_ASTEXT(C.ST_CURVE()) FROM T").unwrap(), ["LINESTRING (0 0, 100 0)"]);
    assert_eq!(rows(&a, "SELECT E.ST_POINT(ST_POSITIONEXP(1, 1, ST_DISTANCEEXP(ST_LRMEASURE(1)))) FROM T").unwrap_err(), Code::SyntaxRule);
    assert_eq!(a.run("INSERT INTO T (ID, C) SELECT ID, E FROM T").unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.14, §15.1.11,
/// §15.2.9, §15.6.9, §15.9.11] the functions of the well-known text read
/// a value of their type, a curve as an ST_LinearElement too, and a
/// value shows its well-known text; text of another type raises.
#[test]
fn from_text_functions() {
    let db = database();
    let a = db.session("A");
    let lrm = format!("ST_LRMFROMTEXT('{}')", KM.replace('\'', "''"));
    assert_eq!(rows(&a, &format!("SELECT {lrm}.ST_LRMNAME() FROM ST_INFORMTN_SCHEMA.ST_SIZINGS FETCH FIRST 1 ROW ONLY")).unwrap(), ["km"]);
    let pe = "ST_POSEXPFROMTEXT('POSEXP (LINEARELEMENT LEID 4), (LRM LRMID 2), (DISEXP MEASURE 25 metre)')";
    assert_eq!(
        rows(&a, &format!("SELECT {pe}.ST_LINEARELEMENTID(), {pe}.ST_LRMID() FROM ST_INFORMTN_SCHEMA.ST_SIZINGS FETCH FIRST 1 ROW ONLY"))
            .unwrap(),
        ["4,2"]
    );
    assert_eq!(
        rows(&a, "SELECT ST_DISEXPFROMTEXT('DISEXP MEASURE 3 metre') FROM ST_INFORMTN_SCHEMA.ST_SIZINGS FETCH FIRST 1 ROW ONLY").unwrap(),
        ["DISEXP MEASURE 3 metre"]
    );
    assert_eq!(state(&a, "SELECT ST_LRMFROMTEXT('DISEXP MEASURE 3 metre') FROM ST_INFORMTN_SCHEMA.ST_SIZINGS"), "2FF22");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.12, §15.3,
/// §15.5, §15.8, §15.10 to §15.12] the GML of an LRM, read by
/// ST_LRMFromGML and by the constructor of one CHARACTER LARGE OBJECT; a
/// feature with a start value and a referent, observed; a directed edge;
/// a distance expression of a lateral offset whose referent is described;
/// a towards referent without a from referent raises 2FF90.
#[test]
fn features_edges_referents_and_offsets() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE D (X INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO D VALUES (1)").unwrap();
    let gml = "'<LRM><lrmID>1</lrmID><name>km</name><type>absolute</type><unitOfMeasure>metre</unitOfMeasure></LRM>'";
    assert_eq!(rows(&a, &format!("SELECT ST_LRMFROMGML({gml}).ST_LRMNAME(), NEW ST_LRM({gml}).ST_LRMID() FROM D")).unwrap(), ["km,1"]);
    let feature = "ST_LRFEATURE(2, 1, ST_LRMEASURE(0), 'road', ARRAY[ST_STARTVALUE(1, ST_LRMEASURE(5))], 'F1', \
                   ARRAY[ST_REFERENT('km0', 'post', ST_POINT(0, 0))])";
    let observed = format!("SELECT {feature}.ST_FEATUREID(), {feature}.ST_LETYPE(), CARDINALITY({feature}.ST_REFERENTS()) FROM D");
    assert_eq!(rows(&a, &observed).unwrap(), ["F1,road,1"]);
    let referent = "ST_REFERENT('km0', 'post', ST_POINT(3, 4))";
    assert_eq!(
        rows(&a, &format!("SELECT {referent}.ST_REFERENTNAME(), ST_ASTEXT({referent}.ST_POSITION()) FROM D")).unwrap(),
        ["km0,POINT (3 4)"]
    );
    let edge =
        "NEW ST_LRDIRECTEDEDGE(3, 1, ST_LRMEASURE(0), CAST(NULL AS VARCHAR(8)), CAST(NULL AS ST_STARTVALUE ARRAY[2]), 'G', 'topo', 5)";
    assert_eq!(
        rows(&a, &format!("SELECT {edge}.ST_TOPOLOGYTYPE(), {edge}.ST_TOPOORNETNAME(), {edge}.ST_EDGEORLINKID() FROM D")).unwrap(),
        ["G,topo,5"]
    );
    let lateral = "ST_LATOFFSETEXP(ST_LRMEASURE(2, 'metre'), 'kerb')";
    let distance = format!("ST_DISTANCEEXP(ST_LRMEASURE(10, 'metre'), {lateral})");
    assert_eq!(rows(&a, &format!("SELECT {distance}.ST_LATOFFSETEXP().ST_OFFSETREFDESC() FROM D")).unwrap(), ["kerb"]);
    let towards = "ST_DISTANCEEXP(ST_LRMEASURE(1), CAST(NULL AS VARCHAR(4)), CAST(NULL AS VARCHAR(4)), 'F1', 'km0', \
                   CAST(NULL AS ST_LATOFFSETEXP), CAST(NULL AS ST_VEROFFSETEXP))";
    assert_eq!(state(&a, &format!("SELECT {towards} FROM D")), "2FF90");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21.1, §21.2,
/// §15.6.8] an LRM defined and a column registered are rows of ST_LRMS
/// and ST_LR_COLUMNS, undone by ROLLBACK; the tables' constraints are
/// kept; a position expression naming its element and LRM by identifier
/// is located through them, and ST_LRPosition finds one of the default
/// LRM.
#[test]
fn catalog_defines_methods_and_columns() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, C ST_LRCURVE, L ST_LRM)").unwrap();
    let a = db.session("A");
    a.run(&format!("INSERT INTO T VALUES (1, {ROAD}, NULL)")).unwrap();
    a.run(DEFINE).unwrap();
    a.rollback().unwrap();
    assert_eq!(rows(&a, LRMS).unwrap(), Vec::<String>::new());
    a.run(&format!("INSERT INTO T VALUES (1, {ROAD}, NULL)")).unwrap();
    a.run(DEFINE).unwrap();
    assert_eq!(rows(&a, LRMS).unwrap(), ["KM,1,ACME,7,posts,absolute"]);
    let code = |sql: &str| a.run(sql).unwrap_err().code;
    assert_eq!(code(DEFINE), Code::Constraint);
    assert_eq!(
        code("CALL ST_DEFINELRM('M2', 2, 'ACME', 8, 'LRM LRMID 3, LRMNAME \"x\", LRMTYPE \"absolute\", LRMUOM \"metre\"', 'x')"),
        Code::Constraint
    );
    assert_eq!(code("CALL ST_REGISTERLRCOLUMN('A', 'T', 'ID', 1)"), Code::SyntaxRule);
    assert_eq!(code("CALL ST_REGISTERLRCOLUMN('A', 'T', 'C', 9)"), Code::Constraint);
    a.run("CALL ST_REGISTERLRCOLUMN('A', 'T', 'C', 1)").unwrap();
    assert_eq!(rows(&a, COLUMNS).unwrap(), ["T,C,1", "T,L,NULL"]);
    let located = format!("SELECT ST_ASTEXT(C.ST_POINT(ST_POSITIONEXP(1, 1, {D25}))) FROM T");
    assert_eq!(rows(&a, &located).unwrap(), ["POINT (25 0)"]);
    let equal = format!("SELECT ST_POSITIONEXP(1, 1, {D25}).ST_EQUALS(ST_POSITIONEXP(C, 1, {D25})) FROM T");
    assert_eq!(rows(&a, &equal).unwrap(), ["1"]);
    let back = "SELECT C.ST_LRPOSITION(ST_POINT(40, 0)).ST_DISTANCEEXP().ST_DISTANCEALONG().ST_MEASURE() FROM T";
    assert_eq!(rows(&a, back).unwrap(), ["4E1"]);
    assert_eq!(rows(&a, "SELECT C.ST_STARTVALUE(1).ST_MEASURE() FROM T").unwrap(), ["0E0"]);
    assert_eq!(code("CALL ST_DROPLRM('KM')"), Code::Constraint);
    a.run("CALL ST_UNREGISTERLRCOLUMN('A', 'T', 'C')").unwrap();
    a.run("CALL ST_DROPLRM('KM')").unwrap();
    assert_eq!(rows(&a, LRMS).unwrap(), Vec::<String>::new());
    assert_eq!(state(&a, &located), "2FF81");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21.2] the methods
/// defined and the columns registered survive a checkpoint, a crash and a
/// reopening; a registration goes with its column dropped; one of a
/// transaction that did not commit is not recovered.
#[test]
fn catalog_survives_recovery() {
    let scratch = Scratch::new("spatial-lr-recovery");
    let dir = scratch.path("db");
    let open = || {
        let db = database();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, C ST_LRCURVE, D ST_LRCURVE)").unwrap();
    let a = db.session("A");
    a.run(DEFINE).unwrap();
    a.commit().unwrap();
    db.checkpoint().unwrap();
    a.run("CALL ST_REGISTERLRCOLUMN('A', 'T', 'C', 1)").unwrap();
    a.run("CALL ST_REGISTERLRCOLUMN('A', 'T', 'D', 1)").unwrap();
    a.commit().unwrap();
    a.run("ALTER TABLE T DROP COLUMN D RESTRICT").unwrap();
    a.commit().unwrap();
    a.run(
        "CALL ST_DEFINELRM('LOST', 9, 'ACME', 9, 'LRM LRMID 9, LRMNAME \"l\", LRMTYPE \"absolute\", LRMUOM \"metre\"', 'never committed')",
    )
    .unwrap();
    let crashed = a;
    drop(db);
    let db = open();
    let a = db.session("A");
    assert_eq!(rows(&a, LRMS).unwrap(), ["KM,1,ACME,7,posts,absolute"]);
    assert_eq!(rows(&a, COLUMNS).unwrap(), ["T,C,1"]);
    a.commit().unwrap();
    db.checkpoint().unwrap();
    db.close().unwrap();
    drop((a, db, crashed));
    let db = open();
    assert_eq!(rows(&db.session("A"), COLUMNS).unwrap(), ["T,C,1"]);
    assert_eq!(rows(&db.session("A"), LRMS).unwrap(), ["KM,1,ACME,7,posts,absolute"]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.1, §15.3.1,
/// §15.5.1 Definitional Rules, §20.5, Annex A.1] the limits of linear
/// referencing are reported as ST_SIZINGS rows; a name of more characters
/// than its limit is refused as it is assigned to the parameter (22001).
#[test]
fn sizings_are_the_limits_enforced() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE D (X INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO D VALUES (1)").unwrap();
    let sizings = "SELECT VARIABLE_NAME, SUPPORTED_VALUE FROM ST_INFORMTN_SCHEMA.ST_SIZINGS WHERE VARIABLE_NAME IN \
                   ('ST_MaxConstraintArrayElements', 'ST_MaxConstraintLength', 'ST_MaxFeatureIDLength', 'ST_MaxLRAsGML', \
                   'ST_MaxLRAsText', 'ST_MaxLRMNameLength', 'ST_MaxPositionExpArrayElements', 'ST_MaxReferentArrayElements', \
                   'ST_MaxStartValueArrayElements', 'ST_MaxLEMeasureArrayElements') ORDER BY VARIABLE_NAME";
    let bytes = turborel::geometry::MAX_BYTES;
    assert_eq!(
        rows(&a, sizings).unwrap(),
        [
            "ST_MaxConstraintArrayElements,1024".to_owned(),
            "ST_MaxConstraintLength,256".to_owned(),
            "ST_MaxFeatureIDLength,128".to_owned(),
            "ST_MaxLEMeasureArrayElements,NULL".to_owned(),
            format!("ST_MaxLRAsGML,{bytes}"),
            format!("ST_MaxLRAsText,{bytes}"),
            "ST_MaxLRMNameLength,128".to_owned(),
            "ST_MaxPositionExpArrayElements,1024".to_owned(),
            "ST_MaxReferentArrayElements,1024".to_owned(),
            "ST_MaxStartValueArrayElements,1024".to_owned(),
        ]
    );
    let lrm = format!("ST_LRMFROMTEXT('{}')", KM.replace('\'', "''"));
    assert_eq!(rows(&a, &format!("SELECT {lrm}.ST_LRMNAME('{}').ST_LRMNAME() FROM D", "n".repeat(128))).unwrap(), ["n".repeat(128)]);
    assert_eq!(state(&a, &format!("SELECT {lrm}.ST_LRMNAME('{}') FROM D", "n".repeat(129))), "22001");
    let edge =
        "NEW ST_LRDIRECTEDEDGE(3, 1, ST_LRMEASURE(0), CAST(NULL AS VARCHAR(8)), CAST(NULL AS ST_STARTVALUE ARRAY[2]), 'G', 'topo', 5)";
    assert_eq!(state(&a, &format!("SELECT {edge}.ST_TOPOORNETNAME('{}') FROM D", "t".repeat(129))), "22001");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.6.8, RULES.md
/// D85] ST_Equals and ST_Point are the geometry functions and the
/// routines of §15 at once: a geometry call keeps its R-tree plan and its
/// rows, prepared of its constant, while one of position expressions, a
/// point of coordinates and one located along a curve are still taken.
#[test]
fn shared_names_keep_geometry_plans() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE U (ID INTEGER, G GEOMETRY(0), C ST_LRCURVE)").unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0..300_i32).map(|i| format!("({i}, ST_POINT({i}, 0), NULL)")).collect();
    a.run(&format!("INSERT INTO U VALUES {}", values.join(", "))).unwrap();
    a.run(&format!("INSERT INTO U VALUES (300, NULL, {ROAD})")).unwrap();
    a.commit().unwrap();
    db.create_index("U_G", "A.U", &["G"], "rtree").unwrap();
    let a = db.session("A");
    let equal = "SELECT ID FROM U WHERE ST_EQUALS(G, ST_POINT(7, 0)) = 1";
    let plan = a.physical_plan(equal, &Params::new()).unwrap();
    assert!(plan.contains("U_G"), "{plan}");
    assert_eq!(rows(&a, equal).unwrap(), ["7"]);
    let at = format!("ST_POSITIONEXP(C, ST_LRM('{}'), {D25})", KM.replace('\'', "''"));
    let same = format!("SELECT {at}.ST_EQUALS({at}), ST_ASTEXT(C.ST_POINT({at})) FROM U WHERE ID = 300");
    assert_eq!(rows(&a, &same).unwrap(), ["1,POINT (25 0)"]);
}
