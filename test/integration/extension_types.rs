//! Data types beyond the standard, plug-ins (RULES.md D53): registered,
//! written in column definitions and CAST, stored, compared, indexed,
//! spilled, logged and recovered, as a built-in type is; a database whose
//! files hold one opened only with it registered.

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;
use turborel::value::{Decimal, SqlType, Value};

use crate::common::{Counted, Scratch, exec, os, rows, sorted};
use crate::ident::{IDENT, Ident};

/// The identifier of `n`, its 32 hexadecimal digits.
fn hex(n: u8) -> String {
    format!("{n:032X}")
}

/// `CAST('...' AS IDENT)` of `n`.
fn cast(n: u8) -> String {
    format!("CAST('{}' AS IDENT)", hex(n))
}

/// A database with IDENT registered, and schema A: table T of an
/// identifier K, UNIQUE, and a number N.
fn identified() -> Database {
    let db = Database::new();
    db.register_type(Box::new(Ident)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K IDENT UNIQUE, N INTEGER)").unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D53: a type is registered under a regular
/// identifier in upper case, no key word, once.
#[test]
fn a_type_is_registered_once_by_its_name() {
    #[derive(Debug)]
    struct Named(&'static str);
    impl SqlType for Named {
        fn name(&self) -> &'static str {
            self.0
        }
        fn written(&self, t: &turborel::value::DataType) -> String {
            IDENT.written(t)
        }
        fn comparable(&self, other: &turborel::value::DataType) -> bool {
            IDENT.comparable(other)
        }
        fn compare(&self, a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
            IDENT.compare(a, b)
        }
        fn bucket(&self, v: &Value) -> Option<turborel::value::Bucket> {
            IDENT.bucket(v)
        }
        fn assign(&self, v: Value, t: &turborel::value::DataType, rule: &str) -> turborel::error::Result<Value> {
            IDENT.assign(v, t, rule)
        }
        fn cast(&self, v: Value, t: &turborel::value::DataType) -> turborel::error::Result<Value> {
            IDENT.cast(v, t)
        }
        fn literal(&self, v: &Value) -> String {
            IDENT.literal(v)
        }
        fn text(&self, v: &Value) -> String {
            IDENT.text(v)
        }
        fn position(&self, v: &Value) -> Option<f64> {
            IDENT.position(v)
        }
        fn descriptor(&self, t: &turborel::value::DataType) -> turborel::value::Descriptor {
            IDENT.descriptor(t)
        }
        fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
            IDENT.put_value(v, out);
        }
        fn value(&self, r: &mut turborel::bytes::Reader<'_>) -> turborel::error::Result<Value> {
            IDENT.value(r)
        }
        fn put_params(&self, t: &turborel::value::DataType, out: &mut Vec<u8>) {
            IDENT.put_params(t, out);
        }
        fn params(&self, r: &mut turborel::bytes::Reader<'_>) -> turborel::error::Result<turborel::value::DataType> {
            IDENT.params(r)
        }
        fn record_width(&self, t: &turborel::value::DataType) -> Option<usize> {
            IDENT.record_width(t)
        }
        fn encode_record(&self, v: &Value, t: &turborel::value::DataType) -> turborel::error::Result<Vec<u8>> {
            IDENT.encode_record(v, t)
        }
        fn decode_record(&self, field: &[u8], t: &turborel::value::DataType) -> Option<Value> {
            IDENT.decode_record(field, t)
        }
        fn encode_key(&self, v: &Value, t: &turborel::value::DataType, out: &mut Vec<u8>) -> turborel::error::Result<()> {
            IDENT.encode_key(v, t, out)
        }
        fn decode_key(&self, rest: &mut &[u8], t: &turborel::value::DataType) -> turborel::error::Result<Value> {
            IDENT.decode_key(rest, t)
        }
        fn anchor(&self, v: &Value, t: &turborel::value::DataType) -> Option<(Value, bool)> {
            IDENT.anchor(v, t)
        }
    }
    let db = Database::new();
    for bad in ["ident", "SELECT", "TWO WORDS", "\"IDENT\""] {
        assert_eq!(db.register_type(Box::new(Named(bad))).err().map(|e| e.code), Some(Code::Api), "{bad}");
    }
    db.register_type(Box::new(Ident)).unwrap();
    assert_eq!(db.register_type(Box::new(Named("IDENT"))).err().map(|e| e.code), Some(Code::Api), "twice");
    assert_eq!(db.types().unwrap().names().collect::<Vec<_>>(), ["IDENT"]);
}

