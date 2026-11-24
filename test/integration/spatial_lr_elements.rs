//! The routines of linear elements, position expressions, measures,
//! start values and LRMs in SQL [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §15.1 to §15.8], and casts to the linear referencing types, called
//! through SQL: what each returns, and the SQLSTATE of each exception
//! condition it raises.

use turborel::db::{Database, Session};
use turborel::value::Value;

use crate::common::{rows, state};

/// A session of a database of linear referencing routines, its table D
/// of one row.
fn session() -> (Database, Session) {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE D (X INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO D VALUES (1)").unwrap();
    (db, a)
}

/// Checks each case: the value of the expression on the left, or, of an
/// expected text starting "ERR ", the SQLSTATE it raises.
fn check(a: &Session, cases: &[(&str, &str)]) {
    for (expr, expected) in cases {
        let sql = format!("SELECT {expr} FROM D");
        match expected.strip_prefix("ERR ") {
            Some(code) => assert_eq!(state(a, &sql), *code, "{expr}"),
            None => assert_eq!(rows(a, &sql).unwrap_or_else(|e| panic!("{expr}: {e:?}")), [*expected], "{expr}"),
        }
    }
}

/// A curve element 1 of LRM 1, of type road, along (0 0) to (100 0).
const ROAD: &str = "NEW ST_LRCURVE(1, 1, ST_LRMEASURE(0), 'road', CAST(NULL AS ST_STARTVALUE ARRAY[2]), \
                    ST_GEOMFROMTEXT('LINESTRING (0 0, 100 0)'))";
/// Its well-known text.
const ROAD_TEXT: &str = "LINEARELEMENT LRCURVE LEID 1, DEFAULT LRMID 1, DEFAULT MEASURE 0, LETYPE \"road\", LINESTRING (0 0, 100 0)";
/// A directed edge element 3 of LRM 1.
const EDGE: &str = "NEW ST_LRDIRECTEDEDGE(3, 1, ST_LRMEASURE(0), CAST(NULL AS VARCHAR(8)), CAST(NULL AS ST_STARTVALUE ARRAY[2]), \
                    'G', 'topo', 5)";
/// A feature element 2 of LRM 1.
const FEATURE: &str = "ST_LRFEATURE(2, 1, ST_LRMEASURE(0), 'road', CAST(NULL AS ST_STARTVALUE ARRAY[2]), 'F1')";
/// The LRM 1 in metres, of no constraints.
const LRM: &str = "ST_LRM(1, 'km', 'absolute', 'metre', CAST(NULL AS VARCHAR(5) ARRAY[1]))";

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.2.2 to §15.2.6,
/// §15.4] a linear element's observers and mutators: a changed default
/// measure raises the warning 01F82; a null measure or LRM identifier
/// raises 2FF03, of a null SELF too; a curve of a point 2FF08.
#[test]
fn linear_element_observers_and_mutators_15_2() {
    let (_db, a) = session();
    let changed = |rest: &str| format!("LINEARELEMENT LRCURVE LEID 1, DEFAULT LRMID 1, DEFAULT MEASURE 0, LETYPE \"road\", {rest}");
    check(
        &a,
        &[
            (ROAD, ROAD_TEXT),
            (&format!("{ROAD}.ST_DEFAULTLRM(2)"), &ROAD_TEXT.replace("LRMID 1", "LRMID 2")),
            (&format!("{ROAD}.ST_DEFAULTMEASURE()"), "MEASURE 0"),
            (&format!("{ROAD}.ST_DEFAULTMEASURE(CAST(NULL AS ST_LRMEASURE))"), "ERR 2FF03"),
            (&format!("{ROAD}.ST_LETYPE()"), "road"),
            (&format!("{ROAD}.ST_LETYPE('rail')"), &ROAD_TEXT.replace("road", "rail")),
            (&format!("{ROAD}.ST_STARTVALUE(2, ST_LRMEASURE(9))"), &changed("STARTVALUES ((LRMID 2, 9)), LINESTRING (0 0, 100 0)")),
            (&format!("{ROAD}.ST_STARTVALUE(CAST(NULL AS INTEGER))"), "ERR 2FF03"),
            ("CAST(NULL AS ST_LRCURVE).ST_STARTVALUE(CAST(NULL AS INTEGER))", "ERR 2FF03"),
            ("CAST(NULL AS ST_LRCURVE).ST_STARTVALUE(1)", "NULL"),
            (&format!("{ROAD}.ST_CURVE(ST_GEOMFROMTEXT('LINESTRING (0 0, 0 50)'))"), &changed("LINESTRING (0 0, 0 50)")),
            (&format!("{ROAD}.ST_CURVE(ST_POINT(1, 1))"), "ERR 2FF08"),
        ],
    );
    let moved = format!("SELECT {ROAD}.ST_DEFAULTMEASURE(ST_LRMEASURE(7)).ST_DEFAULTMEASURE() FROM D");
    assert_eq!(rows(&a, &moved).unwrap(), ["MEASURE 7"]);
    assert_eq!(a.warning().unwrap().as_str(), "01F82");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.2.9, §15.2.10,
/// §15.3 to §15.5] each element type's constructor and function of its
/// text; the functions of GML refuse what is not GML (2FF24); a feature's
/// and a directed edge's observers and mutators, a null edge identifier
/// raising 2FF03.
#[test]
fn feature_curve_and_edge_routines_15_3() {
    let (_db, a) = session();
    let feature_text = "LINEARELEMENT LRFEATURE LEID 2, DEFAULT LRMID 1, DEFAULT MEASURE 0, LETYPE \"road\", FID \"F1\" REFERENTS ()";
    let edge_text = "LINEARELEMENT LRDIRECTEDEDGE LEID 3, DEFAULT LRMID 1, DEFAULT MEASURE 0, LETYPE NULL, E \"topo\" 5";
    let lit = |s: &str| s.replace('\'', "''");
    check(
        &a,
        &[
            (&format!("ST_LRCURVE('{}')", lit(ROAD_TEXT)), ROAD_TEXT),
            (&format!("ST_LRCURVEFROMTEXT('{}')", lit(ROAD_TEXT)), ROAD_TEXT),
            (&format!("ST_LEFROMTEXT('{}')", lit(ROAD_TEXT)), ROAD_TEXT),
            (&format!("ST_LRFEATURE('{}')", lit(feature_text)), feature_text),
            (&format!("ST_LRFEATFROMTEXT('{}')", lit(feature_text)), feature_text),
            (&format!("ST_LRDIRECTEDEDGE('{}')", lit(edge_text)), edge_text),
            (&format!("ST_LREDGEFROMTEXT('{}')", lit(edge_text)), edge_text),
            ("ST_LRCURVEFROMTEXT('LINEARELEMENT LEID 1')", "ERR 2FF22"),
            ("ST_LEFROMGML('<LinearElement/>')", "ERR 2FF24"),
            ("ST_LRFEATFROMGML('<LinearElement/>')", "ERR 2FF24"),
            ("ST_LRCURVEFROMGML('<LinearElement/>')", "ERR 2FF24"),
            ("ST_LREDGEFROMGML('<LinearElement/>')", "ERR 2FF24"),
            (FEATURE, feature_text),
            (&format!("{FEATURE}.ST_FEATUREID('F2')"), &feature_text.replace("F1", "F2")),
            (&format!("CARDINALITY({FEATURE}.ST_REFERENTS())"), "0"),
            (&format!("{FEATURE}.ST_REFERENTS(ARRAY[ST_REFERENT('a', 'b')])"), &feature_text.replace("()", "((NAME \"a\", TYPE \"b\"))")),
            (EDGE, edge_text),
            (&format!("{EDGE}.ST_TOPOLOGYTYPE('N').ST_TOPOLOGYTYPE()"), "N"),
            (&format!("{EDGE}.ST_TOPOORNETNAME('net')"), &edge_text.replace("topo", "net")),
            (&format!("{EDGE}.ST_EDGEORLINKID(9)"), &edge_text.replace(" 5", " 9")),
            (&format!("{EDGE}.ST_EDGEORLINKID(CAST(NULL AS INTEGER))"), "ERR 2FF03"),
        ],
    );
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.6.2 to §15.6.10]
/// ST_PositionExp's constructors of its text, of an element value and an
/// LRM identifier, and of an element identifier and an LRM value; its
/// observers and mutators; a null distance expression raises 2FF03, as
/// does ST_Equals of a null position; GML of no distance 2FF24.
#[test]
fn position_routines_15_6() {
    let (_db, a) = session();
    let p = "ST_POSITIONEXP(4, 2, ST_DISTANCEEXP(ST_LRMEASURE(3)))";
    let text = "POSEXP (LINEARELEMENT LEID 4), (LRM LRMID 2), (DISEXP MEASURE 25 metre)";
    let lrm_text = "(LRM LRMID 1, LRMNAME \"km\", LRMTYPE \"absolute\", LRMUOM \"metre\")";
    let with_road = format!("POSEXP (LINEARELEMENT {})", ROAD_TEXT.trim_start_matches("LINEARELEMENT "));
    check(
        &a,
        &[
            (&format!("ST_POSITIONEXP('{text}')"), text),
            (
                &format!("ST_POSITIONEXP({ROAD}, 1, ST_DISTANCEEXP(ST_LRMEASURE(3)))"),
                &format!("{with_road}, (LRM LRMID 1), (DISEXP MEASURE 3)"),
            ),
            (
                &format!("ST_POSITIONEXP(4, {LRM}, ST_DISTANCEEXP(ST_LRMEASURE(3)))"),
                &format!("POSEXP (LINEARELEMENT LEID 4), {lrm_text}, (DISEXP MEASURE 3)"),
            ),
            (&format!("{p}.ST_LINEARELEMENTID(5)"), "POSEXP (LINEARELEMENT LEID 5), (LRM LRMID 2), (DISEXP MEASURE 3)"),
            (&format!("{p}.ST_LINEARELEMENT()"), "NULL"),
            (&format!("{p}.ST_LINEARELEMENT({ROAD})"), &format!("{with_road}, (LRM LRMID 2), (DISEXP MEASURE 3)")),
            (&format!("{p}.ST_LRMID(6)"), "POSEXP (LINEARELEMENT LEID 4), (LRM LRMID 6), (DISEXP MEASURE 3)"),
            (&format!("{p}.ST_LRM()"), "NULL"),
            (&format!("{p}.ST_LRM({LRM})"), &format!("POSEXP (LINEARELEMENT LEID 4), {lrm_text}, (DISEXP MEASURE 3)")),
            (&format!("{p}.ST_DISTANCEEXP()"), "DISEXP MEASURE 3"),
            (
                &format!("{p}.ST_DISTANCEEXP(ST_DISTANCEEXP(ST_LRMEASURE(4)))"),
                "POSEXP (LINEARELEMENT LEID 4), (LRM LRMID 2), (DISEXP MEASURE 4)",
            ),
            (&format!("{p}.ST_DISTANCEEXP(CAST(NULL AS ST_DISTANCEEXP))"), "ERR 2FF03"),
            (&format!("{p}.ST_EQUALS(CAST(NULL AS ST_POSITIONEXP))"), "ERR 2FF03"),
            ("ST_POSEXPFROMGML('<PositionExpression></PositionExpression>')", "ERR 2FF24"),
            ("ST_DISEXPFROMGML('<DistanceExpression><distanceAlong>3</distanceAlong></DistanceExpression>')", "DISEXP MEASURE 3"),
        ],
    );
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.7, §15.8] the
/// observers and mutators of ST_LRMeasure and ST_StartValue: a null
/// measure raises 2FF03 and an unsupported unit 2FF14; an LRM identifier
/// not an integer raises 22000 and one out of the range of INTEGER 22003.
#[test]
fn measure_and_start_value_routines_15_7() {
    let (_db, a) = session();
    let s = "ST_STARTVALUE(1, ST_LRMEASURE(5))";
    check(
        &a,
        &[
            ("ST_LRMEASURE(2, 'metre').ST_MEASURE(CAST(NULL AS DOUBLE PRECISION))", "ERR 2FF03"),
            ("ST_LRMEASURE(2, 'metre').ST_UNITOFMEASURE('foot')", "MEASURE 2 foot"),
            ("ST_LRMEASURE(2, 'cubit')", "ERR 2FF14"),
            (s, "(LRMID 1, 5)"),
            (&format!("{s}.ST_LRM()"), "1"),
            (&format!("{s}.ST_LRM(3)"), "(LRMID 3, 5)"),
            (&format!("{s}.ST_MEASURE()"), "MEASURE 5"),
            (&format!("{s}.ST_MEASURE(ST_LRMEASURE(6))"), "(LRMID 1, 6)"),
            (&format!("{s}.ST_MEASURE(CAST(NULL AS ST_LRMEASURE))"), "ERR 2FF03"),
            ("ST_STARTVALUE(1e10, ST_LRMEASURE(5))", "ERR 22000"),
            ("ST_STARTVALUE(CAST(3000000000 AS BIGINT), ST_LRMEASURE(5))", "ERR 22003"),
        ],
    );
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.5 to
/// §15.1.10] an LRM's type, constraints and offsets: setting the offset
/// unit gives the default directions; a direction with no offset unit
/// raises 2FF89.
#[test]
fn lrm_offset_routines_15_1() {
    let (_db, a) = session();
    let lrm_text = "LRM LRMID 1, LRMNAME \"km\", LRMTYPE \"absolute\", LRMUOM \"metre\"";
    check(
        &a,
        &[
            (&format!("{LRM}.ST_LRMTYPE('relative')"), &lrm_text.replace("absolute", "relative")),
            (&format!("{LRM}.ST_CONSTRAINTS(ARRAY['a', 'b'])"), &format!("{lrm_text}, LRMCONSTRAINTS (\"a\", \"b\")")),
            (&format!("{LRM}.ST_OFFSETMEASUNIT()"), "NULL"),
            (
                &format!("{LRM}.ST_OFFSETMEASUNIT('foot')"),
                &format!("{lrm_text}, LRMOFFSETUOM \"foot\", POSLATOFFDIR \"right\", POSVEROFFDIR \"up\""),
            ),
            (&format!("{LRM}.ST_POSLATOFFSETDIR('right')"), "ERR 2FF89"),
            (&format!("{LRM}.ST_POSVEROFFSETDIR()"), "NULL"),
            (&format!("{LRM}.ST_POSVEROFFSETDIR('up')"), "ERR 2FF89"),
            (&format!("{LRM}.ST_OFFSETMEASUNIT('foot').ST_POSVEROFFSETDIR('down').ST_POSVEROFFSETDIR()"), "down"),
        ],
    );
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.14, ISO/IEC
/// 9075-2:2023 §9.2 SR2, §9.18 SR10b] a character string casts to the
/// value of its text, a value to its own type, and the null value to any;
/// a value of another type raises 2FF08; a call no routine takes 42000.
#[test]
fn casts_to_linear_referencing_types() {
    let (_db, a) = session();
    let d = "CAST('DISEXP MEASURE 3 metre' AS ST_DISTANCEEXP)";
    check(
        &a,
        &[
            (d, "DISEXP MEASURE 3 metre"),
            (&format!("CAST({d} AS ST_DISTANCEEXP)"), "DISEXP MEASURE 3 metre"),
            ("CAST(ST_LRMEASURE(3) AS ST_DISTANCEEXP)", "ERR 2FF08"),
            ("CAST(NULL AS ST_LRM)", "NULL"),
            ("ST_LRMID(ST_LRMEASURE(3))", "ERR 42000"),
        ],
    );
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21.2, §21.2.3,
/// ISO/IEC 9075-2:2023 §9.2 GR3b xxii 2B] ST_DefineLRM refuses a null
/// LRM name and an organization without its LRM identifier (23000); an
/// ARRAY of constraints past ST_MaxConstraintArrayElements raises 2202F.
#[test]
fn define_lrm_arguments_and_array_limits() {
    let (_db, a) = session();
    let definition = "'LRM LRMID 1, LRMNAME \"km\", LRMTYPE \"absolute\", LRMUOM \"metre\"'";
    let unnamed = format!("CALL ST_DEFINELRM(CAST(NULL AS VARCHAR(5)), 1, 'ACME', 7, {definition}, 'd')");
    assert_eq!(a.run(&unnamed).unwrap_err().sqlstate().as_str(), "23000");
    let half = format!("CALL ST_DEFINELRM('Y', 1, 'ACME', CAST(NULL AS INTEGER), {definition}, 'd')");
    assert_eq!(a.run(&half).unwrap_err().sqlstate().as_str(), "23000");
    let many = vec!["'c'"; 1025].join(", ");
    check(
        &a,
        &[
            (&format!("CARDINALITY({LRM}.ST_CONSTRAINTS(ARRAY[{}]).ST_CONSTRAINTS())", vec!["'c'"; 1024].join(", ")), "1024"),
            (&format!("{LRM}.ST_CONSTRAINTS(ARRAY[{many}])"), "ERR 2202F"),
        ],
    );
}
