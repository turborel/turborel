//! Data types beyond the standard, plug-ins (RULES.md D53): registered,
//! written in column definitions and CAST, stored, compared, indexed,
//! spilled, logged and recovered, as a built-in type is; a database whose
//! files hold one opened only with it registered; a character string
//! stored in one that takes it; functions declaring arguments of one.

use turborel::db::{Database, OpenOptions};
use turborel::error::{Code, Result, fail};
use turborel::functions::ScalarFunction;
use turborel::value::{DataType, Decimal, Spelled, SqlType, Value};

use crate::common::{Counted, Scratch, exec, os, rows, sorted};
use crate::ident::{IDENT, Ident, ident_type};

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

/// A kind of the name it is given, as IDENT in all else but that it takes
/// no character string by store assignment, as `SqlType::assigns_text`
/// says by default.
#[derive(Debug)]
struct Named(&'static str);

impl SqlType for Named {
    fn name(&self) -> &'static str {
        self.0
    }
    fn written(&self, t: &DataType) -> String {
        IDENT.written(t)
    }
    fn comparable(&self, other: &DataType) -> bool {
        IDENT.comparable(other)
    }
    fn compare(&self, a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
        IDENT.compare(a, b)
    }
    fn bucket(&self, v: &Value) -> Option<turborel::value::Bucket> {
        IDENT.bucket(v)
    }
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        IDENT.assign(v, t, rule)
    }
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
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
    fn descriptor(&self, t: &DataType) -> turborel::value::Descriptor {
        IDENT.descriptor(t)
    }
    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        IDENT.put_value(v, out);
    }
    fn value(&self, r: &mut turborel::bytes::Reader<'_>) -> Result<Value> {
        IDENT.value(r)
    }
    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        IDENT.put_params(t, out);
    }
    fn params(&self, r: &mut turborel::bytes::Reader<'_>) -> Result<DataType> {
        IDENT.params(r)
    }
    fn record_width(&self, t: &DataType) -> Option<usize> {
        IDENT.record_width(t)
    }
    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        IDENT.encode_record(v, t)
    }
    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        IDENT.decode_record(field, t)
    }
    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        IDENT.encode_key(v, t, out)
    }
    fn decode_key(&self, rest: &mut &[u8], t: &DataType) -> Result<Value> {
        IDENT.decode_key(rest, t)
    }
    fn anchor(&self, v: &Value, t: &DataType) -> Option<(Value, bool)> {
        IDENT.anchor(v, t)
    }
}

/// Verifies: [EXTENSION] RULES.md D53: a type is registered under a regular
/// identifier in upper case, no key word, once.
#[test]
fn a_type_is_registered_once_by_its_name() {
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

/// Verifies: [EXTENSION] RULES.md D53; [ISO/IEC 9075:1992 §9.2 SR2,
/// §13.8 SR6, §13.10 SR5] a character string is stored in a type beyond
/// the standard whose kind takes one, by INSERT, INSERT from a query and
/// UPDATE, as CAST makes it, and refused as CAST refuses it; not in one
/// whose kind takes none; a number is stored in neither, nor a value of
/// one in a number, each refused as the statement is bound. Regression
/// test (R-TEST-7): those two were refused only as a row was stored
/// (SQLSTATE 22005).
#[test]
fn a_character_string_is_stored_in_a_plug_in_type_that_takes_one() {
    let db = identified();
    db.register_type(Box::new(Named("OPAQUE"))).unwrap();
    let a = db.session("A");
    a.run(&format!("INSERT INTO T VALUES ('{}', 1)", hex(1))).unwrap();
    a.run(&format!("UPDATE T SET K = '{}' WHERE N = 1", hex(2))).unwrap();
    a.run(&format!("INSERT INTO T SELECT '{}', 3 FROM T", hex(3))).unwrap();
    assert_eq!(rows(&a, "SELECT K FROM T ORDER BY K").unwrap(), [hex(2), hex(3)]);
    assert_eq!(rows(&a, &format!("SELECT N FROM T WHERE K = {}", cast(2))).unwrap(), ["1"], "as CAST makes it");
    assert_eq!(exec(&a, "INSERT INTO T VALUES ('XYZ', 5)").err(), Some(Code::Data), "not an identifier");
    assert_eq!(exec(&a, "INSERT INTO T VALUES (5, 5)").err(), Some(Code::SyntaxRule), "a number");
    assert_eq!(exec(&a, "INSERT INTO T (N) SELECT K FROM T").err(), Some(Code::SyntaxRule), "an IDENT in a number");
    a.run("CREATE TABLE O (K OPAQUE)").unwrap();
    assert_eq!(exec(&a, &format!("INSERT INTO O VALUES ('{}')", hex(1))).err(), Some(Code::SyntaxRule), "OPAQUE takes none");
    a.commit().unwrap();
}

/// NUMERIC(10, 0), the type IDENT_BYTE declares its second argument.
const POSITION: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };

