//! SQL-invoked procedures of an SQL routine body (Feature T321), invoked
//! by CALL [ISO/IEC 9075-2:2023 §16.1, §9.18, §10.4]: their arguments by
//! mode, their bodies run as their owners, and where CALL may be.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

/// Schema A: T (K, N) and L (WHO, K), a log.
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER) CREATE TABLE L (WHO VARCHAR(10), K INTEGER)").unwrap();
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

/// The SQLCODE class of `sql` run in `s`, which fails.
fn code(s: &Session, sql: &str) -> Code {
    s.run(sql).unwrap_err().code
}

/// An INTEGER host parameter's type.
fn integer() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }
}

/// The exact number `n`.
fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §16.1 GR1, §9.18 GR3, GR4, GR6, GR10b]
/// CALL runs the body with each input parameter its argument's value, an
/// output parameter null until set; each output and input-output
/// parameter's value is then assigned to its target, by retrieval
/// assignment to a host parameter [§9.1], a null to one of no indicator
/// SQLSTATE 22002.
#[test]
fn call_passes_parameters_by_mode_16_1() {
    let db = defined();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    a.run("CREATE PROCEDURE ADD_ROW (IN K INTEGER, IN N INTEGER) MODIFIES SQL DATA INSERT INTO T VALUES (K, N)").unwrap();
    a.run("CREATE PROCEDURE FIND (IN WANTED INTEGER, OUT GOT INTEGER) READS SQL DATA SELECT N INTO GOT FROM T WHERE K = WANTED").unwrap();
    a.run("CREATE PROCEDURE BUMP (INOUT X INTEGER) READS SQL DATA SELECT X + MAX(N) INTO X FROM T").unwrap();
    a.commit().unwrap();
    let mut p = Params::new().declare("V", integer(), Value::Null).declare("W", integer(), int(5));
    a.execute("CALL ADD_ROW(3, 30)", &mut p).unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["1,10", "2,20", "3,30"]);
    a.execute("CALL FIND(2, V)", &mut p).unwrap();
    assert_eq!(p.value("V").to_string(), "20");
    a.execute("CALL A.BUMP(W)", &mut p).unwrap();
    assert_eq!(p.value("W").to_string(), "35");
    let none = a.execute("CALL FIND(9, V)", &mut p).unwrap_err();
    assert_eq!(none.state, Some(SqlState::NULL_VALUE_NO_INDICATOR), "no row: the output parameter is null, and V has no indicator");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR9h iii, §10.4 CR6, CR7] an
/// output parameter's argument is a target specification; NULL or
/// DEFAULT of an input parameter, and DEFAULT of one both, are of
/// Features T522 and T523, not supported.
#[test]
fn arguments_are_as_their_modes_allow_9_18() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE GIVE (IN X INTEGER, OUT Y INTEGER, INOUT Z INTEGER) CONTAINS SQL GET DIAGNOSTICS Y = NUMBER").unwrap();
    a.commit().unwrap();
    let mut p = Params::new().declare("V", integer(), int(1)).declare("W", integer(), int(1));
    let run = |p: &mut Params, sql: &str| a.execute(sql, p).map_err(|e| e.code);
    assert_eq!(run(&mut p, "CALL GIVE(1, 2, W)"), Err(Code::SyntaxRule), "an output parameter takes a target");
    assert_eq!(run(&mut p, "CALL GIVE(1, V, 3)"), Err(Code::SyntaxRule), "so does one both");
    assert_eq!(run(&mut p, "CALL GIVE(1, V, NULL)"), Err(Code::SyntaxRule));
    assert_eq!(run(&mut p, "CALL GIVE(NULL, V, W)"), Err(Code::Unsupported), "T522");
    assert_eq!(run(&mut p, "CALL GIVE(DEFAULT, V, W)"), Err(Code::Unsupported), "T522");
    assert_eq!(run(&mut p, "CALL GIVE(1, V, DEFAULT)"), Err(Code::Unsupported), "T523");
    assert_eq!(run(&mut p, "CALL GIVE(1, V)"), Err(Code::SyntaxRule), "SR9b");
    assert_eq!(run(&mut p, "CALL GIVE('x', V, W)"), Err(Code::SyntaxRule), "SR9h v");
    assert!(run(&mut p, "CALL GIVE(1, V, W)").is_ok());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR6, SR9, §16.1 SR3, §12.3] CALL
