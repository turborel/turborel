//! The data types of SQL descriptor areas [ISO/IEC 9075:1992 §17.1,
//! Tables 17-20]: a row of every type put into one and taken from it, each
//! item's type described and valid; a plug-in's type (RULES.md D53); and
//! what makes an item's type not valid.

use turborel::db::{Completion, Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

use crate::common::{code_with as code, dynamic_host, number, run_with as run, set_text as text, setup};
use crate::ident::Ident;

/// The field `field` of item `n` of area 'D', into the target `target`.
fn field(s: &Session, n: i64, field: &str, target: &str, p: &mut Params) -> String {
    s.execute(&format!("GET DESCRIPTOR 'D' VALUE {n} {target} = {field}"), p).unwrap();
    p.value(target).to_string()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.9 GR3, GR7, §17.1 SR2, SR3,
/// Tables 18-20] a row of every standard type is put into an area DESCRIBE
/// set, each item's type valid: a value cast to it, INDICATOR 0, DATA the
/// value; DATA is got into targets of the item's type, a number's of its
/// name and precision, a fixed string's of its length, a varying one's of
/// no shorter length, a datetime's and an interval's of its type.
#[test]
fn a_row_of_every_type_into_a_descriptor_17_9() {
    let db = setup();
    let s = db.session("APP");
    s.run(
        "CREATE TABLE W (A CHAR(4), B VARCHAR(7), C NUMERIC(5,2), D DECIMAL(4), E INTEGER, F SMALLINT, G FLOAT(30), H REAL, \
         I DOUBLE PRECISION, J BIT(3), L BIT VARYING(9), M DATE, O TIME(3), Q TIMESTAMP(2) WITH TIME ZONE, \
         R INTERVAL DAY(3) TO SECOND(4), S INTERVAL YEAR TO MONTH, X TIME WITH TIME ZONE, Y TIMESTAMP, Z INTERVAL SECOND(2,3), \
         V INTERVAL DAY)",
    )
    .unwrap();
    s.run(
        "INSERT INTO W VALUES ('ab', 'abc', 1.25, 12, 7, 3, 1.5E0, 2.5E0, 3.5E0, B'101', B'1101', DATE '2026-10-30', \
         TIME '12:34:56.789', TIMESTAMP '2026-10-30 01:02:03.45+01:00', INTERVAL '1 02:03:04.5' DAY TO SECOND, \
         INTERVAL '1-2' YEAR TO MONTH, TIME '01:02:03+00:00', TIMESTAMP '2026-10-30 04:05:06', INTERVAL '5.25' SECOND, \
         INTERVAL '9' DAY)",
    )
    .unwrap();
    let mut p = dynamic_host()
        .declare("CH", DataType::Character { length: 4 }, Value::Null)
        .declare("VA", DataType::Varying { length: 20 }, Value::Null)
        .declare("NU", DataType::Exact { precision: 5, scale: 2, spelled: Spelled::Numeric }, Value::Null)
        .declare("FL", DataType::Approximate { precision: 53, spelled: Spelled::Float }, Value::Null)
        .declare("RE", DataType::Approximate { precision: 24, spelled: Spelled::Real }, Value::Null)
        .declare("BI", DataType::Bit { length: 3 }, Value::Null)
        .declare("DA", DataType::Date, Value::Null);
    text(&mut p, "SELECT * FROM W");
    run(&s, "PREPARE Q FROM TEXT", &mut p).unwrap();
    run(&s, "ALLOCATE DESCRIPTOR 'D' WITH MAX 20", &mut p).unwrap();
    run(&s, "DESCRIBE Q USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    s.declare_cursor("DECLARE R CURSOR FOR Q").unwrap();
    run(&s, "OPEN R", &mut p).unwrap();
    assert_eq!(run(&s, "FETCH R INTO SQL DESCRIPTOR 'D'", &mut p), Ok(Completion::Success));
    let indicators: Vec<String> = (1..=20).map(|n| field(&s, n, "INDICATOR", "N", &mut p)).collect();
    assert!(indicators.iter().all(|i| i == "0"), "{indicators:?}");
    let got = [(1, "CH"), (2, "VA"), (3, "NU"), (5, "K"), (7, "FL"), (8, "RE"), (10, "BI"), (12, "DA")];
    let values: Vec<String> = got.iter().map(|(n, target)| field(&s, *n, "DATA", target, &mut p)).collect();
    assert_eq!(values, ["ab  ", "abc", "1.25", "7", "1.5E0", "2.5E0", "101", "2026-10-30"]);
    assert_eq!(field(&s, 11, "RETURNED_LENGTH", "N", &mut p), "4", "of a BIT VARYING");
    assert_eq!(field(&s, 11, "RETURNED_OCTET_LENGTH", "N", &mut p), "1");
    assert_eq!(field(&s, 19, "PRECISION", "N", &mut p), "3");
    assert_eq!(field(&s, 20, "DATETIME_INTERVAL_CODE", "N", &mut p), "3");
    assert_eq!(run(&s, "GET DESCRIPTOR 'D' VALUE 4 NU = DATA", &mut p), Err("22005"), "DECIMAL(4) to NUMERIC(5,2)");
    assert_eq!(run(&s, "GET DESCRIPTOR 'D' VALUE 9 FL = DATA", &mut p), Err("22005"), "DOUBLE PRECISION to FLOAT");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.1 SR3, §17.9 GR5b iii,
/// GR6b iii, GR7g ii, §17.5 GR5] an item of CHARACTER of another character
/// set than SQL_TEXT is not valid (07001); a varying string's
/// RETURNED_LENGTH is its value's, cast to a fixed-length item too;
/// DATETIME_INTERVAL_CODE set of no datetime or interval sets nothing
/// with it.
#[test]
fn what_makes_an_item_valid_17_1() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    text(&mut p, "SELECT ID FROM T WHERE NAME = ?");
    run(&s, "PREPARE S FROM TEXT", &mut p).unwrap();
    run(&s, "ALLOCATE DESCRIPTOR 'D' WITH MAX 2", &mut p).unwrap();
    run(&s, "DESCRIBE INPUT S USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 CHARACTER_SET_NAME = 'LATIN1'", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE S INTO N USING SQL DESCRIPTOR 'D'", &mut p), Err("07001"));
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 TYPE = 4, DATETIME_INTERVAL_CODE = 3", &mut p).unwrap();
    assert_eq!(field(&s, 1, "PRECISION", "N", &mut p), "0", "GR5f: nothing with it");
    text(&mut p, "SELECT CAST(NAME AS VARCHAR(6)) FROM T WHERE ID = 2");
    run(&s, "PREPARE V FROM TEXT", &mut p).unwrap();
    run(&s, "DESCRIBE V USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 TYPE = 1, LENGTH = 6", &mut p).unwrap();
    run(&s, "EXECUTE V INTO SQL DESCRIPTOR 'D'", &mut p).unwrap();
    assert_eq!(field(&s, 1, "RETURNED_LENGTH", "N", &mut p), "6", "GR7g ii 2: SV's length");
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 TYPE = 99", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE V INTO SQL DESCRIPTOR 'D'", &mut p), Err("07002"), "no type of Table 18");
}

/// Verifies: [EXTENSION] RULES.md D53, D57 [ISO/IEC 9075:1992 §17.1 Table
/// 18, §17.9 GR3e] a plug-in's type is described by the code -1, and an
/// item of it takes its column's value; TYPE set -1 describes no type
/// (07002).
#[test]
fn a_plug_in_type_described_17_1() {
    let db = Database::new();
    db.register_type(Box::new(Ident)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K IDENT, N INTEGER)").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (CAST('000000000000000000000000000000FF' AS IDENT), 1)").unwrap();
    let mut p = dynamic_host();
    text(&mut p, "SELECT K FROM T");
    run(&s, "PREPARE Q FROM TEXT", &mut p).unwrap();
    run(&s, "ALLOCATE DESCRIPTOR 'D'", &mut p).unwrap();
    run(&s, "DESCRIBE Q USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    assert_eq!(field(&s, 1, "TYPE", "N", &mut p), "-1");
    assert_eq!(run(&s, "EXECUTE Q INTO SQL DESCRIPTOR 'D'", &mut p), Ok(Completion::Success));
    assert_eq!(field(&s, 1, "INDICATOR", "N", &mut p), "0");
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 TYPE = -1", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE Q INTO SQL DESCRIPTOR 'D'", &mut p), Err("07002"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4 SR14-SR16, §6.2] an
/// extended name's value is a character string of 128 octets at most
/// (SQLCODE -103); a parameter written with a colon is embedded SQL's, not
/// implemented (-900); one not declared breaks §7.3 SR3 (-103); a
/// descriptor item is a name (-102).
#[test]
fn names_and_parameters_of_dynamic_sql_5_4() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    text(&mut p, &"D".repeat(129));
    assert_eq!(code(&s, "ALLOCATE DESCRIPTOR TEXT", &mut p), Code::SyntaxRule);
    p.set("DN", Value::Char("D".to_owned()));
    run(&s, "ALLOCATE DESCRIPTOR DN WITH MAX K", &mut p).unwrap();
    assert_eq!(code(&s, "GET DESCRIPTOR DN :N = COUNT", &mut p), Code::Unsupported);
    assert_eq!(code(&s, "SET DESCRIPTOR DN COUNT = :N", &mut p), Code::Unsupported);
    assert_eq!(code(&s, "GET DESCRIPTOR DN NOPE = COUNT", &mut p), Code::SyntaxRule);
    assert_eq!(code(&s, "GET DESCRIPTOR DN VALUE 1 N = 5", &mut p), Code::Syntax);
    assert_eq!(code(&s, "FETCH GLOBAL CN", &mut p), Code::Syntax);
    p.set("K", number(1));
    assert_eq!(run(&s, "GET DESCRIPTOR DN VALUE K N = TYPE", &mut p), Ok(Completion::NoData));
}