/// IDENT_BYTE(id, n): byte n, from 1, of the identifier id, declaring its
/// arguments an IDENT and a POSITION.
struct IdentByte;

impl ScalarFunction for IdentByte {
    fn name(&self) -> &'static str {
        "IDENT_BYTE"
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if args == [ident_type(), POSITION] {
            return Ok(POSITION);
        }
        fail(Code::SyntaxRule, "IDENT_BYTE is given an IDENT and a position, as it declares")
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [Value::Extension(id), Value::Exact(n)] = args else { return Ok(Value::Null) };
        let at = usize::try_from(n.units).ok().and_then(|n| n.checked_sub(1));
        Ok(at.and_then(|at| id.bytes.get(at)).map_or(Value::Null, |b| Value::Exact(Decimal::new(i128::from(*b), 0))))
    }
    fn argument_type(&self, position: usize, _args: &[Option<DataType>]) -> Option<DataType> {
        [ident_type(), POSITION].get(position).copied()
    }
}

/// Verifies: [EXTENSION] RULES.md D53, R-SAFE-3; [ISO/IEC 9075:1992 §9.2
/// SR2, §17.6 GR5] a function declaring its arguments' types is told
/// them, and given each argument stored in its type: a character string
/// in a type beyond the standard that takes one, an exact number of a
/// scale in a NUMERIC of none; a `?` takes the type declared; an argument
/// not assignable is refused as the statement is bound, one CAST refuses
/// as it runs.
#[test]
fn a_function_declares_its_arguments_types() {
    let db = identified();
    db.register_scalar_function(Box::new(IdentByte)).unwrap();
    let mut a = db.session("A");
    a.enable_extensions(true);
    a.run(&format!("INSERT INTO T VALUES ({}, 1)", cast(7))).unwrap();
    assert_eq!(rows(&a, "SELECT IDENT_BYTE(K, 16) FROM T").unwrap(), ["7"], "of a column");
    let given = format!("'0A{}'", "0".repeat(30));
    assert_eq!(rows(&a, &format!("VALUES IDENT_BYTE({given}, 1.0)")).unwrap(), ["10"], "a string and a number assigned");
    assert_eq!(crate::common::code(&a, "VALUES IDENT_BYTE(5, 1)"), Code::SyntaxRule, "a number is no IDENT");
    assert_eq!(crate::common::code(&a, "SELECT IDENT_BYTE(K, 'X') FROM T"), Code::SyntaxRule, "nor a string a position");
    assert_eq!(rows(&a, "VALUES IDENT_BYTE('XYZ', 1)").err(), Some(Code::Data), "not an identifier");
    let p = a.prepare("VALUES IDENT_BYTE(?, ?)").unwrap();
    let described: Vec<DataType> = p.parameters().iter().map(|t| t.data_type()).collect();
    assert_eq!(described, [ident_type(), POSITION]);
    let mut opened = a.open_prepared(&p, &[Value::Char(hex(9)), Value::Exact(Decimal::new(16, 0))]).unwrap();
    assert_eq!(opened.fetch().unwrap().map(|r| r[0].to_string()), Some("9".to_owned()));
}
