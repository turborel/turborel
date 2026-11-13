//! Columns of array and row types [ISO/IEC 9075-2:2023 §4.10, §4.12,
//! §11.4, §11.5, §11.24]: kept across a close, a crash and a checkpoint;
//! their defaults, domains, keys and indexes; their description in the
//! Information Schema; and the host parameters that give them values.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::types::Field;
use turborel::value::{DataType, Decimal, Spelled, Value};

use crate::common::{Scratch, after_crash, failure, rows, sorted};

/// The database in `dir`, opened from its files by `fs`.
fn opened(fs: &Arc<dyn FileSystem>, dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db
}

/// INTEGER.
fn integer() -> DataType {
    DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer }
}

/// The whole number `n`.
fn whole(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap()
}

/// Verifies: [DESIGN] RULES.md D68, D25 array and row values, nested,
/// with null elements and fields, in base table columns and of a domain,
/// are read the same after the database is closed and opened, crashes, or
/// crashes after a checkpoint.
#[test]
fn kept_after_reopening() {
    let table = "CREATE SCHEMA AUTHORIZATION A
        CREATE DOMAIN D AS INTEGER ARRAY[2]
        CREATE TABLE T (K INTEGER, A INTEGER ARRAY[3], P ROW(X INTEGER, Y VARCHAR(5) ARRAY[2]),
          N ROW(A INTEGER, B ROW(C CHARACTER(2), E DECIMAL(5,2))), V D)";
    let expected = [
        "1,ARRAY[1, 2],ROW(1, ARRAY['a', NULL]),ROW(1, ROW('x ', 1.50)),ARRAY[9]",
        "2,NULL,NULL,NULL,NULL",
        "3,ARRAY[],ROW(NULL, NULL),ROW(3, NULL),ARRAY[]",
    ];
    for (case, checkpoint, closed) in [("closed", false, true), ("crashed", false, false), ("checkpointed", true, false)] {
        let scratch = Scratch::new(&format!("composite-{case}"));
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let before = opened(&fs, &dir);
        before.schema(table).unwrap();
        let a = before.session("A");
        a.run("INSERT INTO T VALUES (1, ARRAY[1, 2], ROW(1, ARRAY['a', CAST(NULL AS VARCHAR(5))]), ROW(1, ROW('x', 1.5)), ARRAY[9])")
            .unwrap();
        a.run("INSERT INTO T VALUES (2, NULL, NULL, NULL, NULL)").unwrap();
        a.commit().unwrap();
        if checkpoint {
            before.checkpoint().unwrap();
        }
        let none = "CAST(NULL AS ROW(C CHARACTER(2), E DECIMAL(5,2)))";
        a.run(&format!(
            "INSERT INTO T VALUES (3, ARRAY[], ROW(CAST(NULL AS INTEGER), CAST(NULL AS VARCHAR(5) ARRAY[2])), ROW(3, {none}), ARRAY[])"
        ))
        .unwrap();
        a.commit().unwrap();
        if closed {
            before.close().unwrap();
        }
        let db = opened(&if closed { Arc::clone(&fs) } else { after_crash() }, &dir);
        assert_eq!(sorted(&db.session("A"), "SELECT * FROM T"), expected, "{case}");
        db.close().unwrap();
        drop((db, before));
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.5 SR3e, §6.5 SR1, §11.11 GR] ARRAY[]
/// is the default of a column of an array type: as a column is defined,
/// and as one is added, every row taking it; it is the default of no other
/// type.
#[test]
fn empty_array_default_11_5() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, A INTEGER ARRAY[3] DEFAULT ARRAY[])").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T (K) VALUES (1)").unwrap();
    a.run("ALTER TABLE T ADD COLUMN W ROW(F INTEGER) ARRAY[4] DEFAULT ARRAY[]").unwrap();
    assert_eq!(query(&a, "SELECT K, A, CARDINALITY(W) FROM T"), ["1,ARRAY[],0"]);
    let defaults = "SELECT COLUMN_NAME, COLUMN_DEFAULT FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' AND COLUMN_NAME <> 'K'";
    assert_eq!(sorted(&a, defaults), ["A,ARRAY[]", "W,ARRAY[]"]);
    assert_eq!(failure(a.run("CREATE TABLE U (K INTEGER DEFAULT ARRAY[])")).0, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.24, §9.2 GR3] a domain of an array
