//! SQL/JSON through statements, the cases the operators' and constructors'
//! own modules leave [ISO/IEC 9075-2:2023 §4.48.4; RULES.md D84]: each
//! type JSON_VALUE returns, constructors returning a CLOB, in views,
//! CHECK constraints and generated columns, JSON_OBJECTAGG filtered and
//! windowed, dynamic parameters in every form, the SQLSTATEs of the path
//! language raised from SQL, a DEFAULT evaluated only when taken, and
//! definitions using IS JSON kept across a restart.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Session};
use turborel::params::Params;
use turborel::value::Value;

use crate::common::{Scratch, after_crash, exec, expected, failed, rows, setup};

/// The SQLSTATE `sql` raises in `s`.
fn state(s: &Session, sql: &str) -> String {
    failed(s, sql).sqlstate().as_str().to_owned()
}

/// The SQLSTATE of the statement `sql`, which changes data, in `s`.
fn violated(s: &Session, sql: &str) -> String {
    s.execute(sql, &mut Params::new()).unwrap_err().sqlstate().as_str().to_owned()
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.28 SR3, GR5; §9.48 GR4b] JSON_VALUE
/// RETURNING each type of the cast's targets: a time and a timestamp with
/// and without a time zone, BIGINT, REAL, NUMERIC, DECFLOAT, a fixed-length
/// character string padded, and a CLOB.
#[test]
fn json_value_returns_each_type_6_28() {
    let db = setup();
    let s = db.session("APP");
    let q = r#"VALUES (JSON_VALUE('"10:20:30"', 'lax $' RETURNING TIME),
             JSON_VALUE('"10:20:30+02:00"', 'lax $' RETURNING TIME WITH TIME ZONE),
             JSON_VALUE('"2024-02-29 10:20:30+02:00"', 'lax $' RETURNING TIMESTAMP WITH TIME ZONE),
             JSON_VALUE('9000000000', 'lax $' RETURNING BIGINT) + 1)"#;
    assert_eq!(rows(&s, q), expected(&["10:20:30,10:20:30+02:00,2024-02-29 10:20:30.000000+02:00,9000000001"]));
    let q = r#"VALUES (JSON_VALUE('1.5', 'lax $' RETURNING REAL), JSON_VALUE('1.25', 'lax $' RETURNING NUMERIC(4,1)),
             JSON_VALUE('1234567890123456789012', 'lax $' RETURNING DECFLOAT), JSON_VALUE('"ab"', 'lax $' RETURNING CHAR(4)) || '|',
             JSON_VALUE('"long"', 'lax $' RETURNING CLOB(10)))"#;
    assert_eq!(rows(&s, q), expected(&["1.5E0,1.2,1234567890123456789012,ab  |,long"]));
    assert_eq!(state(&s, "VALUES JSON_VALUE('\"x\"', 'lax $' RETURNING TIME ERROR ON ERROR)"), "22018");
    assert_eq!(state(&s, "VALUES JSON_VALUE('1e300', 'lax $' RETURNING REAL ERROR ON ERROR)"), "22003");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 SR5, GR2e, GR3e; §10.11 SR6] the
/// constructors RETURNING CLOB: of a text past a CHARACTER VARYING's
/// default length, 32767, whole; without RETURNING, 22001.
#[test]
fn constructors_return_a_clob_6_34() {
    let db = setup();
    let s = db.session("APP");
    let q = "VALUES (CHAR_LENGTH(JSON_ARRAY(RPAD('a', 40000, 'a') RETURNING CLOB)), \
             CHAR_LENGTH(JSON_OBJECT('k' VALUE RPAD('b', 40000, 'b') RETURNING CLOB(1M))))";
    assert_eq!(rows(&s, q), expected(&["32771,32775"]));
    let q = "SELECT JSON_ARRAYAGG(ID ORDER BY ID RETURNING CLOB) FROM T";
    assert_eq!(rows(&s, q), expected(&["[1,2,3]"]));
    assert_eq!(state(&s, "VALUES JSON_ARRAY(RPAD('a', 40000, 'a'))"), "22001");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.11; §10.9 SR, GR; §7.15] JSON_OBJECTAGG
