//! BOOLEAN [ISO/IEC 9075-2:2023 §4.6, §5.3, §6.1]: the truth values as
//! values, kept in columns, compared, cast, and written as predicates.

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;
use turborel::value::{DataType, Value};

use crate::common::{Scratch, code, dynamic_host, exec, os, rows, run_with as run, set_text as text, sorted};

/// Schema A: table T of an INTEGER K and a BOOLEAN B.
fn booleans() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, B BOOLEAN)").unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §5.3 SR47, GR14; §6.1] TRUE, FALSE and
/// UNKNOWN are literals of BOOLEAN, UNKNOWN its null value, and a BOOLEAN
/// column keeps them.
#[test]
fn boolean_literals_are_kept_5_3() {
    let db = booleans();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, TRUE), (2, FALSE), (3, UNKNOWN), (4, NULL)").unwrap();
    assert_eq!(sorted(&a, "SELECT K, B FROM T"), ["1,TRUE", "2,FALSE", "3,NULL", "4,NULL"]);
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE B"), ["1"]);
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE NOT B"), ["2"]);
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE B IS UNKNOWN"), ["3", "4"]);
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE B IS NOT TRUE"), ["2", "3", "4"]);
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE B IS NULL"), ["3", "4"]);
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE B = FALSE"), ["2"]);
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE B > FALSE"), ["1"], "True is greater than False");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.29, §6.46] a predicate is a value
/// where a value is written: in a select list, a VALUES row, an UPDATE's
/// SET, a CASE result and a CAST operand.
#[test]
fn predicates_are_values_6_46() {
    let db = booleans();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 1 < 2), (2, (1 = 2)), (3, 1 = 1 AND 2 > 3)").unwrap();
    assert_eq!(sorted(&a, "SELECT K, B, K > 1, K IS NULL OR B FROM T"), ["1,TRUE,FALSE,TRUE", "2,FALSE,TRUE,FALSE", "3,FALSE,TRUE,FALSE"]);
    a.run("UPDATE T SET B = K = 2").unwrap();
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE B"), ["2"]);
    assert_eq!(
        rows(&a, "SELECT CASE WHEN K = 2 THEN K > 1 ELSE FALSE END, CAST(K = 2 AS VARCHAR(5)) FROM T WHERE K = 2").unwrap(),
        ["TRUE,TRUE"]
    );
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE (K = 1) = (B = FALSE) AND K < 3"), ["1", "2"]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 GR11f, GR12f, GR21a] a character
/// string casts to BOOLEAN as a literal, spaces trimmed and in any case,
/// else SQLSTATE 22018; a truth value casts to 'TRUE' or 'FALSE', else
/// 22018 if too long.
#[test]
fn booleans_are_cast_6_13() {
    let db = booleans();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, CAST(' true ' AS BOOLEAN)), (2, CAST('False' AS BOOLEAN)), (3, CAST('unknown' AS BOOLEAN))").unwrap();
    assert_eq!(sorted(&a, "SELECT K, B FROM T"), ["1,TRUE", "2,FALSE", "3,NULL"]);
    assert_eq!(code(&a, "SELECT CAST('yes' AS BOOLEAN) FROM T"), Code::Data, "22018");
    assert_eq!(rows(&a, "SELECT CAST(B AS CHAR(6)) || '|' FROM T WHERE K = 1").unwrap(), ["TRUE  |"]);
    assert_eq!(code(&a, "SELECT CAST(B AS CHAR(4)) FROM T WHERE K = 2"), Code::Data, "22018: FALSE is longer than 4");
    assert!(exec(&a, "INSERT INTO T VALUES (4, 1)").is_err(), "a number is no truth value");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.46 SR3] a value written as a
/// condition is of type BOOLEAN, else SQLCODE -103.
#[test]
fn a_predicand_is_boolean_6_46() {
    let db = booleans();
    let a = db.session("A");
    assert_eq!(code(&a, "SELECT K FROM T WHERE K"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT K FROM T WHERE B AND K"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT CASE WHEN K THEN 1 END FROM T"), Code::SyntaxRule);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR7e, GR7d ii, vii, viii] EVERY