/// type holds no array of more elements than its maximum cardinality
/// (2202F), and checks its constraints of each value.
#[test]
fn of_a_domain_11_24() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE DOMAIN D AS INTEGER ARRAY[2] CHECK (CARDINALITY(VALUE) > 0) CREATE TABLE T (K INTEGER, V D)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, ARRAY[7, 8]), (2, NULL)").unwrap();
    assert_eq!(failure(a.run("INSERT INTO T VALUES (3, ARRAY[1, 2, 3])")), (Code::Data, "2202F".to_owned()));
    assert_eq!(failure(a.run("INSERT INTO T VALUES (4, ARRAY[])")).0, Code::Constraint);
    assert_eq!(sorted(&a, "SELECT K, V FROM T"), ["1,ARRAY[7, 8]", "2,NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.7 SR1, §9.12 SR3] a key's columns
/// are operands of a grouping operation, as an index's are: a UNIQUE row
/// column is kept unique and an index on one is used, while a key or an
/// index of an array column, or of a row holding an array, is refused.
#[test]
fn keys_and_indexes() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N ROW(A INTEGER, B CHARACTER(2)) UNIQUE, A INTEGER ARRAY[3], P ROW(X INTEGER ARRAY[2]))").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, ROW(1, 'x'), NULL, NULL), (2, ROW(2, 'x'), NULL, NULL), (3, NULL, NULL, NULL)").unwrap();
    assert_eq!(failure(a.run("INSERT INTO T VALUES (4, ROW(1, 'x'), NULL, NULL)")).0, Code::Constraint);
    a.commit().unwrap();
    a.run("CREATE INDEX I ON T (N)").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT K FROM T WHERE N = ROW(2, 'x')"), ["2"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE N < ROW(2, 'a')"), ["1"]);
    a.commit().unwrap();
    for sql in [
        "CREATE INDEX J ON T (A)",
        "CREATE INDEX L ON T (P)",
        "CREATE TABLE U (A INTEGER ARRAY[2] PRIMARY KEY)",
        "CREATE TABLE U (A INTEGER ARRAY[2] UNIQUE)",
        "CREATE TABLE U (P ROW(X INTEGER ARRAY[2]) UNIQUE)",
    ] {
        assert_eq!(failure(a.run(sql)).0, Code::SyntaxRule, "{sql}");
        a.rollback().unwrap();
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.10.2, §4.12.1] ISO/IEC 9075-11
/// ELEMENT_TYPES and FIELDS describe the descriptors subordinate to a
/// column's or a domain's, nested, each by its DTD_IDENTIFIER and the one
/// it is subordinate to; COLUMNS and DOMAINS give each data type
/// descriptor's maximum cardinality and identifier, a column of a domain
/// none of its own.
#[test]
fn described_in_the_information_schema() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION APP CREATE DOMAIN DA AS ROW(P INTEGER ARRAY[4], Q VARCHAR(3))
           CREATE TABLE T (K INTEGER, R ROW(X INTEGER, Y ROW(Z CHARACTER(2)) ARRAY[5]), A INTEGER ARRAY[3], D DA)",
    )
    .unwrap();
    let s = db.session("APP");
    let elements =
        "SELECT OBJECT_NAME, OBJECT_TYPE, COLLECTION_TYPE_IDENTIFIER, DTD_IDENTIFIER, DATA_TYPE FROM INFORMATION_SCHEMA.ELEMENT_TYPES";
    assert_eq!(sorted(&s, elements), ["DA,DOMAIN,1.1,1.1.1,INTEGER", "T,TABLE,2.2,2.2.1,ROW", "T,TABLE,3,3.1,INTEGER"]);
    let fields = "SELECT OBJECT_NAME, ROW_IDENTIFIER, FIELD_NAME, ORDINAL_POSITION, DTD_IDENTIFIER, DATA_TYPE,
        CHARACTER_MAXIMUM_LENGTH, MAXIMUM_CARDINALITY FROM INFORMATION_SCHEMA.FIELDS";
    assert_eq!(
        sorted(&s, fields),
        [
            "DA,1,P,1,1.1,ARRAY,NULL,4",
            "DA,1,Q,2,1.2,CHARACTER VARYING,3,NULL",
            "T,2,X,1,2.1,INTEGER,NULL,NULL",
            "T,2,Y,2,2.2,ARRAY,NULL,5",
            "T,2.2.1,Z,1,2.2.1.1,CHARACTER,2,NULL",
        ]
    );
    let columns =
        "SELECT COLUMN_NAME, DATA_TYPE, MAXIMUM_CARDINALITY, DTD_IDENTIFIER FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T'";
    assert_eq!(sorted(&s, columns), ["A,ARRAY,3,3", "D,ROW,NULL,NULL", "K,INTEGER,NULL,1", "R,ROW,NULL,2"]);
    assert_eq!(query(&s, "SELECT DOMAIN_NAME, DATA_TYPE, DTD_IDENTIFIER FROM INFORMATION_SCHEMA.DOMAINS"), ["DA,ROW,1"]);
}

/// Verifies: [DESIGN] a host program's array argument suits an array
/// parameter of no fewer elements than it has, each element null or
/// suiting the element type; a row argument suits a row parameter of its
/// degree, each field null or suiting its field's type. MC/DC of the
/// array's two conditions: 3 elements against 4 of ARRAY[3] (the
/// cardinality), and 'a' against 1 (an element); of the row's two: a
/// degree of 2 against 1, and a field 'a' against 1.
#[test]
fn host_arguments_of_composite_types() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (A INTEGER ARRAY[3], P ROW(X INTEGER, Y INTEGER))").unwrap();
    let a = db.session("A");
    let array = DataType::array(integer(), 3);
    let row = DataType::row(vec![
        Field { name: "X".into(), data_type: integer(), unnamed: false },
        Field { name: "Y".into(), data_type: integer(), unnamed: false },
    ]);
    let run = |arr: Vec<Value>, fields: Vec<Value>| {
        let mut p =
            Params::new().declare("ARR", array.clone(), Value::Array(arr.into())).declare("RW", row.clone(), Value::Row(fields.into()));
        a.execute("INSERT INTO T VALUES (ARR, RW)", &mut p).map(|_| ()).map_err(|e| e.code)
    };
    assert_eq!(run(vec![whole(1), Value::Null, whole(3)], vec![whole(1), Value::Null]), Ok(()), "every condition true");
    assert_eq!(run(vec![whole(1); 4], vec![whole(1), whole(2)]), Err(Code::Api), "the array's cardinality");
    assert_eq!(run(vec![Value::Char("a".into())], vec![whole(1), whole(2)]), Err(Code::Api), "an element");
    assert_eq!(run(vec![whole(1)], vec![whole(1)]), Err(Code::Api), "the row's degree");
    assert_eq!(run(vec![whole(1)], vec![whole(1), Value::Char("a".into())]), Err(Code::Api), "a field");
    assert_eq!(query(&a, "SELECT * FROM T"), ["ARRAY[1, NULL, 3],ROW(1, NULL)"]);
}
