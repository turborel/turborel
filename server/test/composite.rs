//! Arrays, rows and distinct types over a connection [ISO/IEC 9075-2:2023
//! §4.9, §4.10, §4.12], their types described and their values sent both
//! ways, nested too.

use std::sync::Arc;

use turborel::db::Database;
use turborel::value::{DataType, Decimal, Value};
use turborel_client::connection::Outcome;

use crate::common::{done, rows, start};

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// A database whose table X.T holds a key, an array of rows, and a row of
/// an array.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL, A ROW(F INTEGER, G CHAR(2)) ARRAY[3], R ROW(H INTEGER ARRAY[2]))",
    )
    .unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.10, §4.12, §6.49, §7.1]; [DESIGN]
/// RULES.md D42, D45: a query's columns of an array type and of a row type
/// are described as the database declares them, nested types too, an
/// exact type by its precision and scale; their
/// values, a null element and a null field among them, reach the client
/// as they are kept; and an array and a row sent as a prepared statement's
/// values are kept as sent.
#[test]
fn arrays_and_rows_go_both_ways() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let insert = "INSERT INTO T VALUES (1, ARRAY[ROW(1, 'a'), ROW(CAST(NULL AS INTEGER), 'b')], ROW(ARRAY[2, CAST(NULL AS INTEGER)]))";
    assert_eq!(done(&c.query(insert).unwrap()), 0);
    let out = c.query("SELECT K, A, R FROM T").unwrap();
    assert_eq!(rows(&out), ["1,ARRAY[ROW(1, 'a '), ROW(NULL, 'b ')],ROW(ARRAY[2, NULL])"]);
    let Outcome::Rows { columns, .. } = &out else { panic!("no rows") };
    let declared: Vec<String> = columns.iter().map(|c| c.data_type.to_string()).collect();
    // An exact type goes as its precision and scale, not its spelling.
    assert_eq!(declared, ["NUMERIC(10,0)", "ROW(F NUMERIC(10,0), G CHARACTER(2)) ARRAY[3]", "ROW(H NUMERIC(10,0) ARRAY[2])"]);
    let prepared = c.prepare("INSERT INTO T VALUES (?, ?, ?)").unwrap();
    let kinds: Vec<bool> = prepared.parameters().iter().map(|p| matches!(p.clone().data_type(), DataType::Array(_))).collect();
    assert_eq!(kinds, [false, true, false]);
    let array = Value::Array(Arc::from(vec![Value::Row(Arc::from(vec![int(5), Value::Char("e".into())]))]));
    let row = Value::Row(Arc::from(vec![Value::Array(Arc::from(vec![int(6)]))]));
    assert_eq!(done(&c.execute(&prepared, &[int(2), array, row]).unwrap()), 0);
    assert_eq!(rows(&c.query("SELECT A, R FROM T WHERE K = 2").unwrap()), ["ARRAY[ROW(5, 'e ')],ROW(ARRAY[6])"]);
    c.terminate().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.9.6, §11.51 GR2b; §20.11, §20.12];
/// [DESIGN] RULES.md D69: a column of a distinct type is described to the
/// client as its source type, by the transform its definition makes
/// (ID048), and its values, an array of the type's among them, reach the
/// client as its source's; a value sent as a prepared statement's is made
/// a value of the type, as one of its source is.
#[test]
fn distinct_types_go_as_their_source() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TYPE MONEY AS NUMERIC(10, 2) CREATE TABLE M (K INTEGER, P MONEY, L MONEY ARRAY[2])")
        .unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    assert_eq!(done(&c.query("INSERT INTO M VALUES (1, 5, ARRAY[MONEY(1), MONEY(2.5)])").unwrap()), 0);
    let out = c.query("SELECT P, L FROM M").unwrap();
    assert_eq!(rows(&out), ["5.00,ARRAY[1.00, 2.50]"]);
    let Outcome::Rows { columns, .. } = &out else { panic!("no rows") };
    let declared: Vec<String> = columns.iter().map(|c| c.data_type.to_string()).collect();
    assert_eq!(declared, ["NUMERIC(10,2)", "NUMERIC(10,2) ARRAY[2]"]);
    let prepared = c.prepare("INSERT INTO M (K, P) VALUES (?, ?)").unwrap();
    assert_eq!(prepared.parameters()[1].clone().data_type().to_string(), "NUMERIC(10,2)");
    assert_eq!(done(&c.execute(&prepared, &[int(2), Value::Exact(Decimal::new(725, 2))]).unwrap()), 0);
    assert_eq!(rows(&c.query("SELECT K FROM M WHERE P = MONEY(7.25)").unwrap()), ["2"]);
    c.terminate().unwrap();
}
