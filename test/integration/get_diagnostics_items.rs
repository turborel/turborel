//! What ISO/IEC 9075-2:2023 §23.1 adds to GET DIAGNOSTICS: targets of
//! another type than their items' (Feature F122), ALL (Feature F123), the
//! transaction counts (Feature T511), the statement codes, CONDITION in
//! EXCEPTION's place, and the items of routines and triggers.

use turborel::db::{Completion, Database, Session};
use turborel::error::{Code, SqlState};
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

use crate::common::{dynamic_host, number, run_with as run, set_text as text, setup};

/// Every `<statement information item name>`, and whether it is a number
/// [§23.1 Table 37].
const STATEMENT_ITEMS: [(&str, bool); 10] = [
    ("NUMBER", true),
    ("MORE", false),
    ("COMMAND_FUNCTION", false),
    ("COMMAND_FUNCTION_CODE", true),
    ("DYNAMIC_FUNCTION", false),
    ("DYNAMIC_FUNCTION_CODE", true),
    ("ROW_COUNT", true),
    ("TRANSACTIONS_COMMITTED", true),
    ("TRANSACTIONS_ROLLED_BACK", true),
    ("TRANSACTION_ACTIVE", true),
];

/// Every `<condition information item name>` but CONDITION_NUMBER, and
/// whether it is a number [§23.1 Table 38].
const CONDITION_ITEMS: [(&str, bool); 26] = [
    ("CATALOG_NAME", false),
    ("CLASS_ORIGIN", false),
    ("COLUMN_NAME", false),
    ("CONNECTION_NAME", false),
    ("CONSTRAINT_CATALOG", false),
    ("CONSTRAINT_NAME", false),
    ("CONSTRAINT_SCHEMA", false),
    ("CURSOR_NAME", false),
    ("MESSAGE_LENGTH", true),
    ("MESSAGE_OCTET_LENGTH", true),
    ("MESSAGE_TEXT", false),
    ("PARAMETER_MODE", false),
    ("PARAMETER_NAME", false),
    ("PARAMETER_ORDINAL_POSITION", true),
    ("RETURNED_SQLSTATE", false),
    ("ROUTINE_CATALOG", false),
    ("ROUTINE_NAME", false),
    ("ROUTINE_SCHEMA", false),
    ("SCHEMA_NAME", false),
    ("SERVER_NAME", false),
    ("SPECIFIC_NAME", false),
    ("SUBCLASS_ORIGIN", false),
    ("TABLE_NAME", false),
    ("TRIGGER_CATALOG", false),
    ("TRIGGER_NAME", false),
    ("TRIGGER_SCHEMA", false),
];

fn exact(precision: u32, scale: u32) -> DataType {
    DataType::Exact { precision, scale, spelled: Spelled::Numeric }
}

/// Targets: NUM, NUMERIC(9); TXT, CHARACTER VARYING(32767); and J, 1, a
/// condition number.
fn host() -> Params {
    Params::new().declare("NUM", exact(9, 0), Value::Null).declare("TXT", DataType::varying(32_767), Value::Null).declare(
        "J",
        exact(9, 0),
        Value::Exact(Decimal::new(1, 0)),
    )
}

/// Schema A: T, keyed by K, which CHECK POSITIVE keeps above 0, holding
/// 1, 2 and 3, committed.
fn diagnosed() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, N CHAR(2), CONSTRAINT POSITIVE CHECK (K > 0))")
        .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 'A'), (2, 'B'), (3, 'C')").unwrap();
    a.commit().unwrap();
    db
}

/// The item `item`, a number if `numeric`, of the statement information
/// or, of `condition`, of that condition, as text.
fn item(s: &Session, (item, numeric): (&str, bool), condition: Option<u32>) -> String {
    let mut p = host();
    let target = if numeric { "NUM" } else { "TXT" };
    let of = condition.map_or(String::new(), |n| format!("CONDITION {n} "));
    s.execute(&format!("GET DIAGNOSTICS {of}{target} = {item}"), &mut p).unwrap();
    p.value(target).to_string()
}

/// The statement information item `name`, as text.
fn statement(s: &Session, name: &str) -> String {
    let numeric = STATEMENT_ITEMS.iter().find(|(n, _)| *n == name).unwrap().1;
    item(s, (name, numeric), None)
}