/// invokes a procedure, on which EXECUTE is needed; a function is no
/// subject of it. The body runs as the owner of the procedure's schema
/// [§9.18 GR6i], so the caller needs no privilege on what it writes.
#[test]
fn call_needs_execute_and_runs_as_the_owner_12_3() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE NOTE (IN K INTEGER) MODIFIES SQL DATA INSERT INTO L VALUES (CURRENT_USER, K)").unwrap();
    a.run("CREATE FUNCTION F () RETURNS INTEGER CONTAINS SQL RETURN 1").unwrap();
    a.commit().unwrap();
    assert_eq!(code(&a, "CALL F()"), Code::SyntaxRule, "§9.18 SR9e");
    assert_eq!(code(&a, "CALL NOWHERE()"), Code::SyntaxRule);
    let b = db.session("B");
    assert_eq!(code(&b, "CALL A.NOTE(1)"), Code::Privilege);
    a.run("GRANT EXECUTE ON PROCEDURE NOTE TO B").unwrap();
    a.commit().unwrap();
    b.run("CALL A.NOTE(2)").unwrap();
    b.commit().unwrap();
    assert_eq!(rows(&a, "SELECT WHO, K FROM L"), ["A,2"]);
    assert_eq!(code(&b, "INSERT INTO A.L VALUES ('B', 3)"), Code::Privilege);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR9h iii 5, §6.8] a procedure's
/// body may CALL another, an output parameter of its own the target of
/// the other's; and a body's SQL parameter is hidden by a column of its
/// name [§6.6 SR4].
#[test]
fn a_body_calls_another_procedure_9_18() {
    let db = defined();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    a.run("CREATE PROCEDURE INNER_TOTAL (OUT S INTEGER) READS SQL DATA SELECT SUM(N) INTO S FROM T").unwrap();
    a.run("CREATE PROCEDURE OUTER_TOTAL (OUT TOTAL INTEGER) READS SQL DATA CALL INNER_TOTAL(TOTAL)").unwrap();
    a.run("CREATE PROCEDURE HIDDEN (IN N INTEGER, OUT C INTEGER) READS SQL DATA SELECT COUNT(*) INTO C FROM T WHERE N = HIDDEN.N").unwrap();
    a.commit().unwrap();
    let mut p = Params::new().declare("V", integer(), Value::Null);
    a.execute("CALL OUTER_TOTAL(V)", &mut p).unwrap();
    assert_eq!(p.value("V").to_string(), "30");
    a.execute("CALL HIDDEN(10, V)", &mut p).unwrap();
    assert_eq!(p.value("V").to_string(), "1", "N is the column, HIDDEN.N the parameter");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR13, SR14a, §9.18 SR9h iii 3]
/// a triggered action may CALL a procedure; a BEFORE trigger's only one
/// that modifies no SQL-data, which may assign a column of the new row as
/// an output parameter's target; an AFTER trigger's no such column.
#[test]
fn a_trigger_calls_a_procedure_11_49() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE NOTE (IN K INTEGER) MODIFIES SQL DATA INSERT INTO L VALUES ('NOTE', K)").unwrap();
    a.run("CREATE PROCEDURE TEN (IN K INTEGER, OUT N INTEGER) READS SQL DATA SELECT K * 10 INTO N FROM L WHERE WHO = 'NOTE' AND K = 1")
        .unwrap();
    a.run("CREATE PROCEDURE NONE (OUT N INTEGER) CONTAINS SQL GET DIAGNOSTICS N = NUMBER").unwrap();
    a.commit().unwrap();
    a.run("CREATE TRIGGER AFTERWARDS AFTER INSERT ON T REFERENCING NEW AS NR FOR EACH ROW CALL NOTE(NR.K)").unwrap();
    let before = "CREATE TRIGGER EARLY BEFORE INSERT ON T REFERENCING NEW AS NR FOR EACH ROW CALL NOTE(NR.K)";
    assert_eq!(code(&a, before), Code::SyntaxRule, "SR14a");
    a.run("CREATE TRIGGER SET_N BEFORE INSERT ON T REFERENCING NEW AS NR FOR EACH ROW CALL NONE(NR.N)").unwrap();
    let after = "CREATE TRIGGER LATE AFTER INSERT ON T REFERENCING NEW AS NR FOR EACH ROW CALL NONE(NR.N)";
    assert_eq!(code(&a, after), Code::SyntaxRule, "SR9h iii 3");
    a.run("INSERT INTO T VALUES (1, 99)").unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T"), ["1,0"], "NUMBER of an empty diagnostics area");
    assert_eq!(rows(&a, "SELECT WHO, K FROM L"), ["NOTE,1"]);
}

/// Verifies: [DESIGN] RULES.md D71: triggered actions and procedures nest
/// at most 16 deep, a CALL past it refused with SQLCODE -900.
#[test]
fn procedures_nest_at_most_sixteen_deep() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE P0 () MODIFIES SQL DATA INSERT INTO L VALUES ('DEEP', 0)").unwrap();
    for k in 1..=16 {
        a.run(&format!("CREATE PROCEDURE P{k} () MODIFIES SQL DATA CALL P{}()", k - 1)).unwrap();
    }
    a.commit().unwrap();
    a.run("CALL P15()").unwrap();
    assert_eq!(rows(&a, "SELECT K FROM L"), ["0"]);
    assert_eq!(code(&a, "CALL P16()"), Code::Unsupported);
    assert_eq!(rows(&a, "SELECT K FROM L"), ["0"], "the statement undone");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 GR6f, GR14, §23.1 GR4q] a
