//! Large objects in SQL descriptor areas [ISO/IEC 9075-2:2023 §20.1 SR6c,
//! Table 30, §20.5, §20.9]: a CLOB and a BLOB column described, of codes 40
//! and 30, their lengths, octet lengths and a character one's character
//! set; their values fetched into an area; and an item set to either type
//! describing a dynamic parameter's value.

use turborel::db::{Completion, Database, Session};
use turborel::params::Params;
use turborel::value::lob::LobKind;
use turborel::value::{CharUnits, DataType, Value};

use crate::common::{dynamic_host, run_with as run, set_text as text};

/// The field `field` of item `n` of area 'D', into the target `target`.
fn field(s: &Session, n: i64, field: &str, target: &str, p: &mut Params) -> String {
    s.execute(&format!("GET DESCRIPTOR 'D' VALUE {n} {target} = {field}"), p).unwrap();
    p.value(target).to_string()
}

/// Schema A: table L of a CLOB(10) C and a BLOB(20) B, holding ('abc', X'0102').
fn lobs() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE L (C CLOB(10), B BLOB(20))").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO L VALUES ('abc', X'0102')").unwrap();
    a.commit().unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.9 GR7, §20.1 Table 30]
/// DESCRIBE of a CHARACTER LARGE OBJECT column gives TYPE 40, its length,
/// four octets to a character and SQL_TEXT; of a BINARY LARGE OBJECT
/// column TYPE 30 and its length in octets; FETCH into the area gives
/// their values.
#[test]
fn large_object_columns_described_20_9() {
    let db = lobs();
    let s = db.session("A");
    let mut p =
        dynamic_host().declare("CL", DataType::Lob { kind: LobKind::Character, length: 10, units: CharUnits::Characters }, Value::Null);
    text(&mut p, "SELECT C, B FROM L");
    run(&s, "PREPARE Q FROM TEXT", &mut p).unwrap();
    run(&s, "ALLOCATE DESCRIPTOR 'D' WITH MAX 2", &mut p).unwrap();
    run(&s, "DESCRIBE Q USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    let described: Vec<String> = ["TYPE", "LENGTH", "OCTET_LENGTH"].iter().map(|f| field(&s, 1, f, "N", &mut p)).collect();
    assert_eq!(described, ["40", "10", "40"]);
    assert_eq!(field(&s, 1, "CHARACTER_SET_NAME", "TXT", &mut p).trim_end(), "SQL_TEXT");
    let binary: Vec<String> = ["TYPE", "LENGTH", "OCTET_LENGTH"].iter().map(|f| field(&s, 2, f, "N", &mut p)).collect();
    assert_eq!(binary, ["30", "20", "20"]);
    assert_eq!(run(&s, "EXECUTE Q INTO SQL DESCRIPTOR 'D'", &mut p), Ok(Completion::Success));
    assert_eq!(field(&s, 1, "DATA", "CL", &mut p), "abc");
    assert_eq!(field(&s, 1, "INDICATOR", "N", &mut p), "0");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.1 SR6c, §20.5 GR4,
/// §20.11 GR5] DESCRIBE INPUT of parameters of large object columns gives
/// TYPE 40 and 30; an item set to either type with a LENGTH describes a
/// CHARACTER or BINARY LARGE OBJECT of that length, so a literal of a
/// fixed string type is no DATA of it [ISO/IEC 9075:1992 §17.5 GR5]
/// (22005); items of string types give the large object columns their
/// values.
#[test]
fn an_item_set_to_a_large_object_type_20_5() {
    let db = lobs();
    let s = db.session("A");
    let mut p = dynamic_host();
    text(&mut p, "INSERT INTO L VALUES (?, ?)");
    run(&s, "PREPARE I FROM TEXT", &mut p).unwrap();
    run(&s, "ALLOCATE DESCRIPTOR 'D' WITH MAX 2", &mut p).unwrap();
    run(&s, "DESCRIBE INPUT I USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    assert_eq!([field(&s, 1, "TYPE", "N", &mut p), field(&s, 2, "TYPE", "N", &mut p)], ["40", "30"]);
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 TYPE = 40, LENGTH = 10", &mut p).unwrap();
    assert_eq!(run(&s, "SET DESCRIPTOR 'D' VALUE 1 DATA = 'xyz'", &mut p), Err("22005".to_owned()), "a CHARACTER(3)");
    run(&s, "SET DESCRIPTOR 'D' VALUE 2 TYPE = 30, LENGTH = 20", &mut p).unwrap();
    assert_eq!(run(&s, "SET DESCRIPTOR 'D' VALUE 2 DATA = X'09'", &mut p), Err("22005".to_owned()), "a BINARY(1)");
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 TYPE = 1, LENGTH = 3", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 DATA = 'xyz'", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'D' VALUE 2 TYPE = 60, LENGTH = 1", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'D' VALUE 2 DATA = X'09'", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE I USING SQL DESCRIPTOR 'D'", &mut p), Ok(Completion::Success));
    let inserted = crate::common::rows(&s, "SELECT C, B FROM L WHERE OCTET_LENGTH(B) = 1").unwrap();
    assert_eq!(inserted, ["xyz,09"]);
}
