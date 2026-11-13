//! BIGINT [ISO/IEC 9075-2:2023 §6.1 SR30, Feature T071]: an exact numeric
//! type of scale 0 and a precision no smaller than INTEGER's, here
//! NUMERIC(18, 0), shown by its name; its values, keys and descriptors.

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;
use turborel::value::{DataType, Decimal, Spelled, Value};

use crate::common::{Scratch, dynamic_host, exec, os, rows, run_with, set_text, setup, sorted};

/// Schema A: table T of a BIGINT B and an INTEGER K.
fn bigints() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (B BIGINT, K INTEGER)").unwrap();
    db
}

/// The Information Schema's DATA_TYPE, NUMERIC_PRECISION,
/// NUMERIC_PRECISION_RADIX and NUMERIC_SCALE of the columns of `table`.
fn described(s: &Session, table: &str) -> Vec<String> {
    let sql = format!(
        "SELECT COLUMN_NAME, DATA_TYPE, NUMERIC_PRECISION, NUMERIC_PRECISION_RADIX, NUMERIC_SCALE FROM INFORMATION_SCHEMA.COLUMNS \
         WHERE TABLE_NAME = '{table}' ORDER BY 1"
    );
    rows(s, &sql).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1 SR30, §4.5.2] BIGINT
/// is exact numeric of scale 0 and decimal precision 18, no smaller than
/// INTEGER's 10 (IV032), described as BIGINT; it holds every value of 18
/// digits, and one of 19 is refused (SQLSTATE 22003). BIGINT is no key
/// word, so a column or a domain keeps the name, yet BIGINT as a
/// column's data type is always the type.
#[test]
fn the_bigint_type_6_1() {
    let db = bigints();
    let a = db.session("A");
    assert_eq!(described(&a, "T"), ["B,BIGINT,18,10,0", "K,INTEGER,10,10,0"]);
    a.run("INSERT INTO T VALUES (999999999999999999, 1), (-999999999999999999, 2), (9223372036, 3)").unwrap();
    assert_eq!(sorted(&a, "SELECT K, B FROM T"), ["1,999999999999999999", "2,-999999999999999999", "3,9223372036"]);
    assert_eq!(exec(&a, "INSERT INTO T (B) VALUES (CAST(1E18 AS DOUBLE PRECISION))").err(), Some(Code::Data), "19 digits");
    assert_eq!(exec(&a, "INSERT INTO T (B) VALUES ('1')").err(), Some(Code::SyntaxRule), "a character string");
    a.run("CREATE TABLE W (BIGINT INTEGER)").unwrap();
    assert_eq!(rows(&a, "SELECT BIGINT FROM W").unwrap(), Vec::<String>::new(), "the name of a column");
    a.run("CREATE DOMAIN BIGINT AS SMALLINT").unwrap();
    a.run("CREATE TABLE X (A BIGINT)").unwrap();
    assert_eq!(described(&a, "X"), ["A,BIGINT,18,10,0"], "the type, not the domain");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1, §4.5.2] the name is no
/// part of the type: BIGINT and NUMERIC(18, 0) are one type, so a
/// foreign key of one references the other, and a value compares with
/// any number. A sum past 18 digits is refused (SQLCODE -301), as of any
/// exact numbers; a sum within them keeps every digit.
#[test]
fn bigint_is_numeric_of_its_precision_4_5() {
    let db = bigints();
    let a = db.session("A");
    a.run("CREATE TABLE P (N NUMERIC(18, 0) PRIMARY KEY)").unwrap();
    a.run("CREATE TABLE C (B BIGINT REFERENCES P)").unwrap();
    a.run("INSERT INTO P VALUES (123456789012345678)").unwrap();
    a.run("INSERT INTO C VALUES (123456789012345678)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO C VALUES (1)").err(), Some(Code::Constraint), "referenced");
    a.run("INSERT INTO T VALUES (4000000000, 4000000), (-5, 7)").unwrap();
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE B > K AND B = 4E9"), ["4000000"]);
    assert_eq!(rows(&a, "SELECT SUM(B), MAX(B) * 1000 FROM T").unwrap(), ["3999999995,4000000000000"]);
    a.run("INSERT INTO T VALUES (999999999999999999, 0)").unwrap();
    assert_eq!(rows(&a, "SELECT SUM(B) FROM T WHERE B > 0").err(), Some(Code::Data), "past 18 digits");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 GR10, GR11, §9.2 GR3k]
