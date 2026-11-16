//! Array element assignment (Feature S097) [ISO/IEC 9075-2:2023 §6.4]: a
//! SELECT INTO or CALL target `A[i]`, of an SQL parameter's array or a
//! BEFORE trigger's new row column's, which assigns one element of it
//! [§14.7 GR4b ii 1, §10.4 GR10b ii 1].

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

/// The maximum cardinality of the arrays of these tests.
const MAX: u32 = 3;

/// Schema A, of a table H of a key and an array, and procedures PUT, of
/// an array, a position and a value, which assigns the value to that
/// element of the array, and SEVEN, which gives 7.
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE H (K INTEGER, V INTEGER ARRAY[3])").unwrap();
    let a = db.session("A");
    a.run(
        "CREATE PROCEDURE PUT (INOUT R INTEGER ARRAY[3], IN I INTEGER, IN X INTEGER) READS SQL DATA \
         SELECT X INTO R[I] FROM (VALUES (0)) AS D (Z)",
    )
    .unwrap();
    a.run("CREATE PROCEDURE SEVEN (OUT X INTEGER) READS SQL DATA SELECT 7 INTO X FROM (VALUES (0)) AS D (Z)").unwrap();
    a.commit().unwrap();
    db
}

/// INTEGER.
fn integer() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }
}

/// INTEGER ARRAY[3].
fn trio() -> DataType {
    DataType::array(integer(), MAX)
}

/// The array of `elements`, each an integer or, if none, null.
fn array(elements: &[Option<i128>]) -> Value {
    Value::Array(elements.iter().map(|e| e.map_or(Value::Null, |n| Value::Exact(Decimal::new(n, 0)))).collect())
}

/// Parameters of one, R, INTEGER ARRAY[3], of value `value`.
fn holding(value: Value) -> Params {
    Params::new().declare("R", trio(), value)
}

/// The SQLSTATE of `sql` run in `s` with `p`, which fails.
fn state(s: &Session, sql: &str, p: &mut Params) -> Option<SqlState> {
    s.execute(sql, p).unwrap_err().state
}

/// The SQLCODE class of `sql` run in `s`, which fails.
fn code(s: &Session, sql: &str) -> Code {
    s.run(sql).unwrap_err().code
}

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    out
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.4 SR8, §14.7 GR4b ii 1]
/// SELECT INTO an element of an SQL parameter's array replaces the
/// element at a position within its cardinality, and past it, within the
/// maximum cardinality, extends the array with nulls up to it; the value
/// is stored as in a site of the element type [§9.2].
#[test]
fn select_into_assigns_an_element_14_7() {
    let db = defined();
    let a = db.session("A");
    let mut p = holding(array(&[Some(1), Some(2), Some(3)]));
    a.execute("CALL PUT(R, 2, 9)", &mut p).unwrap();
    assert_eq!(p.value("R").to_string(), array(&[Some(1), Some(9), Some(3)]).to_string(), "replaced");
    let mut p = holding(array(&[Some(1)]));
    a.execute("CALL PUT(R, 3, 9)", &mut p).unwrap();
    assert_eq!(p.value("R").to_string(), array(&[Some(1), None, Some(9)]).to_string(), "extended with nulls");
    let mut p = holding(array(&[]));
    a.execute("CALL PUT(R, 1, CAST(5.0 AS DECIMAL(2,1)))", &mut p).unwrap();
    assert_eq!(p.value("R").to_string(), array(&[Some(5)]).to_string(), "stored in INTEGER");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.7 GR4b ii 1 A, B V 3]
/// an element of a null array, an output parameter's before it is set
/// [§9.18 GR4c], is not assigned, SQLSTATE 2200E; nor one at a position
/// below 1, past the maximum cardinality or null, 2202E.
#[test]
fn an_element_out_of_reach_is_refused_14_7() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE UNSET (OUT R INTEGER ARRAY[3]) READS SQL DATA SELECT 1 INTO R[1] FROM (VALUES (0)) AS D (Z)").unwrap();
    a.commit().unwrap();
    let mut p = holding(array(&[]));
    assert_eq!(state(&a, "CALL UNSET(R)", &mut p), Some(SqlState::NULL_VALUE_IN_ARRAY_TARGET));
    let mut p = holding(array(&[Some(1)]));
    for position in ["0", "-1", "4", "CAST(NULL AS INTEGER)"] {
        let sql = format!("CALL PUT(R, {position}, 9)");
        assert_eq!(state(&a, &sql, &mut p), Some(SqlState::ARRAY_ELEMENT_ERROR), "{position}");
    }
    assert_eq!(p.value("R").to_string(), array(&[Some(1)]).to_string(), "left as it was");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.4 SR8a, SR8c, §14.7
