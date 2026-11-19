//! The routines of linear referencing in SQL [ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §15.2 to §15.13], each constructor, observer and mutator
//! called through SQL: what it returns, and the SQLSTATE of each
//! exception condition it raises.

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

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.10.2 to
/// §15.10.7] ST_Referent's constructors, of a position, a location or
/// both null; its observers and mutators, a null type raising 2FF03, a
/// position not a point 2FF08, and a position with a location of no
/// linear element 2FF83.
#[test]
fn referent_routines_15_10() {
    let (_db, a) = session();
    let at = "ST_POSITIONEXP(1, 1, ST_DISTANCEEXP(ST_LRMEASURE(5)))";
    let other = "ST_POSITIONEXP(2, 3, ST_DISTANCEEXP(ST_LRMEASURE(6)))";
    let located = format!("ST_REFERENT('r1', 'post', {at})");
    let changed = format!("ST_REFERENT('r1', 'post').ST_CHANGEPOSANDLOC(ST_POINT(1, 1), {other})");
    let both = format!("ST_REFERENT('r1', 'post', ST_POINT(1, 2), {at})");
    let location = "POSEXP (LINEARELEMENT LEID 1), (LRM LRMID 1), (DISEXP MEASURE 5)";
    check(
        &a,
        &[
            ("ST_REFERENT('r1', 'post')", "(NAME \"r1\", TYPE \"post\")"),
            (&located, &format!("(NAME \"r1\", TYPE \"post\", LOCATION {location})")),
            (&format!("{located}.ST_LOCATION()"), location),
            ("ST_REFERENT('r1', 'post', CAST(NULL AS ST_POINT), CAST(NULL AS ST_POSITIONEXP))", "(NAME \"r1\", TYPE \"post\")"),
            ("ST_REFERENT('r1', 'post').ST_REFERENTTYPE()", "post"),
            ("ST_REFERENT('r1', 'post').ST_REFERENTNAME('r2')", "(NAME \"r2\", TYPE \"post\")"),
            ("ST_REFERENT('r1', 'post').ST_REFERENTTYPE('sign')", "(NAME \"r1\", TYPE \"sign\")"),
            ("ST_REFERENT('r1', 'post').ST_REFERENTTYPE(CAST(NULL AS VARCHAR(5)))", "ERR 2FF03"),
            ("ST_REFERENT('r1', 'post').ST_POSITION(ST_POINT(7, 8))", "(NAME \"r1\", TYPE \"post\", POSITION POINT (7 8))"),
            ("ST_REFERENT('r1', 'post').ST_POSITION(ST_LINEFROMTEXT('LINESTRING (0 0, 1 1)'))", "ERR 2FF08"),
            ("ST_REFERENT('r1', 'post').ST_POSITION()", "NULL"),
            (
                &format!("ST_REFERENT('r1', 'post').ST_LOCATION({other})"),
                "(NAME \"r1\", TYPE \"post\", LOCATION POSEXP (LINEARELEMENT LEID 2), (LRM LRMID 3), (DISEXP MEASURE 6))",
            ),
            (&changed, "ERR 2FF83"),
            (&both, "ERR 2FF83"),
            (
                "ST_REFERENT('r1', 'post').ST_CHANGEPOSANDLOC(CAST(NULL AS ST_POINT), CAST(NULL AS ST_POSITIONEXP))",
                "(NAME \"r1\", TYPE \"post\")",
            ),
            ("CAST(NULL AS ST_REFERENT).ST_REFERENTNAME('x')", "NULL"),
        ],
    );
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.11] ST_LatOffsetExp
/// of a distance, a geometry or a description; its observers and
/// mutators, a geometry given one of a description raising 2FF94.
#[test]
fn lateral_offset_routines_15_11() {
    let (_db, a) = session();
    let kerb = "ST_LATOFFSETEXP(ST_LRMEASURE(2), 'kerb')";
    let from = "ST_LATOFFSETEXP(ST_LRMEASURE(2), ST_POINT(1, 1))";
    check(
        &a,
        &[
            ("ST_LATOFFSETEXP(ST_LRMEASURE(2))", "LATERALOFFSET 2"),
            (from, "LATERALOFFSET 2 FROM POINT (1 1)"),
            (&format!("{kerb}.ST_OFFSETLATDIST()"), "MEASURE 2"),
            (&format!("{kerb}.ST_OFFSETLATDIST(ST_LRMEASURE(4))"), "LATERALOFFSET 4 FROM \"kerb\""),
            (&format!("{kerb}.ST_OFFSETLATDIST(CAST(NULL AS ST_LRMEASURE))"), "LATERALOFFSET NULL FROM \"kerb\""),
            (&format!("{from}.ST_FEATUREGEOMETRY()"), "POINT (1 1)"),
            (&format!("{kerb}.ST_FEATUREGEOMETRY(ST_POINT(3, 3))"), "ERR 2FF94"),
            (&format!("{kerb}.ST_FEATUREGEOMETRY(CAST(NULL AS ST_POINT))"), "LATERALOFFSET 2 FROM \"kerb\""),
            (&format!("{kerb}.ST_OFFSETREFDESC('wall')"), "LATERALOFFSET 2 FROM \"wall\""),
            (&format!("{kerb}.ST_OFFSETREFDESC(CAST(NULL AS VARCHAR(4)))"), "LATERALOFFSET 2"),
            (&format!("{from}.ST_OFFSETREFDESC()"), "NULL"),
        ],
    );
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.12, §15.13]
/// ST_VerOffsetExp of a distance, a geometry or a description, its
/// observers and mutators, a geometry given one of a description raising
/// 2FF94; ST_VectorOffsetExp of an ARRAY of vectors and ST_Vectors, a
/// null array raising 2FF03.
#[test]
fn vertical_and_vector_offset_routines_15_12() {
    let (_db, a) = session();
    let deck = "ST_VEROFFSETEXP(ST_LRMEASURE(3), 'deck')";
    let from = "ST_VEROFFSETEXP(ST_LRMEASURE(3), ST_POINT(1, 1))";
    let vectors = "ST_VECTOROFFSETEXP(ARRAY[ST_VECTOR(1e0, 0e0), ST_VECTOR(0e0, 1e0)])";
    let one = "ST_VECTOROFFSETEXP(ARRAY[ST_VECTOR(1e0, 0e0)])";
    check(
        &a,
        &[
            ("ST_VEROFFSETEXP(ST_LRMEASURE(3))", "VERTICALOFFSET 3"),
            (from, "VERTICALOFFSET 3 FROM POINT (1 1)"),
            (deck, "VERTICALOFFSET 3 FROM \"deck\""),
            (&format!("{deck}.ST_OFFSETVERDIST()"), "MEASURE 3"),
            (&format!("{deck}.ST_OFFSETVERDIST(ST_LRMEASURE(9))"), "VERTICALOFFSET 9 FROM \"deck\""),
            (&format!("{deck}.ST_OFFSETVERDIST(CAST(NULL AS ST_LRMEASURE))"), "VERTICALOFFSET NULL FROM \"deck\""),
            (&format!("{from}.ST_FEATUREGEOMETRY()"), "POINT (1 1)"),
            (&format!("{deck}.ST_FEATUREGEOMETRY(ST_POINT(5, 5))"), "ERR 2FF94"),
            (&format!("{deck}.ST_FEATUREGEOMETRY(CAST(NULL AS ST_POINT))"), "VERTICALOFFSET 3 FROM \"deck\""),
            (&format!("{deck}.ST_OFFSETREFDESC()"), "deck"),
            (&format!("{deck}.ST_OFFSETREFDESC('roof')"), "VERTICALOFFSET 3 FROM \"roof\""),
            (&format!("{deck}.ST_OFFSETREFDESC(CAST(NULL AS VARCHAR(4)))"), "VERTICALOFFSET 3"),
            (vectors, "VECTOROFFSETS (VECTOR (1 0), VECTOR (0 1))"),
            (&format!("CARDINALITY({vectors}.ST_VECTORS())"), "2"),
            (&format!("{one}.ST_VECTORS(ARRAY[ST_VECTOR(2e0, 0e0)])"), "VECTOROFFSETS (VECTOR (2 0))"),
            (&format!("{one}.ST_VECTORS(CAST(NULL AS ST_VECTOR ARRAY[2]))"), "ERR 2FF03"),
        ],
    );
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9.2] each of
/// ST_DistanceExp's constructors: of its text, of a measure with no, one
/// or two referents and a lateral, a vertical or a vector offset; one
/// RETURNS NULL ON NULL INPUT of a null offset.
#[test]
fn distance_constructors_15_9_2() {
    let (_db, a) = session();
    let (lat, ver) = ("ST_LATOFFSETEXP(ST_LRMEASURE(2))", "ST_VEROFFSETEXP(ST_LRMEASURE(3))");
    let vec = "ST_VECTOROFFSETEXP(ARRAY[ST_VECTOR(1e0, 0e0)])";
    let m = "ST_LRMEASURE(1)";
    let from = "DISEXP MEASURE 1 FROM \"F1\" \"r1\"";
    let towards = "DISEXP MEASURE 1 FROM \"F1\" \"r1\" TOWARDS \"F2\" \"r2\"";
    check(
        &a,
        &[
            ("ST_DISTANCEEXP('DISEXP MEASURE 3 metre')", "DISEXP MEASURE 3 metre"),
            (&format!("ST_DISTANCEEXP({m}, {lat})"), "DISEXP MEASURE 1, LATERALOFFSET 2"),
            (&format!("ST_DISTANCEEXP({m}, {ver})"), "DISEXP MEASURE 1, VERTICALOFFSET 3"),
            (&format!("ST_DISTANCEEXP({m}, {lat}, {ver})"), "DISEXP MEASURE 1, LATERALOFFSET 2, VERTICALOFFSET 3"),
            (&format!("ST_DISTANCEEXP({m}, {vec})"), "DISEXP MEASURE 1, VECTOROFFSETS (VECTOR (1 0))"),
            (&format!("ST_DISTANCEEXP({m}, 'F1', 'r1')"), from),
            (&format!("ST_DISTANCEEXP({m}, 'F1', 'r1', {lat})"), &format!("{from}, LATERALOFFSET 2")),
            (&format!("ST_DISTANCEEXP({m}, 'F1', 'r1', {ver})"), &format!("{from}, VERTICALOFFSET 3")),
            (&format!("ST_DISTANCEEXP({m}, 'F1', 'r1', {lat}, {ver})"), &format!("{from}, LATERALOFFSET 2, VERTICALOFFSET 3")),
            (&format!("ST_DISTANCEEXP({m}, 'F1', 'r1', {vec})"), &format!("{from}, VECTOROFFSETS (VECTOR (1 0))")),
            (&format!("ST_DISTANCEEXP({m}, 'F1', 'r1', 'F2', 'r2')"), towards),
            (&format!("ST_DISTANCEEXP({m}, 'F1', 'r1', 'F2', 'r2', {lat})"), &format!("{towards}, LATERALOFFSET 2")),
            (&format!("ST_DISTANCEEXP({m}, 'F1', 'r1', 'F2', 'r2', {ver})"), &format!("{towards}, VERTICALOFFSET 3")),
            (&format!("ST_DISTANCEEXP({m}, 'F1', 'r1', 'F2', 'r2', {vec})"), &format!("{towards}, VECTOROFFSETS (VECTOR (1 0))")),
            (&format!("ST_DISTANCEEXP({m}, 'F1', 'r1', 'F2', 'r2', CAST(NULL AS ST_VECTOROFFSETEXP))"), "NULL"),
        ],
    );
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9.3 to §15.9.11]
/// ST_DistanceExp's observers and mutators: a null distance along raises
/// 2FF03, a towards referent with no from referent 2FF90, a vector offset
/// with a lateral one 2FF92; the GML of no number raises 2FF24.
#[test]
fn distance_observers_and_mutators_15_9() {
    let (_db, a) = session();
    let d = "ST_DISTANCEEXP(ST_LRMEASURE(1), 'F1', 'r1', 'F2', 'r2')";
    let (lat, ver) = ("ST_LATOFFSETEXP(ST_LRMEASURE(2))", "ST_VEROFFSETEXP(ST_LRMEASURE(2))");
    let vec = "ST_VECTOROFFSETEXP(ARRAY[ST_VECTOR(1e0, 0e0)])";
    let plain = "ST_DISTANCEEXP(ST_LRMEASURE(1))";
    let gml = "ST_DISEXPFROMGML('<DistanceExpression><distanceAlong><measure>3</measure></distanceAlong></DistanceExpression>')";
    check(
        &a,
        &[
            (&format!("{d}.ST_DISTANCEALONG(ST_LRMEASURE(8))"), "DISEXP MEASURE 8 FROM \"F1\" \"r1\" TOWARDS \"F2\" \"r2\""),
            (&format!("{d}.ST_DISTANCEALONG(CAST(NULL AS ST_LRMEASURE))"), "ERR 2FF03"),
            (&format!("{d}.ST_FROMREFFEAID()"), "F1"),
            (&format!("{d}.ST_FROMREFFEAID('G1')"), "DISEXP MEASURE 1 FROM \"G1\" \"r1\" TOWARDS \"F2\" \"r2\""),
            (&format!("{d}.ST_FROMREFNAME()"), "r1"),
            (&format!("{d}.ST_FROMREFNAME('s1')"), "DISEXP MEASURE 1 FROM \"F1\" \"s1\" TOWARDS \"F2\" \"r2\""),
            (&format!("{d}.ST_TOWARDSREFFEAID()"), "F2"),
            (&format!("{d}.ST_TOWARDSREFFEAID('G2')"), "DISEXP MEASURE 1 FROM \"F1\" \"r1\" TOWARDS \"G2\" \"r2\""),
            (&format!("{d}.ST_TOWARDSREFNAME()"), "r2"),
            (&format!("{d}.ST_TOWARDSREFNAME('s2')"), "DISEXP MEASURE 1 FROM \"F1\" \"r1\" TOWARDS \"F2\" \"s2\""),
            (&format!("{plain}.ST_TOWARDSREFNAME('s2')"), "ERR 2FF90"),
            (&format!("ST_DISTANCEEXP(ST_LRMEASURE(1), {lat}).ST_LATOFFSETEXP()"), "LATERALOFFSET 2"),
            (&format!("{plain}.ST_LATOFFSETEXP({lat})"), "DISEXP MEASURE 1, LATERALOFFSET 2"),
            (&format!("ST_DISTANCEEXP(ST_LRMEASURE(1), {ver}).ST_VEROFFSETEXP()"), "VERTICALOFFSET 2"),
            (&format!("{plain}.ST_VEROFFSETEXP({ver})"), "DISEXP MEASURE 1, VERTICALOFFSET 2"),
            (&format!("ST_DISTANCEEXP(ST_LRMEASURE(1), {vec}).ST_VECTOROFFSETEXP()"), "VECTOROFFSETS (VECTOR (1 0))"),
            (&format!("{plain}.ST_VECTOROFFSETEXP({vec})"), "DISEXP MEASURE 1, VECTOROFFSETS (VECTOR (1 0))"),
            (&format!("ST_DISTANCEEXP(ST_LRMEASURE(1), {lat}).ST_VECTOROFFSETEXP({vec})"), "ERR 2FF92"),
            (gml, "ERR 2FF24"),
        ],
    );
}