/// is False if a value is False, else True; ANY and SOME are True if a
/// value is True, else False; nulls are left out, and of no values each
/// is null; the argument is a BOOLEAN, else SQLCODE -103.
#[test]
fn every_any_and_some_10_9() {
    let db = booleans();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, TRUE), (1, TRUE), (2, TRUE), (2, FALSE), (3, FALSE), (3, NULL), (4, NULL)").unwrap();
    assert_eq!(
        sorted(&a, "SELECT K, EVERY(B), ANY(B), SOME(B), EVERY(DISTINCT B) FROM T GROUP BY K"),
        ["1,TRUE,TRUE,TRUE,TRUE", "2,FALSE,TRUE,TRUE,FALSE", "3,FALSE,FALSE,FALSE,FALSE", "4,NULL,NULL,NULL,NULL"]
    );
    assert_eq!(rows(&a, "SELECT EVERY(K > 0), ANY(K > 3), every(K > 3) FROM T").unwrap(), ["TRUE,TRUE,FALSE"]);
    assert_eq!(rows(&a, "SELECT EVERY(B) FROM T WHERE K > 9").unwrap(), ["NULL"], "of no values");
    assert_eq!(sorted(&a, "SELECT K FROM T GROUP BY K HAVING EVERY(B)"), ["1"]);
    assert_eq!(
        sorted(&a, "SELECT DISTINCT K FROM T WHERE K = ANY (SELECT K FROM T WHERE NOT B)"),
        ["2", "3"],
        "a quantifier before a subquery"
    );
    assert_eq!(code(&a, "SELECT EVERY(K) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT ANY(K) FROM T"), Code::SyntaxRule);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.6.2, §11.5 SR4a vii] truth values
/// order False before True, group and are kept distinct, key an index and
/// a UNIQUE constraint, are a column's default, a domain's type and a
/// check, and are kept in a column store as in rows.
#[test]
fn booleans_are_ordered_grouped_and_keyed_4_6() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE DOMAIN FLAG AS BOOLEAN DEFAULT FALSE CHECK (VALUE IS NOT NULL)
           CREATE TABLE T (K INTEGER, B BOOLEAN DEFAULT TRUE, F FLAG, UNIQUE (B, K))
           CREATE TABLE C (K INTEGER, B BOOLEAN CHECK (B OR K > 0)) USING columnstore",
    )
    .unwrap();
    let a = db.session("A");
    a.run("CREATE INDEX TB ON T (B)").unwrap();
    a.run("INSERT INTO T (K) VALUES (1)").unwrap();
    a.run("INSERT INTO T VALUES (2, FALSE, TRUE), (3, NULL, FALSE), (4, FALSE, FALSE)").unwrap();
    assert_eq!(rows(&a, "SELECT K, B, F FROM T WHERE K = 1").unwrap(), ["1,TRUE,FALSE"], "the defaults");
    assert_eq!(rows(&a, "SELECT K, B FROM T ORDER BY B, K").unwrap(), ["2,FALSE", "4,FALSE", "1,TRUE", "3,NULL"], "False before True");
    assert_eq!(rows(&a, "SELECT B, COUNT(*) FROM T GROUP BY B ORDER BY B").unwrap(), ["FALSE,2", "TRUE,1", "NULL,1"]);
    assert_eq!(sorted(&a, "SELECT DISTINCT B FROM T"), ["FALSE", "NULL", "TRUE"]);
    assert_eq!(sorted(&a, "SELECT B FROM T UNION SELECT F FROM T"), ["FALSE", "NULL", "TRUE"]);
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE B = FALSE"), ["2", "4"], "through the index");
    assert_eq!(rows(&a, "SELECT MAX(B), MIN(B) FROM T").unwrap(), ["TRUE,FALSE"]);
    assert_eq!(exec(&a, "INSERT INTO T VALUES (2, FALSE, TRUE)").err(), Some(Code::Constraint), "UNIQUE");
    assert_eq!(exec(&a, "INSERT INTO T VALUES (5, TRUE, NULL)").err(), Some(Code::Constraint), "the domain's check");
    a.run("INSERT INTO C VALUES (1, FALSE), (0, TRUE), (2, NULL)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO C VALUES (0, FALSE)").err(), Some(Code::Constraint), "the table's check");
    assert_eq!(sorted(&a, "SELECT K, B FROM C"), ["0,TRUE", "1,FALSE", "2,NULL"]);
    assert_eq!(sorted(&a, "SELECT C.K FROM C JOIN T ON C.B = T.B"), ["0", "1", "1"]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1] RULES.md D25: a BOOLEAN column,