/// CAST to BIGINT: of a character string its numeric literal, of an exact
/// or approximate number its value truncated to scale 0; one past 18
/// digits is refused (SQLSTATE 22003), a string of no number too
/// (22018). An exact value with fraction digits assigned to BIGINT is
/// truncated.
#[test]
fn values_cast_to_bigint_6_13() {
    let db = bigints();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (0, 0)").unwrap();
    let cast = |v: &str| rows(&a, &format!("SELECT CAST({v} AS BIGINT) FROM T"));
    assert_eq!(cast("' -42 '"), Ok(vec!["-42".to_owned()]));
    assert_eq!(cast("3.99"), Ok(vec!["3".to_owned()]), "truncated");
    assert_eq!(cast("-2.5E0"), Ok(vec!["-2".to_owned()]));
    assert_eq!(cast("1E18"), Err(Code::Data), "19 digits");
    assert_eq!(cast("'x'"), Err(Code::Data), "no number");
    a.run("UPDATE T SET B = 12.75").unwrap();
    assert_eq!(rows(&a, "SELECT B FROM T").unwrap(), ["12"]);
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1, §11.4 SR13] a BIGINT
/// column is a key in the order of its values, negatives first, and
/// BIGINT is a type an identity column and a sequence may have [§11.4
/// SR13, §11.72 SR4], counting from 1 to 999999999999999999 if no bounds
/// are written.
#[test]
fn bigint_keys_and_identities_11_4() {
    let db = bigints();
    let a = db.session("A");
    a.run("CREATE TABLE I (ID BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, V BIGINT UNIQUE)").unwrap();
    a.run("INSERT INTO I (V) VALUES (5000000000), (-5000000000), (0)").unwrap();
    assert_eq!(rows(&a, "SELECT V, ID FROM I ORDER BY V").unwrap(), ["-5000000000,2", "0,3", "5000000000,1"]);
    assert_eq!(exec(&a, "INSERT INTO I (V) VALUES (5E9)").err(), Some(Code::Constraint), "unique");
    a.run("CREATE SEQUENCE S AS BIGINT").unwrap();
    let sql = "SELECT DATA_TYPE, NUMERIC_PRECISION, MAXIMUM_VALUE FROM INFORMATION_SCHEMA.SEQUENCES WHERE SEQUENCE_NAME = 'S'";
    assert_eq!(rows(&a, sql).unwrap(), ["BIGINT,18,999999999999999999"]);
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1] a BIGINT column keeps
/// its name and its values after the database is opened again, from its
/// log and from its checkpoint.
#[test]
fn bigint_columns_are_kept() {
    let scratch = Scratch::new("bigint");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (B BIGINT PRIMARY KEY)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (-999999999999999999), (999999999999999999)").unwrap();
    a.commit().unwrap();
    drop((a, db));
    let kept = |db: &Database| {
        let a = db.session("A");
        assert_eq!(described(&a, "T"), ["B,BIGINT,18,10,0"]);
        assert_eq!(rows(&a, "SELECT B FROM T ORDER BY B").unwrap(), ["-999999999999999999", "999999999999999999"]);
        a.rollback().unwrap();
    };
    let db = open();
    kept(&db);
    db.close().unwrap();
    drop(db);
    kept(&open());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.1 SR6g, SR7k, Table 30,
/// §20.10 GR7d] DESCRIBE gives a BIGINT item TYPE 25, its precision 18 and
/// scale 0; its DATA is got into a BIGINT target, not an INTEGER one
/// (SQLSTATE 22005); an item set TYPE 25 is valid, takes as DATA a BIGINT
/// value and not an INTEGER one (22005), and gives it to the statement.
#[test]
fn bigint_in_a_descriptor_20_1() {
    let db = setup();
    let s = db.session("APP");
    s.run("CREATE TABLE G (B BIGINT)").unwrap();
    s.run("INSERT INTO G VALUES (123456789012)").unwrap();
    let bigint = DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Bigint };
    let mut p = dynamic_host().declare("BG", bigint, Value::Exact(Decimal::new(5, 0)));
    set_text(&mut p, "SELECT B FROM G WHERE B > ?");
    run_with(&s, "PREPARE Q FROM TEXT", &mut p).unwrap();
    run_with(&s, "ALLOCATE DESCRIPTOR 'D' WITH MAX 1", &mut p).unwrap();
    run_with(&s, "ALLOCATE DESCRIPTOR 'E' WITH MAX 1", &mut p).unwrap();
    run_with(&s, "DESCRIBE OUTPUT Q USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    let field = |p: &mut turborel::params::Params, area: &str, f: &str| {
        s.execute(&format!("GET DESCRIPTOR '{area}' VALUE 1 N = {f}"), p).unwrap();
        p.value("N").to_string()
    };
    let shape: Vec<String> = ["TYPE", "PRECISION", "SCALE"].iter().map(|f| field(&mut p, "D", f)).collect();
    assert_eq!(shape, ["25", "18", "0"]);
    run_with(&s, "SET DESCRIPTOR 'E' COUNT = 1", &mut p).unwrap();
    run_with(&s, "SET DESCRIPTOR 'E' VALUE 1 TYPE = 25", &mut p).unwrap();
    assert_eq!((field(&mut p, "E", "PRECISION"), field(&mut p, "E", "SCALE")), ("0".to_owned(), "0".to_owned()), "GR4c: none set");
    assert_eq!(run_with(&s, "SET DESCRIPTOR 'E' VALUE 1 DATA = K", &mut p), Err("22005"), "an INTEGER value");
    run_with(&s, "SET DESCRIPTOR 'E' VALUE 1 DATA = BG", &mut p).unwrap();
    run_with(&s, "EXECUTE Q INTO SQL DESCRIPTOR 'D' USING SQL DESCRIPTOR 'E'", &mut p).unwrap();
    run_with(&s, "GET DESCRIPTOR 'D' VALUE 1 BG = DATA", &mut p).unwrap();
    assert_eq!(p.value("BG").to_string(), "123456789012");
    assert_eq!(run_with(&s, "GET DESCRIPTOR 'D' VALUE 1 N = DATA", &mut p), Err("22005"), "an INTEGER target");
}
