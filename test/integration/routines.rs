//! SQL-invoked functions of an SQL routine body (Feature T321): defined
//! [ISO/IEC 9075-2:2023 §11.60] and invoked [§9.18, §10.4], their body a
//! `<return statement>` [§16.2].

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};
use turborel::params::Params;
use turborel::value::{DataType, Value};

/// Schema A: T (K, P) of rows (1, 10) and (2, 20).
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, P INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    a.commit().unwrap();
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

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60, §9.18 GR3 to GR10, §16.2 GR1] a
/// function defined is invoked for each row, its argument's value its
/// parameter's, the value of its `<return statement>` its result; by its
/// name with its schema, and without, of the default schema [§9.18 SR10b].
#[test]
fn a_function_returns_its_value_9_18() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE FUNCTION PLUS (X INTEGER) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN X + 1").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT K, A.PLUS(K) FROM T ORDER BY K"), ["1,2", "2,3"]);
    assert_eq!(rows(&a, "SELECT K FROM T WHERE PLUS(P) = 21"), ["2"]);
    assert_eq!(rows(&a, "SELECT PLUS(PLUS(1)) FROM T WHERE K = 1"), ["3"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.6 SR8, SR4] a parameter is referenced
/// by its name or qualified by the function's, and a column of a table of
/// the body's own query hides a parameter of its name.
#[test]
fn a_column_hides_a_parameter_6_6() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE FUNCTION TOP (P INTEGER) RETURNS INTEGER READS SQL DATA RETURN (SELECT MAX(P) FROM T)").unwrap();
    a.run("CREATE FUNCTION OWN (P INTEGER) RETURNS INTEGER READS SQL DATA RETURN (SELECT OWN.P + MAX(K) * 0 FROM T)").unwrap();
    a.run("CREATE FUNCTION BELOW (X INTEGER) RETURNS INTEGER READS SQL DATA RETURN (SELECT COUNT(*) FROM T WHERE P < X)").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT TOP(1), OWN(5), BELOW(15) FROM T WHERE K = 1"), ["20,5,1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 GR4c i, GR10a iii, §9.2] each