/// and an index on one, are kept, after a crash and after a clean close;
/// INFORMATION_SCHEMA describes the column as BOOLEAN.
#[test]
fn booleans_are_kept() {
    let scratch = Scratch::new("booleans");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, B BOOLEAN UNIQUE)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, TRUE), (2, FALSE), (3, NULL)").unwrap();
    a.commit().unwrap();
    drop((a, db));
    let kept = |db: &Database| {
        let a = db.session("A");
        assert_eq!(sorted(&a, "SELECT K, B FROM T"), ["1,TRUE", "2,FALSE", "3,NULL"]);
        assert_eq!(exec(&a, "INSERT INTO T VALUES (4, TRUE)").err(), Some(Code::Constraint), "its index kept");
        assert_eq!(rows(&a, "SELECT DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE COLUMN_NAME = 'B'").unwrap(), ["BOOLEAN"]);
        a.rollback().unwrap();
    };
    let db = open();
    kept(&db);
    db.close().unwrap();
    drop(db);
    kept(&open());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.1 Table 30, SR6j; §17.6 GR5] a
/// BOOLEAN item of a descriptor area is of TYPE 16, with no other field;
/// a dynamic parameter compared with a BOOLEAN is one, and takes a truth
/// value through the API and through an area.
#[test]
fn booleans_are_described_and_given_20_1() {
    let db = booleans();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, TRUE), (2, FALSE)").unwrap();
    let p = a.prepare("SELECT K FROM T WHERE B = ?").unwrap();
    assert_eq!(p.parameters().iter().map(|t| t.data_type()).collect::<Vec<_>>(), [DataType::Boolean]);
    let mut opened = a.open_prepared(&p, &[Value::Bool(false)]).unwrap();
    assert_eq!(opened.fetch().unwrap().map(|r| r[0].to_string()), Some("2".to_owned()));
    drop(opened);
    let mut h = dynamic_host().declare("BO", DataType::Boolean, Value::Bool(true));
    text(&mut h, "SELECT B FROM T WHERE B = ?");
    run(&a, "PREPARE Q FROM TEXT", &mut h).unwrap();
    run(&a, "ALLOCATE DESCRIPTOR 'D' WITH MAX 2", &mut h).unwrap();
    run(&a, "DESCRIBE OUTPUT Q USING SQL DESCRIPTOR 'D'", &mut h).unwrap();
    a.execute("GET DESCRIPTOR 'D' VALUE 1 N = TYPE", &mut h).unwrap();
    assert_eq!(h.value("N").to_string(), "16", "of the result");
    run(&a, "DESCRIBE INPUT Q USING SQL DESCRIPTOR 'D'", &mut h).unwrap();
    a.execute("GET DESCRIPTOR 'D' VALUE 1 N = TYPE", &mut h).unwrap();
    assert_eq!(h.value("N").to_string(), "16", "of the parameter");
    run(&a, "SET DESCRIPTOR 'D' VALUE 1 TYPE = 16", &mut h).unwrap();
    run(&a, "SET DESCRIPTOR 'D' VALUE 1 DATA = BO", &mut h).unwrap();
    h.set("BO", Value::Null);
    run(&a, "EXECUTE Q INTO BO USING SQL DESCRIPTOR 'D'", &mut h).unwrap();
    assert_eq!(h.value("BO").to_string(), "TRUE");
    a.commit().unwrap();
}

/// Verifies: [DESIGN] RULES.md D34: a comparison of a BOOLEAN column with
/// a truth value, constant or a dynamic parameter, probes an index on it.
#[test]
fn a_boolean_index_is_probed_d34() {
    let db = booleans();
    let a = db.session("A");
    let values: Vec<String> = (0..2000).map(|k| format!("({k}, {})", k % 500 == 0)).collect();
    a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    a.run("CREATE INDEX TB ON T (B)").unwrap();
    let query = "SELECT K FROM T WHERE B = TRUE";
    assert_eq!(sorted(&a, query), ["0", "1000", "1500", "500"]);
    let plan = a.explain(query, &turborel::params::Params::new()).unwrap();
    assert!(plan.contains("using TB"), "{plan}");
    let p = a.prepare("SELECT K FROM T WHERE B = ?").unwrap();
    let mut opened = a.open_prepared(&p, &[Value::Bool(true)]).unwrap();
    let mut found = 0;
    while opened.fetch().unwrap().is_some() {
        found += 1;
    }
    assert_eq!(found, 4);
}