/// Verifies: [EXTENSION] RULES.md D53: a column of a type beyond the
/// standard takes values CAST to it and its DEFAULT, a character string
/// literal; they compare, order, group and are kept unique, as its kind
/// says, through an index too; a value of it compares with no other type's.
#[test]
fn a_column_of_a_plug_in_type() {
    let db = identified();
    let a = db.session("A");
    for n in [3_u8, 1, 2] {
        a.run(&format!("INSERT INTO T VALUES ({}, {n})", cast(n))).unwrap();
    }
    assert_eq!(rows(&a, "SELECT K FROM T ORDER BY K").unwrap(), [hex(1), hex(2), hex(3)]);
    assert_eq!(rows(&a, &format!("SELECT N FROM T WHERE K = {}", cast(2))).unwrap(), ["2"]);
    assert_eq!(exec(&a, &format!("INSERT INTO T VALUES ({}, 9)", cast(2))).err(), Some(Code::Constraint), "UNIQUE");
    a.commit().unwrap();
    a.run("CREATE INDEX BY_K ON T (K)").unwrap();
    assert_eq!(rows(&a, &format!("SELECT N FROM T WHERE K > {}", cast(1))).unwrap().len(), 2, "through the index");
    a.run(&format!("INSERT INTO T VALUES ({}, 3)", cast(4))).unwrap();
    assert_eq!(sorted(&a, "SELECT DISTINCT N FROM T WHERE K <> K OR N > 0").len(), 3);
    assert_eq!(crate::common::code(&a, "SELECT N FROM T WHERE K = 1"), Code::SyntaxRule, "not comparable with a number");
    assert_eq!(crate::common::code(&a, "SELECT N FROM T WHERE K = 'X'"), Code::SyntaxRule, "nor with a string");
    assert_eq!(exec(&a, "INSERT INTO T VALUES (CAST('XYZ' AS IDENT), 5)").err(), Some(Code::Data), "not an identifier");
    assert_eq!(rows(&a, "SELECT CAST(K AS CHARACTER(32)) FROM T WHERE N = 1").unwrap(), [hex(1)], "its text");
    a.run(&format!("CREATE TABLE U (K IDENT DEFAULT '{}')", hex(7))).unwrap();
    a.run("INSERT INTO U VALUES (DEFAULT)").unwrap();
    assert_eq!(rows(&a, "SELECT K FROM U").unwrap(), [hex(7)]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D53; [ISO/IEC 9075:1992 §11.4 SR4,
/// §21.3.5] a name not qualified is a domain's of the schema before a
/// type's; a type is written with the parameters it takes, and a name
/// neither's is refused; the Information Schema describes the type by its
/// kind.
#[test]
fn names_of_plug_in_types() {
    let db = identified();
    let a = db.session("A");
    assert_eq!(exec(&a, "CREATE TABLE V (K IDENT(4))").err(), Some(Code::SyntaxRule), "IDENT takes no parameters");
    assert_eq!(exec(&a, "CREATE TABLE V (K NOPE)").err(), Some(Code::SyntaxRule), "no domain or type NOPE");
    assert_eq!(exec(&a, "CREATE TABLE V (K B.IDENT)").err(), Some(Code::SyntaxRule), "a type is in no schema");
    let described = "SELECT DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' AND COLUMN_NAME = 'K'";
    assert_eq!(rows(&a, described).unwrap().iter().map(|r| r.trim_end().to_owned()).collect::<Vec<_>>(), ["IDENT"]);
    a.run("CREATE DOMAIN IDENT AS INTEGER").unwrap();
    a.run("CREATE TABLE W (K IDENT)").unwrap();
    a.run("INSERT INTO W VALUES (5)").unwrap();
    assert_eq!(rows(&a, "SELECT K FROM W").unwrap(), ["5"], "the domain's");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D53, D38: with WORK_MEMORY at its least,
/// values of a type beyond the standard are sorted and grouped through the
/// spill area, and read back as they were.
#[test]
fn plug_in_values_spill() {
    let db = identified();
    let a = db.session("A");
    let values: Vec<String> = (0..=255_u8).map(|n| format!("({}, {})", cast(n), n % 7)).collect();
    a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    let queries = [
        "SELECT T1.N, T1.K, T2.K FROM T T1, T T2 WHERE T1.N = T2.N ORDER BY 1, 2, 3",
        "SELECT DISTINCT T1.K, T2.K FROM T T1, T T2 WHERE T1.N = T2.N",
        "SELECT T1.K, T2.K, COUNT(*) FROM T T1, T T2 WHERE T1.N = T2.N GROUP BY T1.K, T2.K",
    ];
    let in_memory: Vec<Vec<String>> = queries.iter().map(|q| sorted(&a, q)).collect();
    assert!(in_memory.first().is_some_and(|r| r.len() > 9000), "enough rows to spill");
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    db.set_setting("WORK_MEMORY", &Value::Exact(Decimal::new(64 << 10, 0))).unwrap();
    db.set_setting("SPILL_BATCH", &Value::Exact(Decimal::new(4 << 10, 0))).unwrap();
    for (q, expected) in queries.iter().zip(&in_memory) {
        assert_eq!(&sorted(&a, q), expected, "{q}, spilled");
        assert!(counted.taken() > 0, "{q} spilled");
    }
}

/// Verifies: [EXTENSION] RULES.md D53, D25: a table of a type beyond the
/// standard, and its rows, are kept by the catalog and the log, after a
/// crash and after a clean close, read with the type registered; a database
/// whose files hold one is not opened without it (SQLCODE -910).
#[test]
fn plug_in_types_are_kept() {
    let scratch = Scratch::new("plug-in-types");
    let dir = scratch.path("db");
    let open = |registered: bool| {
        let db = Database::new();
        if registered {
            db.register_type(Box::new(Ident)).unwrap();
        }
        db.open_files(&os(), &dir, &OpenOptions::default()).map(|_| db)
    };
    let db = open(true).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K IDENT UNIQUE, N INTEGER)").unwrap();
    let a = db.session("A");
    a.run(&format!("INSERT INTO T VALUES ({}, 1), ({}, 2)", cast(1), cast(2))).unwrap();
    a.commit().unwrap();
    drop((a, db));
    let kept = |db: &Database| {
        let a = db.session("A");
        assert_eq!(rows(&a, "SELECT K FROM T ORDER BY K").unwrap(), [hex(1), hex(2)]);
        assert_eq!(exec(&a, &format!("INSERT INTO T VALUES ({}, 3)", cast(1))).err(), Some(Code::Constraint), "its index kept");
        a.rollback().unwrap();
    };
    let db = open(true).unwrap();
    kept(&db);
    db.close().unwrap();
    drop(db);
    assert_eq!(open(false).err().map(|e| e.code), Some(Code::Api), "IDENT not registered");
    kept(&open(true).unwrap());
}
