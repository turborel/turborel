//! SQL/PSM compound statements, variables and the control statements in a
//! procedure's body [ISO/IEC 9075-4:2006 draft §14.1, §14.4 to §14.13]
//! (Feature P002).

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

/// Schema A: T (K, N).
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
    db
}

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// An INTEGER host parameter's type.
fn integer() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.1 GR2, GR3, §14.4 GR1, §14.5 GR1, §14.11]
/// A compound statement creates its variables, of their defaults, and runs
/// its statements in turn: a WHILE loop sums and inserts.
#[test]
fn a_loop_sums_into_a_variable_14_1() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "CREATE PROCEDURE FILL (IN UPTO INTEGER, OUT TOTAL INTEGER) MODIFIES SQL DATA \
         BEGIN DECLARE I INTEGER DEFAULT 1; SET TOTAL = 0; \
         WHILE I <= UPTO DO INSERT INTO T VALUES (I, I * 10); SET TOTAL = TOTAL + I; SET I = I + 1; END WHILE; END",
    )
    .unwrap();
    let mut p = Params::new().declare("V", integer(), Value::Null);
    a.execute("CALL FILL(4, V)", &mut p).unwrap();
    assert_eq!(p.get("V").map(|p| p.value.to_string()), Some(Value::Exact(Decimal::new(10, 0)).to_string()));
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["1,10", "2,20", "3,30", "4,40"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.2 GR3b i, ii, §17.2]
/// A CONTINUE handler's action runs and the statement after the one that
/// raised the condition follows; an EXIT handler's leaves its compound
/// statement; SIGNAL raises a condition of its SQLSTATE value.
#[test]
fn handlers_continue_and_exit_14_2() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "CREATE PROCEDURE H (OUT R INTEGER) MODIFIES SQL DATA \
         BEGIN DECLARE CONTINUE HANDLER FOR SQLSTATE '22012' SET R = R + 100; SET R = 1; \
         INSERT INTO T VALUES (1, 1 / 0); SET R = R + 1; \
         B: BEGIN DECLARE EXIT HANDLER FOR SQLEXCEPTION SET R = R + 1000; SIGNAL SQLSTATE '75001'; SET R = 0; END B; END",
    )
    .unwrap();
    let mut p = Params::new().declare("V", integer(), Value::Null);
    a.execute("CALL H(V)", &mut p).unwrap();
    assert_eq!(p.get("V").map(|p| p.value.to_string()), Some("1102".to_owned()));
    assert_eq!(rows(&a, "SELECT K FROM T"), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §12.1 to §12.3, §14.13]
/// A local cursor fetches into variables and a positioned UPDATE changes
/// the row it is on; FOR runs its statements for each row, its columns
/// variables of its loop variable name.
#[test]
fn cursors_and_for_14_13() {
    let db = defined();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20), (3, 30)").unwrap();
    a.run(
        "CREATE PROCEDURE C (OUT S INTEGER) MODIFIES SQL DATA \
         BEGIN DECLARE X, Y INTEGER; DECLARE DONE BOOLEAN DEFAULT FALSE; \
         DECLARE K1 CURSOR FOR SELECT K, N FROM T FOR UPDATE; \
         DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
         OPEN K1; FETCH K1 INTO X, Y; \
         WHILE NOT DONE DO UPDATE T SET N = Y + X WHERE CURRENT OF K1; FETCH K1 INTO X, Y; END WHILE; CLOSE K1; \
         SET S = 0; FOR R AS SELECT N FROM T DO SET S = S + R.N; END FOR; END",
    )
    .unwrap();
    let mut p = Params::new().declare("V", integer(), Value::Null);
    a.execute("CALL C(V)", &mut p).unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["1,11", "2,22", "3,33"]);
    assert_eq!(p.get("V").map(|p| p.value.to_string()), Some("66".to_owned()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §9.19, §14.1, §14.7]
/// A BEFORE trigger's compound statement declares a variable and assigns
/// a column of the new row by IF.
#[test]
fn a_trigger_runs_a_compound_statement_9_19() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "CREATE TRIGGER G BEFORE INSERT ON T REFERENCING NEW ROW AS NR FOR EACH ROW \
         BEGIN ATOMIC DECLARE D INTEGER DEFAULT 5; IF NR.N IS NULL THEN SET NR.N = NR.K * D; END IF; END",
    )
    .unwrap();
    a.run("INSERT INTO T VALUES (2, NULL), (3, 7)").unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["2,10", "3,7"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §4.10.4, §14.1, §17.2]
/// A compound statement executed directly runs; not ATOMIC, the changes
/// of its statements before one whose exception no handler handles stay.
#[test]
fn a_direct_compound_statement_is_not_atomic_4_10_4() {
    let db = defined();
    let a = db.session("A");
    let e = a.run("BEGIN INSERT INTO T VALUES (1, 1); SIGNAL SQLSTATE '75002' SET MESSAGE_TEXT = 'stop'; END").unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "75002");
    assert_eq!(e.msg, "stop");
    assert_eq!(rows(&a, "SELECT K FROM T"), ["1"]);
    assert!(a.run("BEGIN ATOMIC INSERT INTO T VALUES (2, 2); SIGNAL SQLSTATE '75002'; END").is_err());
    assert_eq!(rows(&a, "SELECT K FROM T"), ["1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR23c, SR23d; ISO/IEC 9075-4:2006 draft §14.1 SR10, §14.13]
/// A routine's SQL-data access counts every statement its program holds:
/// a CONTAINS SQL body that inserts in a compound statement, or a READS
/// SQL DATA one that updates in an IF, is refused; two variables of one
/// name in a compound statement are refused; a positioned UPDATE of a FOR
/// statement's named cursor changes the row it is on.
#[test]
fn nested_statements_count_and_for_cursors_update_14_13() {
    let db = defined();
    let a = db.session("A");
    for refused in [
        "CREATE PROCEDURE P1 () CONTAINS SQL BEGIN INSERT INTO T VALUES (1, 1); END",
        "CREATE PROCEDURE P2 () READS SQL DATA BEGIN IF TRUE THEN UPDATE T SET N = 0; END IF; END",
        "CREATE FUNCTION F1 () RETURNS INTEGER READS SQL DATA BEGIN DELETE FROM T; RETURN 1; END",
        "BEGIN DECLARE X INTEGER; DECLARE X INTEGER; END",
    ] {
        assert_eq!(a.run(refused).unwrap_err().code, turborel::error::Code::SyntaxRule, "{refused}");
    }
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    a.run("BEGIN FOR R AS C CURSOR FOR SELECT K, N FROM T FOR UPDATE DO UPDATE T SET N = R.N + R.K WHERE CURRENT OF C; END FOR; END")
        .unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["1,11", "2,22"]);
}