/// procedure's statements have a diagnostics area of their own; the CALL
/// that failed in it has one condition, and of a parameter's assignment
/// it names the parameter.
#[test]
fn diagnostics_of_a_call_9_18() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE SMALL (IN S SMALLINT) CONTAINS SQL GET DIAGNOSTICS S = NUMBER").unwrap();
    a.commit().unwrap();
    let failed = a.run("CALL SMALL(100000)").unwrap_err();
    assert_eq!(failed.state, Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE));
    let mut p = Params::new().declare("V", DataType::character(128), Value::Null);
    let mut item = |name: &str| {
        a.execute(&format!("GET DIAGNOSTICS CONDITION 1 V = {name}"), &mut p).unwrap();
        p.value("V").to_string().trim_end().to_owned()
    };
    assert_eq!([item("ROUTINE_NAME"), item("PARAMETER_MODE"), item("PARAMETER_NAME")], ["SMALL", "IN", "S"]);
    let mut n = Params::new().declare("N", integer(), Value::Null);
    a.execute("GET DIAGNOSTICS CONDITION 1 N = PARAMETER_ORDINAL_POSITION", &mut n).unwrap();
    assert_eq!(n.value("N").to_string(), "1");
    a.execute("GET DIAGNOSTICS V = COMMAND_FUNCTION", &mut p).unwrap();
    assert_eq!(p.value("V").to_string().trim_end(), "CALL");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §16.2 SR1] RETURN is a function's body
/// alone, not a statement of its own.
#[test]
fn return_outside_a_function_16_2() {
    let db = defined();
    let a = db.session("A");
    assert_eq!(a.run("RETURN 1").unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR10] a statement
/// prepared before a function is defined does not find it; one prepared
/// after it is dropped no more invokes it.
#[test]
fn prepared_statements_follow_the_definitions() {
    let db = defined();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10)").unwrap();
    assert!(a.prepare("SELECT G(K) FROM T").is_err());
    a.run("CREATE FUNCTION G (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X * 3").unwrap();
    a.commit().unwrap();
    let p = a.prepare("SELECT G(K) FROM T").unwrap();
    let mut c = a.open_prepared(&p, &[]).unwrap();
    assert_eq!(c.fetch().unwrap().map(|r| r.first().map(ToString::to_string)), Some(Some("3".to_owned())));
    drop(c);
    a.run("DROP FUNCTION G RESTRICT").unwrap();
    a.commit().unwrap();
    assert!(a.open_prepared(&p, &[]).and_then(|mut c| c.fetch()).is_err());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §16.1, §16.2 SR1, §20.1]
/// CALL, executed directly or prepared, runs its procedure, not read as a
/// query of a table named CALL; RETURN so executed is refused as no
/// function's body.
#[test]
fn call_is_executed_directly_and_prepared_16_1() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE ADD_ONE (IN K INTEGER) MODIFIES SQL DATA INSERT INTO T VALUES (K, 1)").unwrap();
    a.commit().unwrap();
    assert!(matches!(a.direct("CALL ADD_ONE(7)").unwrap(), turborel::db::Direct::Done(_)));
    let prepared = a.prepare("CALL ADD_ONE(8)").unwrap();
    assert!(!prepared.is_query());
    a.execute_prepared(&prepared, &[]).unwrap();
    assert_eq!(rows(&a, "SELECT K FROM T ORDER BY K"), ["7", "8"]);
    assert_eq!(a.direct("RETURN 1").err().map(|e| e.code), Some(Code::SyntaxRule));
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 SR4, SR5d, §10.4] (Feature T521)
/// a CALL's arguments named each give the parameter of its name its
/// value, in any order; one naming no parameter, or a list naming some
/// arguments and not all, is refused (SQLCODE -103).
#[test]
fn named_arguments_in_call_10_4() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE ADD_ROW (IN K INTEGER, IN N INTEGER) MODIFIES SQL DATA INSERT INTO T VALUES (K, N)").unwrap();
    a.run("CALL ADD_ROW(N => 10, K => 1)").unwrap();
    a.run("CALL ADD_ROW(K => 2, N => 20)").unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["1,10", "2,20"]);
    assert_eq!(code(&a, "CALL ADD_ROW(K => 3, M => 30)"), Code::SyntaxRule);
    assert_eq!(code(&a, "CALL ADD_ROW(3, N => 30)"), Code::SyntaxRule);
}