/// argument is stored in its parameter, the result in the result type:
/// a value too long for either is SQLSTATE 22001, one out of range 22003;
/// RETURN NULL is a null of the result type [§16.2 SR2].
#[test]
fn arguments_and_results_are_stored_9_2() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE FUNCTION SHORT (S CHARACTER VARYING(3)) RETURNS CHARACTER VARYING(5) CONTAINS SQL RETURN S").unwrap();
    a.run("CREATE FUNCTION LONG (S CHARACTER VARYING(9)) RETURNS CHARACTER VARYING(2) CONTAINS SQL RETURN S").unwrap();
    a.run("CREATE FUNCTION SMALL (N SMALLINT) RETURNS SMALLINT CONTAINS SQL RETURN N * 1000").unwrap();
    a.run("CREATE FUNCTION NOTHING () RETURNS INTEGER CONTAINS SQL RETURN NULL").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT SHORT('ab') FROM T WHERE K = 1"), ["ab"]);
    let too_long = a.open_cursor("SELECT SHORT('abcd') FROM T").and_then(|mut c| c.fetch()).unwrap_err();
    assert_eq!(too_long.state, Some(SqlState::STRING_DATA_RIGHT_TRUNCATION));
    let cut = a.open_cursor("SELECT LONG('abc') FROM T").and_then(|mut c| c.fetch()).unwrap_err();
    assert_eq!(cut.state, Some(SqlState::STRING_DATA_RIGHT_TRUNCATION));
    let out = a.open_cursor("SELECT SMALL(100) FROM T").and_then(|mut c| c.fetch()).unwrap_err();
    assert_eq!(out.state, Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE));
    assert_eq!(rows(&a, "SELECT NOTHING() IS NULL FROM T WHERE K = 1"), ["TRUE"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 GR8a, §11.60 SR9f] a null-call
/// function given a null returns null, its body not run; one called on
/// null input runs it.
#[test]
fn a_null_call_function_returns_null_9_18() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE FUNCTION SKIP (X INTEGER) RETURNS INTEGER RETURNS NULL ON NULL INPUT CONTAINS SQL RETURN COALESCE(X, 7)").unwrap();
    a.run("CREATE FUNCTION RUN (X INTEGER) RETURNS INTEGER CALLED ON NULL INPUT CONTAINS SQL RETURN COALESCE(X, 7)").unwrap();
    a.commit().unwrap();
    let both = "SELECT SKIP(CAST(NULL AS INTEGER)) IS NULL, RUN(CAST(NULL AS INTEGER)) FROM T WHERE K = 1";
    assert_eq!(rows(&a, both), ["TRUE,7"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 GR8b ix, §23.1 GR4l] a function
/// whose body is no `<return statement>` raises SQLSTATE 2F005 as it is
/// invoked, its diagnostics area naming it and its specific name.
#[test]
fn a_function_that_returns_nothing_raises_2f005() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE FUNCTION NONE (X INTEGER) RETURNS INTEGER SPECIFIC NONE_1 CONTAINS SQL GET DIAGNOSTICS X = NUMBER").unwrap();
    a.commit().unwrap();
    let raised = a.open_cursor("SELECT NONE(K) FROM T").and_then(|mut c| c.fetch()).unwrap_err();
    assert_eq!(raised.state, Some(SqlState::NO_RETURN_STATEMENT));
    let mut p = Params::new().declare("V", DataType::character(128), Value::Null);
    let item = |p: &mut Params, name: &str| {
        a.execute(&format!("GET DIAGNOSTICS CONDITION 1 V = {name}"), p).unwrap();
        p.value("V").to_string().trim_end().to_owned()
    };
    assert_eq!([item(&mut p, "ROUTINE_SCHEMA"), item(&mut p, "ROUTINE_NAME"), item(&mut p, "SPECIFIC_NAME")], ["A", "NONE", "NONE_1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR6, §12.3, §11.60 GR1] invoking
/// a function needs EXECUTE on it, which its schema's owner holds and
/// may grant, by name or to PUBLIC; the body runs as that owner, so the
/// invoker needs no privilege on what it reads [§9.18 GR6i].
#[test]
fn execute_is_needed_and_the_body_runs_as_its_owner_12_3() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE FUNCTION TOTAL () RETURNS INTEGER READS SQL DATA RETURN (SELECT SUM(P) FROM T)").unwrap();
    a.run("CREATE FUNCTION WHO () RETURNS CHARACTER VARYING(9) SPECIFIC WHO_S CONTAINS SQL RETURN CURRENT_USER").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    assert_eq!(b.open_cursor("SELECT A.TOTAL() FROM (VALUES (0)) AS X (Z)").err().map(|e| e.code), Some(Code::Privilege));
    a.run("GRANT EXECUTE ON FUNCTION TOTAL TO B").unwrap();
    a.run("GRANT EXECUTE ON ROUTINE WHO TO PUBLIC").unwrap();
    a.commit().unwrap();
    let one = "SELECT A.TOTAL(), A.WHO() FROM (VALUES (0)) AS X (Z)";
    assert_eq!(rows(&b, one), ["30,A"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR10a, SR10d, SR10h, §16.2 SR4,
/// §7.17 SR25] an invocation of too many arguments, or of one whose type
/// its parameter's takes not, is refused, and so is one, in a query, of a
/// function that possibly modifies SQL-data, and a function whose
/// `<return value>` its result type takes not.
#[test]
fn invocations_and_bodies_are_checked_9_18() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE FUNCTION ONE (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X").unwrap();
    a.run("CREATE FUNCTION WRITER () RETURNS INTEGER MODIFIES SQL DATA RETURN 1").unwrap();
    a.commit().unwrap();
    assert_eq!(a.open_cursor("SELECT WRITER() FROM T").err().map(|e| e.code), Some(Code::SyntaxRule), "§7.17 SR25");
    assert_eq!(a.open_cursor("SELECT ONE(1, 2) FROM T").err().map(|e| e.code), Some(Code::SyntaxRule));
    assert_eq!(a.open_cursor("SELECT ONE(TRUE) FROM T").err().map(|e| e.code), Some(Code::SyntaxRule));
    assert_eq!(code(&a, "CREATE FUNCTION BAD (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN TRUE"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR23c, SR23d, SR23f] (IA075,
/// IA076, IA078) a body of CONTAINS SQL reads no SQL-data, one of READS
/// SQL DATA modifies none, and none is a schema or transaction statement.
#[test]
fn the_data_access_indication_is_checked_11_60() {
    let db = defined();
    let a = db.session("A");
    assert_eq!(code(&a, "CREATE FUNCTION R () RETURNS INTEGER CONTAINS SQL RETURN (SELECT MAX(K) FROM T)"), Code::SyntaxRule);
    assert_eq!(code(&a, "CREATE PROCEDURE M () READS SQL DATA INSERT INTO T VALUES (9, 9)"), Code::SyntaxRule);
    assert_eq!(code(&a, "CREATE PROCEDURE C () MODIFIES SQL DATA COMMIT"), Code::SyntaxRule);
    assert_eq!(code(&a, "CREATE PROCEDURE D () MODIFIES SQL DATA DROP TABLE T CASCADE"), Code::SyntaxRule);
    assert_eq!(code(&a, "CREATE PROCEDURE R () CONTAINS SQL RETURN 1"), Code::SyntaxRule);
    a.run("CREATE PROCEDURE ONE (OUT N INTEGER) READS SQL DATA SELECT K INTO N FROM T WHERE K = 1").unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR20, SR9u] a second
/// function of a name in a schema is defined if told apart by the types
/// of its parameters (Feature T341), and refused if not; a specific name
/// a routine of the schema has already is refused.
#[test]
fn names_are_unique_11_60() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE FUNCTION F (X INTEGER) RETURNS INTEGER SPECIFIC FS CONTAINS SQL RETURN X").unwrap();
    a.run("CREATE FUNCTION F (X CHARACTER(1)) RETURNS INTEGER CONTAINS SQL RETURN 1").unwrap();
    assert_eq!(code(&a, "CREATE FUNCTION F (Y INTEGER) RETURNS INTEGER CONTAINS SQL RETURN 2"), Code::SyntaxRule);
    assert_eq!(code(&a, "CREATE PROCEDURE P (X INTEGER) SPECIFIC FS CONTAINS SQL GET DIAGNOSTICS X = NUMBER"), Code::SyntaxRule);
    assert_eq!(code(&a, "CREATE FUNCTION G (X INTEGER) RETURNS INTEGER SPECIFIC B.GS CONTAINS SQL RETURN X"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.9 CR2, CR3, §11.4 SR10d, SR10e,
/// §9.16] a deterministic function of CONTAINS SQL may be in a CHECK and
/// a generation expression; a possibly non-deterministic one, or one that
/// reads SQL-data, may not.
#[test]
fn constraints_take_deterministic_functions_9_16() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE FUNCTION TWICE (X INTEGER) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN X * 2").unwrap();
    a.run("CREATE FUNCTION MAYBE (X INTEGER) RETURNS INTEGER NOT DETERMINISTIC CONTAINS SQL RETURN X").unwrap();
    a.run("CREATE FUNCTION READER (X INTEGER) RETURNS INTEGER DETERMINISTIC READS SQL DATA RETURN (SELECT MAX(K) FROM T)").unwrap();
    a.commit().unwrap();
    a.run("CREATE TABLE C (K INTEGER CHECK (TWICE(K) < 10), G INTEGER GENERATED ALWAYS AS (TWICE(K)))").unwrap();
    a.run("INSERT INTO C (K) VALUES (3)").unwrap();
    assert_eq!(rows(&a, "SELECT K, G FROM C"), ["3,6"]);
    assert_eq!(code(&a, "INSERT INTO C (K) VALUES (5)"), Code::Constraint);
    assert_eq!(code(&a, "CREATE TABLE M (K INTEGER CHECK (MAYBE(K) < 10))"), Code::SyntaxRule);
    assert_eq!(code(&a, "CREATE TABLE R (K INTEGER CHECK (READER(K) < 10))"), Code::SyntaxRule);
    assert_eq!(code(&a, "CREATE TABLE G (K INTEGER, G INTEGER GENERATED ALWAYS AS (MAYBE(K)))"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR11, §9.18] a
/// trigger's WHEN condition may invoke a function, evaluated for each run;
/// one that possibly modifies SQL-data it may not.
#[test]
fn a_when_condition_invokes_a_function_11_49() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TABLE L (K INTEGER)").unwrap();
    a.run("CREATE FUNCTION BIG (X INTEGER) RETURNS BOOLEAN DETERMINISTIC CONTAINS SQL RETURN X > 5").unwrap();
    a.run("CREATE FUNCTION WRITER (X INTEGER) RETURNS BOOLEAN MODIFIES SQL DATA RETURN TRUE").unwrap();
    a.commit().unwrap();
    let refused = "CREATE TRIGGER W AFTER INSERT ON T REFERENCING NEW AS NR FOR EACH ROW WHEN (WRITER(NR.K)) INSERT INTO L VALUES (NR.K)";
    assert_eq!(code(&a, refused), Code::SyntaxRule, "SR11");
    a.run("CREATE TRIGGER W AFTER INSERT ON T REFERENCING NEW AS NR FOR EACH ROW WHEN (BIG(NR.K)) INSERT INTO L VALUES (NR.K)").unwrap();
    a.run("INSERT INTO T VALUES (3, 0), (7, 0), (9, 0)").unwrap();
    assert_eq!(rows(&a, "SELECT K FROM L ORDER BY K"), ["7", "9"]);
}

/// Verifies: [DESIGN] [ISO/IEC 9075-2:2023 §9.18 GR8b ix 2] two functions whose bodies are
/// subqueries of no outer reference, invoked in one select list, each give
/// their own value, where the second gave the first's, the runner keeping a
/// subquery's rows by the address of one made for the first and freed
/// (regression).
#[test]
fn functions_of_subquery_bodies_give_their_own_values() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TABLE NONE_ (V INTEGER)").unwrap();
    a.run("CREATE TABLE ONE_ (V INTEGER)").unwrap();
    a.run("INSERT INTO ONE_ VALUES (1)").unwrap();
    a.run("CREATE FUNCTION F0 () RETURNS INTEGER READS SQL DATA RETURN (SELECT COUNT(*) FROM NONE_)").unwrap();
    a.run("CREATE FUNCTION F1 () RETURNS INTEGER READS SQL DATA RETURN (SELECT COUNT(*) FROM ONE_)").unwrap();
    assert_eq!(rows(&a, "SELECT F0(), F1() FROM (VALUES (0)) AS X (Z)"), ["0,1"]);
    assert_eq!(rows(&a, "SELECT F1(), F0() FROM (VALUES (0)) AS X (Z)"), ["1,0"]);
}