/// with FILTER, and as a window function OVER a partition and a frame.
#[test]
fn json_objectagg_filtered_and_windowed_10_11() {
    let db = setup();
    let s = db.session("APP");
    exec(&s, "CREATE TABLE KV (G INTEGER, K VARCHAR(3), V INTEGER)").unwrap();
    exec(&s, "INSERT INTO KV VALUES (1, 'a', 1), (1, 'b', 2), (1, 'c', 3), (2, 'd', 4), (2, 'e', NULL)").unwrap();
    let q = "SELECT G, JSON_OBJECTAGG(K VALUE V ABSENT ON NULL RETURNING VARCHAR(40)) FILTER (WHERE V <> 2) FROM KV GROUP BY G ORDER BY G";
    assert_eq!(rows(&s, q), expected(&[r#"1,{"a":1,"c":3}"#, "2,{\"d\":4}"]));
    let q = "SELECT K, JSON_OBJECTAGG(K VALUE V RETURNING VARCHAR(40)) OVER (PARTITION BY G ORDER BY K \
             ROWS BETWEEN 1 PRECEDING AND CURRENT ROW) FROM KV ORDER BY K";
    let windowed = [r#"a,{"a":1}"#, r#"b,{"a":1,"b":2}"#, r#"c,{"b":2,"c":3}"#, r#"d,{"d":4}"#, r#"e,{"d":4,"e":null}"#];
    assert_eq!(rows(&s, q), expected(&windowed));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34; §11.22; §11.4 SR10; §11.9] the
/// constructors in a view, a CHECK constraint and a generated column; the
/// CHECK's IS JSON refusing a row as a violation, 23000.
#[test]
fn constructors_in_definitions_6_34() {
    let db = setup();
    let s = db.session("APP");
    exec(&s, "CREATE VIEW TJ AS SELECT ID, JSON_OBJECT('id' VALUE ID, 'name' VALUE TRIM(NAME)) AS J FROM T").unwrap();
    assert_eq!(rows(&s, "SELECT J FROM TJ WHERE ID = 1"), expected(&[r#"{"id":1,"name":"ANN"}"#]));
    exec(
        &s,
        "CREATE TABLE D (A INTEGER, B INTEGER, J VARCHAR(40) GENERATED ALWAYS AS (JSON_ARRAY(A, B)), \
              CHECK (JSON_ARRAY(A, B) IS JSON ARRAY), CHECK (JSON_OBJECT('a' VALUE A) <> '{\"a\":0}'))",
    )
    .unwrap();
    exec(&s, "INSERT INTO D (A, B) VALUES (1, NULL), (2, 3)").unwrap();
    assert_eq!(rows(&s, "SELECT J FROM D ORDER BY A"), expected(&["[1]", "[2,3]"]));
    assert_eq!(violated(&s, "INSERT INTO D (A, B) VALUES (0, 1)"), "23000");
}

/// Verifies: [ISO/IEC 9075-2:2023 §20.6 GR; §6.28; §6.35; §8.23; §8.22;
/// §6.34; §10.11; §6.36; §20.7 GR5] a dynamic parameter in every SQL/JSON
/// form: the context item, a PASSING argument, a DEFAULT, IS JSON's
/// input, a constructor's name and value, an aggregate's, its name and
/// its value FORMAT JSON typed as a constructor's are, and JSON's. A
/// regression test: a `?` as JSON_OBJECTAGG's name was refused 42000.
#[test]
fn dynamic_parameters_in_every_form_20_6() {
    let db = setup();
    let s = db.session("APP");
    let text = |t: &str| Value::Char(t.into());
    let int = |n: i128| Value::Exact(turborel::value::Decimal::new(n, 0));
    let read = |sql: &str, args: &[Value]| {
        let p = s.prepare(sql).unwrap();
        let mut c = s.open_prepared(&p, args).unwrap();
        let row = c.fetch().unwrap().unwrap();
        row.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
    };
    let q = "VALUES (JSON_VALUE(?, 'lax $.a' RETURNING INTEGER), JSON_QUERY(?, 'lax $.a'), JSON_EXISTS(?, 'lax $.a'), ? IS JSON)";
    let doc = text(r#"{"a":[1]}"#);
    assert_eq!(read(q, &[doc.clone(), doc.clone(), doc.clone(), text("[")]), "NULL,[1],TRUE,FALSE");
    let q = "VALUES (JSON_VALUE('[1,5]', 'lax $[*] ? (@ > $X)' PASSING CAST(? AS INTEGER) AS X), \
             JSON_VALUE('{}', 'lax $.a' RETURNING INTEGER DEFAULT ? ON EMPTY))";
    assert_eq!(read(q, &[int(2), int(7)]), "5,7");
    let q = "VALUES (JSON_OBJECT(? VALUE CAST(? AS INTEGER)), JSON_ARRAY(CAST(? AS VARCHAR(5)), CAST(? AS INTEGER)))";
    assert_eq!(read(q, &[text("k"), int(1), text("x"), int(2)]), r#"{"k":1},["x",2]"#);
    assert_eq!(read("VALUES JSON_SERIALIZE(JSON(?))", &[text("[ 1 ]")]), "[1]");
    let q = "SELECT JSON_ARRAYAGG(ID + ? ORDER BY ID), JSON_OBJECTAGG(CAST(? AS VARCHAR(5)) VALUE ID) FROM T WHERE ID = 1";
    assert_eq!(read(q, &[int(10), text("n")]), r#"[11],{"n":1}"#);
    // [§20.7 GR5a] JSON_OBJECTAGG types a `?` as JSON_OBJECT does: its
    // name, and its value FORMAT JSON, CHARACTER VARYING; a value without
    // FORMAT JSON none, refused 42000 [GR5b].
    let named = s.prepare("SELECT JSON_OBJECTAGG(? VALUE ID) FROM T WHERE ID = 1").unwrap();
    assert_eq!(named.parameters().first().map(|t| t.clone().data_type().is_character()), Some(true));
    assert_eq!(read("SELECT JSON_OBJECTAGG(? VALUE ID) FROM T WHERE ID = 1", &[text("n")]), r#"{"n":1}"#);
    assert_eq!(read("SELECT JSON_OBJECTAGG('k' VALUE ? FORMAT JSON) FROM T WHERE ID = 1", &[text("[1]")]), r#"{"k":[1]}"#);
    assert_eq!(read("SELECT JSON_ARRAYAGG(? FORMAT JSON) FROM T WHERE ID = 1", &[text("{}")]), "[{}]");
    let untyped = s.prepare("SELECT JSON_OBJECTAGG('k' VALUE ?) FROM T").err().map(|e| e.sqlstate().as_str().to_owned());
    assert_eq!(untyped.as_deref(), Some("42000"), "a value without FORMAT JSON takes no type");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 E III, 6 C III 1 a,
/// GR11i iii 1, GR11g ii 3 B, GR11h ii, GR11g ii 2 A II, 6 C XV, XVI,
/// XVIII; §6.34 GR3c ii 2] the SQLSTATEs of the path language raised
/// from SQL under ERROR ON ERROR: 22033, 22036, 22038, 22039, 2203B,
/// 2203C, 2202X, 2202Y and 2202Z.
#[test]
fn path_exceptions_from_sql_9_46() {
    let db = setup();
    let s = db.session("APP");
    let value = |path: &str, doc: &str| state(&s, &format!("VALUES JSON_VALUE('{doc}', '{path}' ERROR ON ERROR)"));
    assert_eq!(value("strict $[5]", "[1]"), "22033");
    assert_eq!(value("lax $.a.abs()", r#"{"a":"x"}"#), "22036");
    assert_eq!(value("strict $.a + 1", r#"{"a":"x"}"#), "22038");
    assert_eq!(value("lax $[*] + 1", "[1,2]"), "22038");
    assert_eq!(value("strict $[0]", "{}"), "22039");
    assert_eq!(value("lax -$", r#""x""#), "2203B");
    assert_eq!(value("strict $.*", "[1]"), "2203C");
    assert_eq!(value("lax $.string()", "{}"), "2202X");
    assert_eq!(value("lax $.time()", "1"), "2202Y");
    assert_eq!(value("lax $.timestamp()", "1"), "2202Z");
    assert_eq!(value("lax $.time_tz()", "1"), "22005");
    assert_eq!(value("lax $.timestamp_tz()", "1"), "22005");
    let exists = "VALUES JSON_EXISTS('[1]', 'strict $[5]' ERROR ON ERROR)";
    assert_eq!(state(&s, exists), "22033");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR2d ii 3, GR3c ii 2; §10.11 GR4b
/// ii 4, GR5g] past turborel's limit of 2^20 elements or members a
/// constructed array is 2203D and an object 2203E: of 1025 rows paired,
/// 1,050,625.
#[test]
fn too_many_elements_or_members_6_34() {
    let db = setup();
    let s = db.session("APP");
    exec(&s, "CREATE TABLE N (I INTEGER)").unwrap();
    let values: Vec<String> = (1..=1025).map(|i| format!("({i})")).collect();
    exec(&s, &format!("INSERT INTO N VALUES {}", values.join(", "))).unwrap();
    let pairs = "FROM N AS A, N AS B";
    assert_eq!(state(&s, &format!("SELECT JSON_ARRAYAGG(A.I RETURNING CLOB) {pairs}")), "2203D");
    assert_eq!(state(&s, &format!("SELECT JSON_OBJECTAGG('k' VALUE A.I RETURNING CLOB) {pairs}")), "2203E");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM N AS A, N AS B"), expected(&["1050625"]));
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.48 GR4c iii, GR5d] a DEFAULT's value
/// expression is evaluated only when its behavior is taken: a division by
/// zero of DEFAULT ON ERROR or ON EMPTY is not of a row that takes
/// neither; DEFAULT ON EMPTY's cast failing is replaced as ON ERROR says,
/// DEFAULT ON ERROR's raised. Regression test (R-TEST-7): every DEFAULT
/// was an argument, evaluated every row. [§6.14 GR1a, SR2] NEXT VALUE FOR
/// of a DEFAULT draws of every row, and is in no CASE.
#[test]
fn a_default_is_evaluated_when_taken_9_48() {
    let db = setup();
    let s = db.session("APP");
    let q = "SELECT ID, JSON_VALUE('{\"a\":' || CAST(ID AS VARCHAR(2)) || '}', 'lax $.a' RETURNING INTEGER \
             DEFAULT 1 / (ID - ID) ON EMPTY DEFAULT 1 / (ID - ID) ON ERROR) FROM T ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&["1,1", "2,2", "3,3"]));
    assert_eq!(state(&s, "VALUES JSON_VALUE('[1,2]', 'lax $' RETURNING INTEGER DEFAULT 1 / 0 ON ERROR)"), "22012");
    assert_eq!(state(&s, "VALUES JSON_VALUE('{}', 'lax $.a' DEFAULT 1 / 0 ON EMPTY)"), "22012");
    let replaced = "VALUES (JSON_VALUE('{}', 'lax $.a' RETURNING INTEGER DEFAULT 'x' || '' ON EMPTY DEFAULT 1 + 1 ON ERROR), \
                    JSON_VALUE('{}', 'lax $.a' RETURNING INTEGER DEFAULT 'x' || '' ON EMPTY NULL ON ERROR))";
    assert_eq!(rows(&s, replaced), expected(&["2,NULL"]));
    let raised = "VALUES JSON_VALUE('{}', 'lax $.a' RETURNING INTEGER DEFAULT 'x' || '' ON EMPTY ERROR ON ERROR)";
    assert_eq!(state(&s, raised), "22018");
    exec(&s, "CREATE SEQUENCE SQ").unwrap();
    let drawn = "SELECT JSON_VALUE(CASE ID WHEN 2 THEN '[]' ELSE '1' END, 'lax $' RETURNING INTEGER \
                 DEFAULT NEXT VALUE FOR SQ ON ERROR) FROM T";
    assert_eq!(rows(&s, drawn).unwrap().iter().filter(|r| *r == "1").count(), 2);
    assert_eq!(rows(&s, "SELECT NEXT VALUE FOR SQ FROM T WHERE ID = 1"), expected(&["4"]), "a value was drawn of each row");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.43 GR2a iii; §9.49] the string
/// JSON_QUERY serializes too long for its RETURNING type is 22001,
/// raised whatever ON ERROR says. Regression test (R-TEST-7): NULL ON
/// ERROR replaced it.
#[test]
fn a_serialization_too_long_is_raised_9_43() {
    let db = setup();
    let s = db.session("APP");
    let q = "VALUES JSON_QUERY('[1,2,3]', 'lax $' RETURNING VARCHAR(3) NULL ON ERROR)";
    assert_eq!(state(&s, q), "22001");
    let q = "VALUES JSON_QUERY('[1,2,3]', 'lax $' RETURNING VARCHAR(3) EMPTY ARRAY ON ERROR)";
    assert_eq!(state(&s, q), "22001");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.42 GR3a iii 3 A IV; §5.3 SR29;
/// §9.46 GR11g ii 6 C VII, VIII; §6.31 SR12] Regression tests (R-TEST-7)
/// of numbers: a JSON number past 34 significant digits is 22003, not
/// rounded, IS JSON included; trailing zeros are not significant; ceiling()
/// gives no negative zero; `%` of an approximate operand is 22000.
#[test]
fn numbers_of_json_text_9_42() {
    let db = setup();
    let s = db.session("APP");
    let long = format!("1{}", "0".repeat(34));
    assert_eq!(state(&s, &format!("VALUES JSON_VALUE('{long}1', 'lax $' ERROR ON ERROR)")), "22003");
    assert_eq!(state(&s, &format!("VALUES '{long}1' IS JSON")), "22003");
    assert_eq!(state(&s, "VALUES '1e999' IS JSON"), "22003");
    let q = format!("VALUES JSON_VALUE('1.{}', 'lax $' RETURNING DECIMAL(3,1))", "0".repeat(40));
    assert_eq!(rows(&s, &q), expected(&["1.0"]));
    let q = "VALUES JSON_QUERY('-0.5e0', 'lax $.ceiling()')";
    assert_eq!(rows(&s, q), expected(&["0E0"]), "no negative zero");
    assert_eq!(state(&s, "VALUES JSON_VALUE('5e0', 'lax $ % 2' ERROR ON ERROR)"), "22000");
    assert_eq!(rows(&s, "VALUES JSON_VALUE('5', 'lax $ % 2')"), expected(&["1"]));
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C XX; §4.7.4] Regression
/// test (R-TEST-7): datetime() takes a text of no time zone in the
/// session's, with a template and without alike; keyvalue()'s id is the
/// same of one object however it is reached.
#[test]
fn datetime_and_keyvalue_9_46() {
    let db = setup();
    let s = db.session("APP");
    exec(&s, "SET TIME ZONE INTERVAL '02:00' HOUR TO MINUTE").unwrap();
    let q = r#"VALUES JSON_EXISTS('"2024-01-02 03:04:05"', 'lax $ ? (@.datetime() == @.datetime("YYYY-MM-DD HH24:MI:SS"))')"#;
    assert_eq!(rows(&s, q), expected(&["TRUE"]));
    let q = r#"VALUES JSON_QUERY('{"a":{"x":1}}', 'lax $.a.keyvalue().id' WITH WRAPPER),
             JSON_QUERY('{"a":{"x":1}}', 'lax $.*.keyvalue().id' WITH WRAPPER)"#;
    assert_eq!(rows(&s, q), expected(&["[1]", "[1]"]));
}

/// The database in `dir`, recovering it if it was left open.
fn reopened(dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.22; §11.22; §11.9]; [DESIGN] RULES.md
/// D25: a view and a CHECK constraint using IS JSON, and a view using
/// JSON_VALUE and JSON_OBJECT, are kept as written across a restart, and
/// enforced and evaluated after it.
#[test]
fn definitions_using_json_survive_a_restart_8_22() {
    let scratch = Scratch::new("json-definitions-restart");
    let dir = scratch.path("db");
    {
        let db = reopened(&dir);
        db.schema("CREATE SCHEMA AUTHORIZATION APP").unwrap();
        let s = db.session("APP");
        exec(&s, "CREATE TABLE DOCS (ID INTEGER, J VARCHAR(40) CHECK (J IS JSON OBJECT WITH UNIQUE KEYS))").unwrap();
        exec(
            &s,
            "CREATE VIEW GOOD AS SELECT ID, JSON_VALUE(J, 'lax $.n' RETURNING INTEGER) AS N, \
                  JSON_OBJECT('id' VALUE ID) AS O FROM DOCS WHERE J IS JSON",
        )
        .unwrap();
        exec(&s, "INSERT INTO DOCS VALUES (1, '{\"n\":5}')").unwrap();
        exec(&s, "COMMIT WORK").unwrap();
        db.close().unwrap();
    }
    let db = reopened(&dir);
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT * FROM GOOD"), expected(&[r#"1,5,{"id":1}"#]));
    assert_eq!(violated(&s, "INSERT INTO DOCS VALUES (2, '{\"a\":1,\"a\":2}')"), "23000");
    assert_eq!(violated(&s, "INSERT INTO DOCS VALUES (2, '[1]')"), "23000");
    exec(&s, "INSERT INTO DOCS VALUES (2, '{\"n\":6}')").unwrap();
    assert_eq!(rows(&s, "SELECT N FROM GOOD ORDER BY N"), expected(&["5", "6"]));
}