/// What GET DIAGNOSTICS TXT = ALL `qualifier` assigns.
fn all(s: &Session, qualifier: &str) -> String {
    let mut p = host();
    s.execute(&format!("GET DIAGNOSTICS TXT = ALL {qualifier}"), &mut p).unwrap();
    p.value("TXT").to_string()
}

/// [§23.1 GR6a] The presentation of each of `items`, each read alone, of
/// the statement or of `condition`, `X = value`, an item of a zero-length
/// value, or one of spaces, left out.
fn presented(s: &Session, items: &[(&str, bool)], condition: Option<u32>) -> Vec<String> {
    items
        .iter()
        .map(|&(name, numeric)| (name, item(s, (name, numeric), condition)))
        .filter(|(_, value)| !value.trim_end().is_empty())
        .map(|(name, value)| format!("{name} = {value}"))
        .collect()
}

/// The three transaction counts of the statement before.
fn counts(s: &Session) -> [String; 3] {
    ["TRANSACTIONS_COMMITTED", "TRANSACTIONS_ROLLED_BACK", "TRANSACTION_ACTIVE"].map(|name| statement(s, name))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1 SR1, CR3, §9.1 SR3,
/// SR5] Feature F122: an item goes to a target of any type its value may
/// be assigned to, a number to SMALLINT, DECIMAL with a scale, DOUBLE
/// PRECISION or DECFLOAT, a character string to CHARACTER or CHARACTER
/// VARYING of any length, as retrieval assignment assigns it [§9.1 GR]: a
/// string cut to fit is the warning 01004, a number too large for its
/// target SQLSTATE 22003. A number to a character string, a character
/// string to a number, an item to a datetime, BOOLEAN or binary string
/// target are SQLCODE -103.
#[test]
fn targets_of_another_type_f122_23_1() {
    let db = diagnosed();
    let a = db.session("A");
    let mut p = host()
        .declare("SI", DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Smallint }, Value::Null)
        .declare("SCALED", exact(9, 2), Value::Null)
        .declare("DBL", DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision }, Value::Null)
        .declare("DF", DataType::DecFloat { precision: 34 }, Value::Null)
        .declare("ONE", exact(1, 0), Value::Null)
        .declare("CH3", DataType::character(3), Value::Null)
        .declare("VC", DataType::varying(10), Value::Null)
        .declare("D", DataType::Date, Value::Null)
        .declare("B", DataType::Boolean, Value::Null)
        .declare("BIN", DataType::VarBinary { length: 10 }, Value::Null);
    a.run("INSERT INTO T VALUES (4, 'D'), (5, 'E'), (6, 'F')").unwrap();
    let done = a.execute("GET DIAGNOSTICS SI = ROW_COUNT, SCALED = NUMBER, DBL = COMMAND_FUNCTION_CODE, DF = TRANSACTION_ACTIVE", &mut p);
    assert_eq!(done.unwrap(), Completion::Success);
    let read = ["SI", "SCALED", "DF"].map(|t| p.value(t).to_string());
    assert_eq!(read, ["3", "1.00", "1"]);
    assert!(matches!(p.value("DBL"), Value::Approx(x) if (x - 50.0_f64).abs() < f64::EPSILON), "{:?}", p.value("DBL"));
    assert_eq!(a.execute("GET DIAGNOSTICS CH3 = MORE, VC = COMMAND_FUNCTION", &mut p).unwrap(), Completion::Success);
    assert_eq!([p.value("CH3").to_string(), p.value("VC").to_string()], ["N  ", "INSERT"], "padded; as it is");
    let cut = a.execute("GET DIAGNOSTICS CONDITION 1 DBL = MESSAGE_LENGTH, CH3 = RETURNED_SQLSTATE", &mut p).unwrap();
    assert_eq!((cut, p.value("CH3").to_string()), (Completion::Warning(SqlState::STRING_TRUNCATED), "000".to_owned()), "cut");
    assert_eq!(statement(&a, "COMMAND_FUNCTION"), "INSERT", "the area as it was");
    a.run("INSERT INTO T VALUES (7, 'G'), (8, 'H'), (9, 'I'), (10, 'J')").unwrap();
    a.run("UPDATE T SET N = 'Z'").unwrap();
    let large = a.execute("GET DIAGNOSTICS ONE = ROW_COUNT", &mut p).err().map(|e| e.sqlstate());
    assert_eq!(large, Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE), "10 rows in NUMERIC(1)");
    for sql in [
        "GET DIAGNOSTICS NUM = MORE",
        "GET DIAGNOSTICS VC = NUMBER",
        "GET DIAGNOSTICS D = ROW_COUNT",
        "GET DIAGNOSTICS B = TRANSACTION_ACTIVE",
        "GET DIAGNOSTICS BIN = COMMAND_FUNCTION",
        "GET DIAGNOSTICS CONDITION 1 D = MESSAGE_TEXT",
        "GET DIAGNOSTICS CONDITION 1 VC = MESSAGE_OCTET_LENGTH",
    ] {
        assert_eq!(a.execute(sql, &mut p).err().map(|e| e.code), Some(Code::SyntaxRule), "{sql}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1 GR2c, GR2d, GR2e,
/// Table 39] COMMAND_FUNCTION_CODE names the statement by its code and
/// DYNAMIC_FUNCTION_CODE the statement prepared by its: 0 before any, of
/// a statement that did not parse, and of one not prepared; a code tells
/// apart the statements one identifier names, the SELECTs of one row, of
/// a cursor read whole and prepared, and the positioned UPDATE of a
/// dynamic cursor from one prepared; this implementation's statements
/// have negative codes (IV212).
#[test]
fn statement_codes_23_1() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    let codes = |s: &Session| {
        ["COMMAND_FUNCTION", "COMMAND_FUNCTION_CODE", "DYNAMIC_FUNCTION", "DYNAMIC_FUNCTION_CODE"].map(|name| statement(s, name))
    };
    let named = |cf: &str, cfc: i32, df: &str, dfc: i32| [cf.to_owned(), cfc.to_string(), df.to_owned(), dfc.to_string()];
    assert_eq!(codes(&s), named("", 0, "", 0), "before any");
    let cases: [(&str, [String; 4]); 9] = [
        ("INSERT INTO T VALUES (7, 'G', 1, 1E0)", named("INSERT", 50, "", 0)),
        ("UPDATE T SET AMT = 2 WHERE ID = 7", named("UPDATE WHERE", 82, "", 0)),
        ("SELECT ID INTO N FROM T WHERE ID = 7", named("SELECT", 65, "", 0)),
        ("DELETE FROM T WHERE ID = 7", named("DELETE WHERE", 19, "", 0)),
        ("MERGE INTO T USING U ON T.ID = U.ID WHEN MATCHED THEN UPDATE SET AMT = 1", named("MERGE", 128, "", 0)),
        ("EXECUTE IMMEDIATE 'DELETE FROM T WHERE ID = 3'", named("EXECUTE IMMEDIATE", 43, "DELETE WHERE", 19)),
        ("COMMIT", named("COMMIT WORK", 11, "", 0)),
        ("DECLARE LOCAL TEMPORARY TABLE MODULE.M (X INTEGER)", named("DECLARE LOCAL TEMPORARY TABLE", -1, "", 0)),
        ("NOT A STATEMENT", named("", 0, "", 0)),
    ];
    for (sql, expected) in cases {
        drop(s.execute(sql, &mut p));
        assert_eq!(codes(&s), expected, "{sql}");
    }
    s.run("CREATE INDEX T_F ON T (F)").unwrap();
    assert_eq!(codes(&s), named("CREATE INDEX", -5, "", 0));
    drop(s.open_cursor("SELECT ID FROM T").unwrap());
    assert_eq!(codes(&s), named("SELECT", 21, "", 0), "a <direct select statement: multiple rows>");
    s.run("ROLLBACK").unwrap();
    assert_eq!(codes(&s), named("ROLLBACK WORK", 62, "", 0));
    text(&mut p, "SELECT ID FROM T WHERE ID = ?");
    run(&s, "PREPARE Q FROM TEXT", &mut p).unwrap();
    assert_eq!(codes(&s), named("PREPARE", 56, "", 0));
    run(&s, "EXECUTE Q INTO N USING K", &mut p).unwrap();
    assert_eq!(codes(&s), named("EXECUTE", 44, "SELECT", 41), "a <dynamic single row select statement>");
    let api = s.prepare("SELECT ID FROM T").unwrap();
    drop(s.open_prepared(&api, &[]).unwrap());
    assert_eq!(codes(&s), named("DYNAMIC OPEN", 40, "SELECT CURSOR", 85), "a <dynamic select statement>");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1 GR2d, GR2e, Table 39]
/// a positioned UPDATE or DELETE of a dynamic cursor is a `<dynamic
/// update statement: positioned>` or a `<dynamic delete statement:
/// positioned>`, codes 42 and 38; prepared and executed, a `<preparable
/// dynamic update statement: positioned>` or a `<preparable dynamic delete
/// statement: positioned>`, codes 55 and 54, of the identifiers SQL-92
/// gives both [ISO/IEC 9075:1992 §18.1 Table 22].
#[test]
fn positioned_statements_of_dynamic_cursors_coded_23_1() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    let codes = |s: &Session| ["COMMAND_FUNCTION_CODE", "DYNAMIC_FUNCTION", "DYNAMIC_FUNCTION_CODE"].map(|name| statement(s, name));
    p.set("SN", Value::Char("Q".to_owned()));
    p.set("CN", Value::Char("CUR".to_owned()));
    text(&mut p, "SELECT ID, AMT FROM T FOR UPDATE OF AMT");
    run(&s, "PREPARE SN FROM TEXT", &mut p).unwrap();
    run(&s, "ALLOCATE CN CURSOR FOR SN", &mut p).unwrap();
    text(&mut p, "UPDATE SET AMT = 5 WHERE CURRENT OF CUR");
    run(&s, "PREPARE U FROM TEXT", &mut p).unwrap();
    text(&mut p, "DELETE WHERE CURRENT OF CUR");
    run(&s, "PREPARE D FROM TEXT", &mut p).unwrap();
    run(&s, "OPEN CN", &mut p).unwrap();
    run(&s, "FETCH CN INTO ID, N", &mut p).unwrap();
    run(&s, "UPDATE T SET AMT = 9 WHERE CURRENT OF CN", &mut p).unwrap();
    assert_eq!(codes(&s), ["42", "", "0"]);
    run(&s, "EXECUTE U", &mut p).unwrap();
    assert_eq!(codes(&s), ["44", "DYNAMIC UPDATE CURSOR", "55"]);
    run(&s, "EXECUTE D", &mut p).unwrap();
    assert_eq!(codes(&s), ["44", "DYNAMIC DELETE CURSOR", "54"]);
    run(&s, "FETCH CN INTO ID, N", &mut p).unwrap();
    run(&s, "DELETE FROM T WHERE CURRENT OF CN", &mut p).unwrap();
    assert_eq!(codes(&s), ["38", "", "0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1 GR2h, GR2i, GR2j, CR2]
/// Feature T511: TRANSACTIONS_COMMITTED and TRANSACTIONS_ROLLED_BACK
/// count the transactions the statement before committed and rolled back,
/// 0 of a statement that ends none, a COMMIT or ROLLBACK of none among
/// them; a transaction that read alone, or changed definitions, commits;
/// one whose COMMIT fails on a deferred constraint is rolled back, and so
/// counted, as is one an engine refuses to commit (40001); through the
/// Rust API and EXECUTE IMMEDIATE too.
/// TRANSACTION_ACTIVE is 1 while one is active, a failed statement
/// leaving it so, and 0 before any.
#[test]
fn transaction_counts_t511_23_1() {
    let db = diagnosed();
    let a = db.session("A");
    let counted = |c: u8, r: u8, active: u8| [c.to_string(), r.to_string(), active.to_string()];
    assert_eq!(counts(&a), counted(0, 0, 0), "before any");
    let cases: [(&str, [String; 3]); 13] = [
        ("INSERT INTO T VALUES (4, 'D')", counted(0, 0, 1)),
        ("COMMIT", counted(1, 0, 0)),
        ("COMMIT", counted(0, 0, 0)),
        ("ROLLBACK", counted(0, 0, 0)),
        ("SELECT K INTO NUM FROM T WHERE K = 4", counted(0, 0, 1)),
        ("COMMIT", counted(1, 0, 0)),
        ("INSERT INTO T VALUES (-5, 'E')", counted(0, 0, 1)),
        ("ROLLBACK WORK", counted(0, 1, 0)),
        ("CREATE TABLE LATER (K INTEGER, CONSTRAINT AFTER CHECK (K > 0) DEFERRABLE INITIALLY DEFERRED)", counted(0, 0, 1)),
        ("COMMIT WORK", counted(1, 0, 0)),
        ("INSERT INTO LATER VALUES (-1)", counted(0, 0, 1)),
        ("COMMIT", counted(0, 1, 0)),
        ("EXECUTE IMMEDIATE 'COMMIT'", counted(0, 0, 0)),
    ];
    let mut p = host();
    for (sql, expected) in cases {
        drop(a.execute(sql, &mut p));
        assert_eq!(counts(&a), expected, "{sql}");
    }
    a.run("DELETE FROM T WHERE K = 4").unwrap();
    a.execute("EXECUTE IMMEDIATE 'COMMIT'", &mut p).unwrap();
    assert_eq!(counts(&a), counted(1, 0, 0), "EXECUTE IMMEDIATE");
    a.run("DELETE FROM T WHERE K = 3").unwrap();
    a.commit().unwrap();
    assert_eq!(counts(&a), counted(1, 0, 0), "the Rust API");
    a.run("DELETE FROM T WHERE K = 2").unwrap();
    a.rollback().unwrap();
    assert_eq!(counts(&a), counted(0, 1, 0));
    // A commit an engine refuses, the transaction rolled back (40001).
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION F CREATE TABLE P (K INTEGER PRIMARY KEY) CREATE TABLE R (K INTEGER REFERENCES P)").unwrap();
    let (f, g) = (db.session("F"), db.session("F"));
    f.run("INSERT INTO P VALUES (2)").unwrap();
    f.commit().unwrap();
    f.run("DELETE FROM P WHERE K = 2").unwrap();
    g.run("INSERT INTO R VALUES (2)").unwrap();
    f.commit().unwrap();
    assert_eq!(g.commit().err().map(|e| e.sqlstate()), Some(SqlState::SERIALIZATION_FAILURE));
    assert_eq!(counts(&g), counted(0, 1, 0), "refused by an engine");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1 SR2, GR6a, CR4]
/// Feature F123: ALL assigns each statement item, then each condition's
/// CONDITION_NUMBER and items, as `X = value` joined by semicolons, an
/// item whose value is a zero-length string left out: before any
/// statement, the statement items alone; after one, its condition too,
/// the same whatever the condition, as each item read alone gives it.
#[test]
fn all_information_f123_23_1() {
    let db = diagnosed();
    let a = db.session("A");
    let none = "NUMBER = 0;MORE = N;COMMAND_FUNCTION_CODE = 0;DYNAMIC_FUNCTION_CODE = 0;ROW_COUNT = 0;\
                TRANSACTIONS_COMMITTED = 0;TRANSACTIONS_ROLLED_BACK = 0;TRANSACTION_ACTIVE = 0";
    assert_eq!((all(&a, ""), all(&a, "STATEMENT"), all(&a, "CONDITION")), (none.to_owned(), none.to_owned(), String::new()));
    a.run("INSERT INTO T VALUES (4, 'D'), (5, 'E')").unwrap();
    let statement = "NUMBER = 1;MORE = N;COMMAND_FUNCTION = INSERT;COMMAND_FUNCTION_CODE = 50;DYNAMIC_FUNCTION_CODE = 0;\
                     ROW_COUNT = 2;TRANSACTIONS_COMMITTED = 0;TRANSACTIONS_ROLLED_BACK = 0;TRANSACTION_ACTIVE = 1";
    let condition = "CLASS_ORIGIN = ISO 9075;CONNECTION_NAME = DEFAULT;MESSAGE_LENGTH = 0;MESSAGE_OCTET_LENGTH = 0;\
                     PARAMETER_ORDINAL_POSITION = 0;RETURNED_SQLSTATE = 00000;SERVER_NAME = DEFAULT;SUBCLASS_ORIGIN = ISO 9075";
    assert_eq!(all(&a, ""), format!("{statement};CONDITION_NUMBER = 1;{condition}"));
    assert_eq!(all(&a, "STATEMENT"), statement);
    assert_eq!(all(&a, "CONDITION"), format!("CONDITION_NUMBER = 1;{condition}"));
    assert_eq!((all(&a, "CONDITION 1"), all(&a, "CONDITION J")), (condition.to_owned(), condition.to_owned()));
    let mut p = host();
    for sql in [
        "INSERT INTO T VALUES (-1, 'Z')",
        "UPDATE T SET N = 'TOO LONG'",
        "SELECT NOPE FROM T",
        "INSERT INTO T VALUES (1, 'A')",
        "SELECT K INTO NUM FROM T WHERE K = 99",
        "COMMIT",
    ] {
        drop(a.execute(sql, &mut p));
        let mut items = presented(&a, &STATEMENT_ITEMS, None);
        items.push("CONDITION_NUMBER = 1".to_owned());
        items.extend(presented(&a, &CONDITION_ITEMS, Some(1)));
        assert_eq!(all(&a, ""), items.join(";"), "{sql}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1 SR2, SR3, GR3, GR6a
/// iv] ALL assigns its string as a FETCH would [§9.1]: cut to fit a
/// shorter target with the warning 01004, padded in a fixed-length one,
/// the area left as it was. Its target not a character string is SQLCODE
/// -103, as is a condition number not exact with scale 0; one of no
/// condition is SQLSTATE 35000, before any statement too.
#[test]
fn all_information_assigned_and_refused_23_1() {
    let db = diagnosed();
    let a = db.session("A");
    let mut p = host().declare("SHORT", DataType::character(10), Value::Null).declare("LONG", DataType::character(400), Value::Null);
    let state = |r: Result<Completion, turborel::error::SqlError>| r.err().map(|e| (e.code, e.sqlstate().as_str().to_owned()));
    assert_eq!(state(a.execute("GET DIAGNOSTICS TXT = ALL CONDITION 1", &mut p)), Some((Code::Data, "35000".to_owned())), "none yet");
    a.run("INSERT INTO T VALUES (4, 'D')").unwrap();
    let done = a.execute("GET DIAGNOSTICS SHORT = ALL", &mut p).unwrap();
    assert_eq!((done, p.value("SHORT").to_string()), (Completion::Warning(SqlState::STRING_TRUNCATED), "NUMBER = 1".to_owned()));
    a.execute("GET DIAGNOSTICS LONG = ALL STATEMENT", &mut p).unwrap();
    let long = p.value("LONG").to_string();
    assert_eq!((long.len(), long.trim_end()), (400, all(&a, "STATEMENT").as_str()), "padded");
    assert_eq!(statement(&a, "COMMAND_FUNCTION"), "INSERT", "the area as it was");
    for (sql, code) in [
        ("GET DIAGNOSTICS NUM = ALL", Some((Code::SyntaxRule, "2A000".to_owned()))),
        ("GET DIAGNOSTICS TXT = ALL CONDITION 1.5", Some((Code::SyntaxRule, "2A000".to_owned()))),
        ("GET DIAGNOSTICS TXT = ALL CONDITION 2", Some((Code::Data, "35000".to_owned()))),
        ("GET DIAGNOSTICS TXT = ALL CONDITION 0", Some((Code::Data, "35000".to_owned()))),
    ] {
        assert_eq!(state(a.execute(sql, &mut p)), code, "{sql}");
    }
    p.set("J", number(3));
    assert_eq!(state(a.execute("GET DIAGNOSTICS TXT = ALL CONDITION J", &mut p)), Some((Code::Data, "35000".to_owned())));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1 GR4l, GR4q, GR5]
/// CONDITION reads a condition as EXCEPTION does; the items of a routine,
/// its parameters and a trigger are zero-length strings, and
/// PARAMETER_ORDINAL_POSITION 0, as no condition is raised in an
/// SQL-invoked routine or a trigger.
#[test]
fn condition_and_the_items_of_routines_and_triggers_23_1() {
    let db = diagnosed();
    let a = db.session("A");
    assert!(a.run("INSERT INTO T VALUES (-1, 'Z')").is_err());
    let mut p = host().declare("SQLSTATE5", DataType::character(5), Value::Null);
    a.execute("GET DIAGNOSTICS CONDITION 1 SQLSTATE5 = RETURNED_SQLSTATE, TXT = CONSTRAINT_NAME", &mut p).unwrap();
    assert_eq!([p.value("SQLSTATE5").to_string(), p.value("TXT").to_string()], ["23000", "POSITIVE"]);
    for name in [
        "PARAMETER_MODE",
        "PARAMETER_NAME",
        "ROUTINE_CATALOG",
        "ROUTINE_NAME",
        "ROUTINE_SCHEMA",
        "SPECIFIC_NAME",
        "TRIGGER_CATALOG",
        "TRIGGER_NAME",
        "TRIGGER_SCHEMA",
    ] {
        assert_eq!(item(&a, (name, false), Some(1)), "", "{name}");
    }
    assert_eq!(item(&a, ("PARAMETER_ORDINAL_POSITION", true), Some(1)), "0");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1] the examples of the
/// manual's page of GET DIAGNOSTICS (`doc/sql/statements/get-diagnostics.md`)
/// do as it prints, and each statement it refuses is refused with the
/// message it prints, as `SQLCODE (SQLSTATE): message`.
#[test]
fn the_manual_examples_23_1() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(30), CONSTRAINT PAID CHECK (ID > 0))",
    )
    .unwrap();
    let s = db.session("ANN");
    let mut p = Params::new()
        .declare("N", exact(9, 0), Value::Null)
        .declare("CMD", DataType::varying(128), Value::Null)
        .declare("STATE", DataType::character(5), Value::Null)
        .declare("NAME", DataType::varying(128), Value::Null)
        .declare("REPORT", DataType::varying(1000), Value::Null);
    let value = |p: &Params, name: &str| p.value(name).to_string();
    s.run("INSERT INTO STAFF VALUES (1, 'Ada'), (2, 'Grace')").unwrap();
    s.execute("GET DIAGNOSTICS N = ROW_COUNT, CMD = COMMAND_FUNCTION", &mut p).unwrap();
    assert_eq!([value(&p, "N"), value(&p, "CMD")], ["2", "INSERT"]);
    s.execute("GET DIAGNOSTICS REPORT = ALL STATEMENT", &mut p).unwrap();
    let report = "NUMBER = 1;MORE = N;COMMAND_FUNCTION = INSERT;COMMAND_FUNCTION_CODE = 50;DYNAMIC_FUNCTION_CODE = 0;\
                  ROW_COUNT = 2;TRANSACTIONS_COMMITTED = 0;TRANSACTIONS_ROLLED_BACK = 0;TRANSACTION_ACTIVE = 1";
    assert_eq!(value(&p, "REPORT"), report);
    assert!(s.run("INSERT INTO STAFF VALUES (-1, 'Edsger')").is_err());
    s.execute("GET DIAGNOSTICS CONDITION 1 STATE = RETURNED_SQLSTATE, NAME = CONSTRAINT_NAME", &mut p).unwrap();
    assert_eq!([value(&p, "STATE"), value(&p, "NAME")], ["23000", "PAID"]);
    s.commit().unwrap();
    s.execute("GET DIAGNOSTICS N = TRANSACTIONS_COMMITTED", &mut p).unwrap();
    assert_eq!(value(&p, "N"), "1");
    let refused = |sql: &str, p: &mut Params| {
        let e = s.execute(sql, p).expect_err(sql);
        format!("{} ({}): {}", e.sqlcode(), e.sqlstate(), e.described())
    };
    assert!(s.run("INSERT INTO STAFF VALUES (-1, 'Edsger')").is_err());
    for (sql, message) in [
        ("GET DIAGNOSTICS X = NUMBER", "-103 (2A000): parameter X is not declared [X3.135-1986 §7.3 SR3]"),
        ("GET DIAGNOSTICS N = NOPE", "-102 (2A000): expected a <statement information item name>, found NOPE"),
        (
            "GET DIAGNOSTICS N = MORE",
            "-103 (2A000): a CHARACTER VARYING(32767) item cannot be assigned to N (NUMERIC(9,0)) [ISO/IEC 9075-2:2023 §23.1 SR1, §9.1 SR3]",
        ),
        (
            "GET DIAGNOSTICS N = ALL",
            "-103 (2A000): target N of ALL is NUMERIC(9,0), not a character string [ISO/IEC 9075-2:2023 §23.1 SR2]",
        ),
        (
            "GET DIAGNOSTICS CONDITION 5 STATE = RETURNED_SQLSTATE",
            "-301 (35000): there is no condition 5: the area holds 1 [ISO/IEC 9075:1992 §18.1 GR2]",
        ),
    ] {
        assert_eq!(refused(sql, &mut p), message, "{sql}");
    }
    p = p.declare("ONE", exact(1, 0), Value::Null);
    s.run("INSERT INTO STAFF VALUES (3, 'C'), (4, 'D'), (5, 'E'), (6, 'F'), (7, 'G'), (8, 'H'), (9, 'I'), (10, 'J')").unwrap();
    s.run("UPDATE STAFF SET NAME = 'X'").unwrap();
    let e = s.execute("GET DIAGNOSTICS ONE = ROW_COUNT", &mut p).err();
    assert_eq!(e.map(|e| (e.sqlcode(), e.sqlstate())), Some((-301, SqlState::NUMERIC_VALUE_OUT_OF_RANGE)), "the ERRORS table's 22003");
}