/// SR3b ii 1] a definition is refused (SQLCODE -103) whose target is an
/// element of an SQL parameter not of an array type; or at a position not
/// exact numeric of scale 0, a decimal or character literal or a
/// character parameter; or at a name that is no SQL parameter; or that
/// takes a value its element type cannot store. A host parameter's array
/// is no target array.
#[test]
fn element_targets_are_checked_6_4() {
    let db = defined();
    let a = db.session("A");
    let (one, from) = ("READS SQL DATA SELECT 1 INTO", "FROM (VALUES (0)) AS D (Z)");
    let refused = [
        (format!("CREATE PROCEDURE Q (INOUT N INTEGER) {one} N[1] {from}"), "§6.4 SR8a"),
        (format!("CREATE PROCEDURE Q (INOUT R INTEGER ARRAY[3]) {one} R[1.5] {from}"), "§6.4 SR8c"),
        (format!("CREATE PROCEDURE Q (INOUT R INTEGER ARRAY[3]) {one} R['1'] {from}"), "§6.4 SR8c"),
        (format!("CREATE PROCEDURE Q (INOUT R INTEGER ARRAY[3], IN C CHAR(1)) {one} R[C] {from}"), "§6.4 SR8c"),
        (format!("CREATE PROCEDURE Q (INOUT R INTEGER ARRAY[3]) {one} R[NOPE] {from}"), "§6.4"),
        (format!("CREATE PROCEDURE Q (INOUT R INTEGER ARRAY[3]) READS SQL DATA SELECT 'x' INTO R[1] {from}"), "§9.2 SR2"),
    ];
    for (sql, cited) in &refused {
        let e = a.run(sql).unwrap_err();
        assert_eq!((e.code, e.rule), (Code::SyntaxRule, Some(format!("ISO/IEC 9075-2:2023 {cited}"))), "{sql}");
    }
    let mut p = holding(array(&[Some(1)]));
    let host = a.execute("SELECT 1 INTO R[1] FROM H", &mut p).unwrap_err();
    assert_eq!((host.code, host.rule.as_deref()), (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §6.4")), "a host parameter");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.4 GR10b ii 1, §9.18
/// SR9h iii] an output argument of CALL may be an element of an SQL
/// parameter's array, at a literal position or an SQL parameter's, the
/// latter qualified by the procedure's name too; at a negative literal,
/// no element, SQLSTATE 2202E. A host parameter's element, or an element
/// of an element, is no target (SQLCODE -103).
#[test]
fn a_call_assigns_an_element_10_4() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE AT_TWO (INOUT R INTEGER ARRAY[3]) READS SQL DATA CALL SEVEN(R[2])").unwrap();
    a.run("CREATE PROCEDURE AT_I (INOUT R INTEGER ARRAY[3], IN I INTEGER) READS SQL DATA CALL SEVEN(AT_I.R[AT_I.I])").unwrap();
    a.run("CREATE PROCEDURE BELOW (INOUT R INTEGER ARRAY[3]) READS SQL DATA CALL SEVEN(R[-1])").unwrap();
    a.commit().unwrap();
    let mut p = holding(array(&[Some(1), Some(2), Some(3)]));
    a.execute("CALL AT_TWO(R)", &mut p).unwrap();
    assert_eq!(p.value("R").to_string(), array(&[Some(1), Some(7), Some(3)]).to_string());
    a.execute("CALL AT_I(R, 3)", &mut p).unwrap();
    assert_eq!(p.value("R").to_string(), array(&[Some(1), Some(7), Some(7)]).to_string());
    assert_eq!(state(&a, "CALL BELOW(R)", &mut p), Some(SqlState::ARRAY_ELEMENT_ERROR));
    assert_eq!(a.execute("CALL SEVEN(R[1])", &mut p).unwrap_err().code, Code::SyntaxRule, "a host parameter's");
    let nested = "CREATE PROCEDURE Q (INOUT R INTEGER ARRAY[3]) READS SQL DATA CALL SEVEN(R[1][1])";
    assert_eq!(code(&a, nested), Code::SyntaxRule, "an element of an element");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.4 SR7, SR8a, §14.7 GR4b
/// ii 1] a BEFORE trigger's action assigns an element of a new row
/// column's array, by SELECT INTO and by CALL, of an array type or of a
/// distinct type whose source is one; of a null array, SQLSTATE 2200E. Its
/// position is a literal, there being no SQL parameter (SQLCODE -103).
#[test]
fn a_trigger_assigns_an_element_of_the_new_row_6_4() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TYPE TRIO AS INTEGER ARRAY[3]").unwrap();
    a.run("CREATE TABLE G (K INTEGER, V TRIO)").unwrap();
    let keyed = "BEFORE INSERT ON H REFERENCING NEW AS NR FOR EACH ROW SELECT NR.K INTO NR.V[2] FROM (VALUES (0)) AS D (Z)";
    a.run(&format!("CREATE TRIGGER KEYED {keyed}")).unwrap();
    a.run("CREATE TRIGGER SEVENTH BEFORE INSERT ON G REFERENCING NEW AS NR FOR EACH ROW CALL SEVEN(NR.V[3])").unwrap();
    a.commit().unwrap();
    a.run("INSERT INTO H VALUES (4, ARRAY[1, 1, 1])").unwrap();
    a.run("INSERT INTO G VALUES (5, CAST(ARRAY[1] AS TRIO))").unwrap();
    assert_eq!(rows(&a, "SELECT CARDINALITY(V), V[2] FROM H"), ["3,4"]);
    assert_eq!(rows(&a, "SELECT CARDINALITY(CAST(V AS INTEGER ARRAY[3])), CAST(V AS INTEGER ARRAY[3])[3] FROM G"), ["3,7"]);
    let e = a.run("INSERT INTO H VALUES (6, NULL)").unwrap_err();
    assert_eq!(e.state, Some(SqlState::TRIGGERED_ACTION_EXCEPTION), "{e}");
    assert!(e.msg.contains("2200E"), "{e}");
    let named = "BEFORE INSERT ON H REFERENCING NEW AS NR FOR EACH ROW SELECT 1 INTO NR.V[NR.K] FROM (VALUES (0)) AS D (Z)";
    assert_eq!(code(&a, &format!("CREATE TRIGGER NAMED {named}")), Code::SyntaxRule);
}
