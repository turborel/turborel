//! The SQL/JSON query operators [ISO/IEC 9075-2:2023 §4.48.4; RULES.md
//! D84]: JSON_EXISTS [§8.23], JSON_VALUE [§6.28] and JSON_QUERY [§6.35],
//! of the `<JSON API common syntax>` [§10.14], with PASSING, RETURNING,
//! the wrappers, the quotes and ON EMPTY and ON ERROR, in statements.

use turborel::db::Session;
use turborel::error::{SqlError, SqlState};
use turborel::value::Value;

use crate::common::{exec, expected, rows, setup};

/// The error `sql` raises as it is prepared, opened or read.
fn failed(s: &Session, sql: &str) -> SqlError {
    let read = s.open_cursor(sql).and_then(|mut c| {
        while c.fetch()?.is_some() {}
        Ok(())
    });
    read.expect_err(sql)
}

/// The SQLCODE, the SQLSTATE and the rule of the error `sql` raises.
fn refused(s: &Session, sql: &str) -> (i32, String, Option<String>) {
    let e = failed(s, sql);
    (e.sqlcode(), e.sqlstate().as_str().to_owned(), e.rule)
}

/// `refused`, of a rule of ISO/IEC 9075-2:2023 `cited`.
fn broke(code: i32, state: &str, cited: &str) -> (i32, String, Option<String>) {
    (code, state.to_owned(), Some(format!("ISO/IEC 9075-2:2023 {cited}")))
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.28 SR1, SR3, SR4, SR5,
/// GR2, GR5; §9.48 GR4b] JSON_VALUE is the one scalar its path gives, cast
/// to the RETURNING type, of CHARACTER VARYING(32767) without one; null of
/// a null context item, of the SQL/JSON null and, NULL ON EMPTY and NULL ON
/// ERROR being implicit, of no item, of an array and of two items; lax mode
/// unwraps an array a member accessor is of [§9.46 GR11g ii 1], strict
/// mode does not.
#[test]
fn json_value_6_28() {
    let db = setup();
    let s = db.session("APP");
    let q = r#"VALUES (JSON_VALUE('{"a":1}', 'lax $.a'), JSON_VALUE('{"a":1.50}', 'lax $.a' RETURNING DECIMAL(5,2)) + 1,
             JSON_VALUE('{"a":true}', 'lax $.a' RETURNING BOOLEAN), JSON_VALUE('{"d":"2024-02-29"}', 'strict $.d' RETURNING DATE),
             JSON_VALUE('{"a":"x"}', 'lax $.a') || '!', JSON_VALUE('[2e0]', 'lax $[0]' RETURNING DOUBLE PRECISION) * 2)"#;
    assert_eq!(rows(&s, q), expected(&["1,2.50,TRUE,2024-02-29,x!,4E0"]));
    let q = r#"VALUES (JSON_VALUE(CAST(NULL AS VARCHAR(5)), 'lax $'), JSON_VALUE('{"a":null}', 'lax $.a'), JSON_VALUE('{}', 'lax $.a'),
             JSON_VALUE('[1,2]', 'lax $'), JSON_VALUE('[1,2]', 'lax $[*]'), JSON_VALUE('"7"', 'lax $' RETURNING INTEGER))"#;
    assert_eq!(rows(&s, q), expected(&["NULL,NULL,NULL,NULL,NULL,7"]));
    let q =
        r#"VALUES (JSON_VALUE('[{"a":8}]', 'lax $.a'), JSON_VALUE('[{"a":8}]', 'strict $.a'), JSON_VALUE('[{"a":8}]', 'strict $[0].a'))"#;
    assert_eq!(rows(&s, q), expected(&["8,NULL,8"]), "lax mode unwraps the array a member accessor is of");
    let types = "CREATE VIEW VT AS SELECT JSON_VALUE('1', 'lax $') AS A, JSON_VALUE('1', 'lax $' RETURNING SMALLINT) AS B, \
                 JSON_QUERY('1', 'lax $') AS C, JSON_EXISTS('1', 'lax $') AS D, JSON_QUERY('1', 'lax $' RETURNING CHAR(4)) AS E FROM T";
    exec(&s, types).unwrap();
    let q = "SELECT COLUMN_NAME, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'VT' \
             ORDER BY COLUMN_NAME";
    assert_eq!(
        rows(&s, q),
        expected(&["A,CHARACTER VARYING,32767", "B,SMALLINT,NULL", "C,CHARACTER VARYING,32767", "D,BOOLEAN,NULL", "E,CHARACTER,4"])
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.28; §9.48 GR4, GR5;
/// Features T825, T826] Each ON EMPTY and ON ERROR behavior of JSON_VALUE:
/// ERROR ON EMPTY is 22035, and ON ERROR then replaces it, so ERROR ON
/// EMPTY alone, NULL ON ERROR implicit, is null [GR4c i, GR5c]; ERROR ON
/// ERROR raises 22034 of two items, 2203F of an array, 2203G of an item of
/// a type not cast to the returned type, and a cast's own exception;
/// DEFAULT gives its value, of any value expression, cast to the returned
/// type, a column's too, the exception of that cast ON EMPTY replaced ON
/// ERROR in turn [GR4c iii], ON ERROR raised [GR5d i]; ON ERROR catches no
/// other error than a data exception, not a limit of turborel's.
#[test]
fn json_value_behaviors_9_48() {
    let db = setup();
    let s = db.session("APP");
    let fails = |sql: &str| failed(&s, sql).sqlstate().as_str().to_owned();
    assert_eq!(rows(&s, "VALUES JSON_VALUE('{}', 'lax $.a' ERROR ON EMPTY)"), expected(&["NULL"]));
    assert_eq!(fails("VALUES JSON_VALUE('{}', 'lax $.a' ERROR ON EMPTY ERROR ON ERROR)"), "22035");
    assert_eq!(rows(&s, "VALUES JSON_VALUE('{}', 'lax $.a' ERROR ON EMPTY NULL ON ERROR)"), expected(&["NULL"]));
    assert_eq!(fails("VALUES JSON_VALUE('[1,2]', 'lax $[*]' ERROR ON ERROR)"), "22034");
    assert_eq!(fails("VALUES JSON_VALUE('[1,2]', 'lax $' ERROR ON ERROR)"), "2203F");
    assert_eq!(fails("VALUES JSON_VALUE('true', 'lax $' RETURNING INTEGER ERROR ON ERROR)"), "2203G");
    assert_eq!(fails("VALUES JSON_VALUE('\"x\"', 'lax $' RETURNING INTEGER ERROR ON ERROR)"), "22018");
    assert_eq!(fails("VALUES JSON_VALUE('{', 'lax $' ERROR ON ERROR)"), "22032");
    assert_eq!(fails("VALUES JSON_VALUE('{}', 'strict $.a' ERROR ON ERROR)"), "2203A");
    let q = "SELECT ID, JSON_VALUE('{}', 'lax $.a' RETURNING INTEGER DEFAULT ID * 10 ON EMPTY), \
             JSON_VALUE('[1,2]', 'lax $' RETURNING INTEGER DEFAULT '-1' ON ERROR), \
             JSON_VALUE('{}', 'lax $.a' RETURNING INTEGER DEFAULT 5 ON EMPTY DEFAULT 6 ON ERROR), \
             JSON_VALUE('{}', 'lax $.a' RETURNING INTEGER ERROR ON EMPTY DEFAULT 6 ON ERROR), \
             JSON_VALUE('{\"a\":3}', 'lax $.a' RETURNING INTEGER DEFAULT 5 ON EMPTY) FROM T WHERE ID = 1";
    assert_eq!(rows(&s, q), expected(&["1,10,-1,5,6,3"]));
    assert_eq!(rows(&s, "VALUES JSON_VALUE('{}', 'lax $.a' RETURNING INTEGER DEFAULT 'x' ON EMPTY)"), expected(&["NULL"]));
    assert_eq!(fails("VALUES JSON_VALUE('{}', 'lax $.a' RETURNING INTEGER DEFAULT 'x' ON EMPTY ERROR ON ERROR)"), "22018");
    assert_eq!(fails("VALUES JSON_VALUE('[]', 'lax $' RETURNING INTEGER DEFAULT 'x' ON ERROR)"), "22018");
    let deep = format!("VALUES JSON_VALUE('{}1{}', 'lax $' NULL ON ERROR)", "[".repeat(200), "]".repeat(200));
    assert_eq!(failed(&s, &deep).sqlcode(), -900, "a limit of turborel's is no exception ON ERROR replaces");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.35 SR1b, SR4 to SR9;
/// §9.49 GR4, GR5; Features T828, T829] JSON_QUERY serializes the one item
/// its path gives as JSON text: WITH (UNCONDITIONAL) ARRAY WRAPPER wraps
/// the sequence in an array, WITH CONDITIONAL ARRAY WRAPPER unless it is
/// one array or object, so a lone scalar is wrapped [GR4a iv]; WITHOUT
/// ARRAY, the default, does not, and two items are then 22034; an empty
/// sequence is never wrapped; OMIT QUOTES casts a lone character string,
/// KEEP QUOTES, the default, serializes it in quotation marks.
#[test]
fn json_query_6_35() {
    let db = setup();
    let s = db.session("APP");
    let doc = r#"'{"a":[1,{"b":"x"}],"s":"t"}'"#;
    let q = format!(
        "VALUES (JSON_QUERY({doc}, 'lax $.a'), JSON_QUERY({doc}, 'lax $.a[*]' WITH ARRAY WRAPPER), \
         JSON_QUERY({doc}, 'lax $.a[*]' WITH UNCONDITIONAL WRAPPER), JSON_QUERY({doc}, 'lax $.a' WITH UNCONDITIONAL ARRAY WRAPPER), \
         JSON_QUERY({doc}, 'lax $.a' WITH CONDITIONAL ARRAY WRAPPER), JSON_QUERY({doc}, 'lax $.s' WITH CONDITIONAL WRAPPER), \
         JSON_QUERY({doc}, 'lax $.a[1]' WITHOUT WRAPPER), JSON_QUERY({doc}, 'lax $.s' WITHOUT ARRAY WRAPPER))"
    );
    assert_eq!(rows(&s, &q), expected(&[r#"[1,{"b":"x"}],[1,{"b":"x"}],[1,{"b":"x"}],[[1,{"b":"x"}]],[1,{"b":"x"}],["t"],{"b":"x"},"t""#]));
    let q = format!(
        "VALUES (JSON_QUERY({doc}, 'lax $.s' OMIT QUOTES), JSON_QUERY({doc}, 'lax $.s' KEEP QUOTES ON SCALAR STRING), \
         JSON_QUERY({doc}, 'lax $.s' OMIT QUOTES ON SCALAR STRING) || '!', JSON_QUERY({doc}, 'lax $.a[0]' OMIT QUOTES), \
         JSON_QUERY({doc}, 'lax $.z' WITH ARRAY WRAPPER), JSON_QUERY({doc}, 'lax $.a[*]'), JSON_QUERY(CAST(NULL AS CLOB), 'lax $'))"
    );
    assert_eq!(rows(&s, &q), expected(&[r#"t,"t",t!,1,NULL,NULL,NULL"#]));
    let q = format!("VALUES JSON_QUERY({doc}, 'lax $.a' RETURNING VARCHAR(30) FORMAT JSON), JSON_QUERY({doc}, 'lax $' RETURNING CLOB)");
    assert_eq!(rows(&s, &q), expected(&[r#"[1,{"b":"x"}]"#, r#"{"a":[1,{"b":"x"}],"s":"t"}"#]));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.35 SR4, SR5; §9.49 GR4c,
/// GR5a, GR6; Feature T825] Each ON EMPTY and ON ERROR behavior of
/// JSON_QUERY: ERROR ON EMPTY is 22035, EMPTY ARRAY and EMPTY OBJECT give
/// `[]` and `{}`; ERROR ON ERROR raises 22034 of two items unwrapped, and
/// 22001 of a text longer than the type, which NULL, EMPTY ARRAY and EMPTY
/// OBJECT ON ERROR replace; ERROR ON EMPTY is replaced by ON ERROR too.
#[test]
fn json_query_behaviors_9_49() {
    let db = setup();
    let s = db.session("APP");
    let fails = |sql: &str| failed(&s, sql).sqlstate().as_str().to_owned();
    assert_eq!(fails("VALUES JSON_QUERY('{}', 'lax $.a' ERROR ON EMPTY ERROR ON ERROR)"), "22035");
    assert_eq!(fails("VALUES JSON_QUERY('[1,2]', 'lax $[*]' ERROR ON ERROR)"), "22034");
    assert_eq!(fails("VALUES JSON_QUERY('[1,2]', 'lax $' RETURNING VARCHAR(3) ERROR ON ERROR)"), "22001");
    assert_eq!(fails("VALUES JSON_QUERY('[1,', 'lax $' ERROR ON ERROR)"), "22032");
    let q = "VALUES (JSON_QUERY('{}', 'lax $.a' EMPTY ARRAY ON EMPTY), JSON_QUERY('{}', 'lax $.a' EMPTY OBJECT ON EMPTY), \
             JSON_QUERY('[1,2]', 'lax $[*]' EMPTY ARRAY ON ERROR), JSON_QUERY('[1,2]', 'lax $[*]' EMPTY OBJECT ON ERROR), \
             JSON_QUERY('[1,2]', 'lax $' RETURNING VARCHAR(3) NULL ON ERROR), JSON_QUERY('{}', 'lax $.a' ERROR ON EMPTY EMPTY ARRAY ON ERROR), \
             JSON_QUERY('{}', 'lax $.a' NULL ON EMPTY))";
    assert_eq!(rows(&s, q), expected(&["[],{},[],{},NULL,[],NULL"]));
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.23 SR1, GR1, GR4] JSON_EXISTS
/// is True if the path gives an item, False if none, Unknown of a null
/// context item; an exception is False (FALSE ON ERROR being implicit),
/// True, Unknown or raised, as its ON ERROR says. It is a predicate: in a
/// WHERE clause, under NOT and IS, and a value of a select list.
#[test]
fn json_exists_8_23() {
    let db = setup();
    let s = db.session("APP");
    let q = r#"VALUES (JSON_EXISTS('{"a":1}', 'lax $.a'), JSON_EXISTS('{"a":1}', 'lax $.b'), JSON_EXISTS(CAST(NULL AS CHAR(2)), 'lax $'),
             JSON_EXISTS('{"a":1}', 'strict $.b'), JSON_EXISTS('{"a":1}', 'strict $.b' TRUE ON ERROR),
             JSON_EXISTS('{"a":1}', 'strict $.b' FALSE ON ERROR), JSON_EXISTS('{"a":1}', 'strict $.b' UNKNOWN ON ERROR),
             JSON_EXISTS('{', 'lax $'), JSON_EXISTS('{"a":[]}', 'lax $.a[*]'))"#;
    assert_eq!(rows(&s, q), expected(&["TRUE,FALSE,NULL,FALSE,TRUE,FALSE,NULL,FALSE,FALSE"]));
    assert_eq!(failed(&s, r#"VALUES JSON_EXISTS('{"a":1}', 'strict $.b' ERROR ON ERROR)"#).sqlstate().as_str(), "2203A");
    assert_eq!(failed(&s, "VALUES JSON_EXISTS('{', 'lax $' ERROR ON ERROR)").sqlstate().as_str(), "22032");
    let q = "SELECT ID FROM T WHERE JSON_EXISTS('[1,3]', 'lax $[*] ? (@ == $ID)' PASSING ID AS ID) ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&["1", "3"]));
    let q = "SELECT ID FROM T WHERE NOT JSON_EXISTS('[1,3]', 'lax $[*] ? (@ == $I)' PASSING ID AS I) \
             AND JSON_EXISTS('{}', 'strict $.a' UNKNOWN ON ERROR) IS UNKNOWN";
    assert_eq!(rows(&s, q), expected(&["2"]));
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.14 SR4, SR5; §9.46 SR4,
/// GR11b; §10.12; Feature T823] A PASSING argument's value is its named
/// variable's: the `<identifier>`'s value, a regular identifier in upper
/// case, a delimited identifier's body, is the name, so AS x is $X and AS
/// "x" is $x; without FORMAT JSON a value is the item it is, a string a
/// string, an interval cast to one, a null value the SQL/JSON null; FORMAT
/// JSON parses a string's JSON text, of a null value the empty sequence;
/// a named variable no argument has is SQLCODE -103 as it is prepared,
/// whatever the path does with it.
#[test]
fn passing_t823() {
    let db = setup();
    let s = db.session("APP");
    let q = r#"VALUES (JSON_VALUE('{}', 'lax $X' PASSING 1 AS x), JSON_VALUE('{}', 'lax $x' PASSING 2 AS "x"),
             JSON_VALUE('{}', 'lax $x + $Y' PASSING 2 AS "x", 3 AS y), JSON_VALUE('{}', 'lax $A.b' PASSING '{"b":4}' FORMAT JSON AS A),
             JSON_VALUE('{}', 'lax $A' PASSING '{"b":4}' AS A), JSON_VALUE('{}', 'lax $A.type()' PASSING CAST(NULL AS INTEGER) AS A),
             JSON_EXISTS('{}', 'lax $A' PASSING CAST(NULL AS VARCHAR(2)) FORMAT JSON AS A),
             JSON_VALUE('{}', 'lax $A' PASSING INTERVAL '2' DAY AS A), JSON_VALUE('{}', 'lax $A.type()' PASSING DATE '2024-01-01' AS A))"#;
    assert_eq!(rows(&s, q), expected(&[r#"1,2,5,4,{"b":4},null,FALSE,2,date"#]));
    let sr4 = broke(-103, "2A000", "§9.46 SR4");
    assert_eq!(refused(&s, "VALUES JSON_VALUE('{}', 'lax $x' PASSING 1 AS x)"), sr4);
    assert_eq!(refused(&s, "VALUES JSON_VALUE('{}', 'lax $X' PASSING 1 AS \"x\")"), sr4);
    assert_eq!(refused(&s, "VALUES JSON_EXISTS('{}', 'lax $ ? (exists($B))' PASSING 1 AS A)"), sr4);
    assert_eq!(s.prepare("VALUES JSON_VALUE(CAST(NULL AS VARCHAR(2)), 'lax $Z')").map(|_| ()).unwrap_err().sqlcode(), -103);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 SR8] The initial of
/// starts with that is a named variable is a character string argument
/// without FORMAT JSON, else SQLCODE -103; a literal initial, or the
/// variable elsewhere, needs neither. MC/DC of `FORMAT JSON || not a
/// character string`: a string FORMAT JSON refused, a number refused, a
/// string without it taken.
#[test]
fn starts_with_9_46() {
    let db = setup();
    let s = db.session("APP");
    let q = r#"VALUES (JSON_EXISTS('["ab","b"]', 'lax $[*] ? (@ starts with $P)' PASSING 'a' AS P),
             JSON_VALUE('["ab","b"]', 'lax $[*] ? (@ starts with "b")'), JSON_VALUE('[1,2]', 'lax $[$I]' PASSING 1 AS I))"#;
    assert_eq!(rows(&s, q), expected(&["TRUE,b,2"]));
    let sr8 = broke(-103, "2A000", "§9.46 SR8");
    assert_eq!(refused(&s, "VALUES JSON_EXISTS('[]', 'lax $[*] ? (@ starts with $P)' PASSING 1 AS P)"), sr8);
    assert_eq!(refused(&s, "VALUES JSON_EXISTS('[]', 'lax $[*] ? (@ starts with $P)' PASSING '\"a\"' FORMAT JSON AS P)"), sr8);
    let nested = "VALUES JSON_EXISTS('[]', 'lax $[*] ? (!(@ == 1) && (exists(@.a) || (@ starts with $P) is unknown)).b[0 to last]' \
                  PASSING 1 AS P)";
    assert_eq!(refused(&s, nested), sr8);
    let deeper = "VALUES JSON_EXISTS('[]', 'lax -($[1 to 2] ? (@.a like_regex \"x\" && @ starts with $P)) + 1' PASSING 1 AS P)";
    assert_eq!(refused(&s, deeper), sr8);
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.14 SR1, SR2a; §10.12 SR3,
/// SR6 to SR8; §10.13 SR2, SR2a; §6.28 SR2; §6.35 SR8; §9.46 SR3; §9.48
/// GR4c iii] The binder's refusals, each as the statement is prepared: a
/// context item of no string type, a binary string's (SQLCODE -900), AS a
/// table path name, ENCODING of a character string, a returned type
/// JSON_VALUE or JSON_QUERY does not return, a binary one (-900), QUOTES
/// with WITH a wrapper, an argument of a binary string without FORMAT
/// JSON, of an array, of no string with it, a DEFAULT CAST does not take
/// to the returned type, and a path specification no path expression.
/// MC/DC of `returnable`: INTERVAL, BINARY and an array type, of no
/// condition, refused, a character string, numeric, datetime and BOOLEAN
/// type each alone returned (`json_value_6_28`); of QUOTES and WITH
/// CONDITIONAL or UNCONDITIONAL WRAPPER: both refused, either alone taken
/// (`json_query_6_35`).
#[test]
fn refusals_10_14() {
    let db = setup();
    let s = db.session("APP");
    let cast_rule = (-103, "2A000".to_owned(), Some("ISO/IEC 9075:1992 §6.10 SR4".to_owned()));
    let cases = [
        ("VALUES JSON_VALUE(1, 'lax $')", broke(-103, "2A000", "§10.14 SR1")),
        ("VALUES JSON_EXISTS(X'00', 'lax $')", (-900, "0A000".to_owned(), None)),
        ("VALUES JSON_VALUE('1', 'lax $' AS P)", broke(-103, "2A000", "§10.14 SR2a")),
        ("VALUES JSON_VALUE('1' FORMAT JSON ENCODING UTF8, 'lax $')", broke(-103, "2A000", "§10.12 SR8")),
        ("VALUES JSON_VALUE('1', 'lax $' RETURNING INTERVAL DAY)", broke(-103, "2A000", "§6.28 SR2")),
        ("VALUES JSON_VALUE('1', 'lax $' RETURNING BINARY(2))", broke(-103, "2A000", "§6.28 SR2")),
        ("VALUES JSON_VALUE('1', 'lax $' RETURNING INTEGER ARRAY)", broke(-103, "2A000", "§6.28 SR2")),
        ("VALUES JSON_QUERY('1', 'lax $' RETURNING INTEGER)", broke(-103, "2A000", "§10.13 SR2")),
        ("VALUES JSON_QUERY('1', 'lax $' RETURNING ROW(A INTEGER))", broke(-103, "2A000", "§10.13 SR2")),
        ("VALUES JSON_QUERY('1', 'lax $' RETURNING BLOB)", (-900, "0A000".to_owned(), None)),
        ("VALUES JSON_QUERY('1', 'lax $' RETURNING CLOB FORMAT JSON ENCODING UTF16)", broke(-103, "2A000", "§10.13 SR2a")),
        ("VALUES JSON_QUERY('1', 'lax $' WITH WRAPPER KEEP QUOTES)", broke(-103, "2A000", "§6.35 SR8")),
        ("VALUES JSON_QUERY('1', 'lax $' WITH CONDITIONAL WRAPPER OMIT QUOTES)", broke(-103, "2A000", "§6.35 SR8")),
        ("VALUES JSON_VALUE('1', 'lax $A' PASSING X'00' AS A)", broke(-103, "2A000", "§10.12 SR7")),
        ("VALUES JSON_VALUE('1', 'lax $A' PASSING X'00' FORMAT JSON AS A)", (-900, "0A000".to_owned(), None)),
        ("VALUES JSON_VALUE('1', 'lax $A' PASSING ARRAY[1] AS A)", broke(-103, "2A000", "§10.12 SR3")),
        ("VALUES JSON_VALUE('1', 'lax $A' PASSING 1 FORMAT JSON AS A)", broke(-103, "2A000", "§10.12 SR6")),
        ("VALUES JSON_VALUE('1', 'lax $A' PASSING '1' FORMAT JSON ENCODING UTF32 AS A)", broke(-103, "2A000", "§10.12 SR8")),
        ("VALUES JSON_VALUE('1', 'lax $' RETURNING INTEGER DEFAULT DATE '2024-01-01' ON EMPTY)", cast_rule.clone()),
        ("VALUES JSON_VALUE('1', 'lax $' RETURNING DATE DEFAULT 1 ON ERROR)", cast_rule),
        ("VALUES JSON_VALUE('1', '$')", broke(-102, "2A000", "§9.46 F")),
        ("VALUES JSON_VALUE('1', 'lax @')", broke(-103, "2A000", "§9.46 SR5")),
    ];
    for (sql, refusal) in cases {
        assert_eq!(refused(&s, sql), refusal, "{sql}");
        assert!(s.prepare(sql).is_err(), "{sql} is refused as it is prepared");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.35, §6.28, §8.23, §10.12,
/// §10.13] The Format's errors: a path specification that is no literal,
/// a FORMAT of another representation than JSON, an ENCODING of none of
/// UTF8, UTF16 and UTF32, ON of neither EMPTY nor ERROR, EMPTY of neither
/// ARRAY nor OBJECT, a PASSING argument with no identifier, WRAPPER or
/// QUOTES missing, are each SQLCODE -101 or -102.
#[test]
fn syntax_6_35() {
    let db = setup();
    let s = db.session("APP");
    for bad in [
        "VALUES JSON_VALUE('1', NAME)",
        "VALUES JSON_VALUE('1' FORMAT XML, 'lax $')",
        "VALUES JSON_VALUE('1' FORMAT JSON ENCODING UTF7, 'lax $')",
        "VALUES JSON_VALUE('1', 'lax $' NULL ON NOTHING)",
        "VALUES JSON_QUERY('1', 'lax $' EMPTY LIST ON EMPTY)",
        "VALUES JSON_VALUE('1', 'lax $A' PASSING 1 AS 2)",
        "VALUES JSON_QUERY('1', 'lax $' WITH ARRAY)",
        "VALUES JSON_QUERY('1', 'lax $' KEEP)",
        "VALUES JSON_QUERY('1', 'lax $' KEEP QUOTES ON SCALAR)",
        "VALUES JSON_EXISTS('1', 'lax $' TRUE ON EMPTY)",
        "VALUES JSON_VALUE('1', 'lax $' NULL ON EMPTY NULL ON EMPTY)",
    ] {
        let code = failed(&s, bad).sqlcode();
        assert!(code == -101 || code == -102, "{bad}: {code}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.48 GR4b iii 2, §9.49 GR5b
/// i; ISO/IEC 9075:1992 §3.3.4.1] A cast that cuts a string to the
/// returned type completes with the warning 01004, as CAST does: of
/// JSON_VALUE, and of JSON_QUERY OMIT QUOTES.
#[test]
fn warnings_9_48() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, r#"VALUES JSON_VALUE('{"a":"abcdef"}', 'lax $.a' RETURNING VARCHAR(3))"#), expected(&["abc"]));
    assert_eq!(s.warning(), Some(SqlState::STRING_TRUNCATED));
    assert_eq!(rows(&s, r#"VALUES JSON_QUERY('"abcdef"', 'lax $' RETURNING CHAR(2) OMIT QUOTES)"#), expected(&["ab"]));
    assert_eq!(s.warning(), Some(SqlState::STRING_TRUNCATED));
    assert_eq!(rows(&s, r#"VALUES JSON_VALUE('{"a":"ab"}', 'lax $.a' RETURNING VARCHAR(3))"#), expected(&["ab"]));
    assert_eq!(s.warning(), None);
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.23, §6.28, §6.35; §20.7
/// GR5a] The operators where an expression may be: a query's select list
/// and WHERE, a view, a CHECK constraint, which refuses a row it is false
/// of, and a generated column; a `?` context item or argument FORMAT JSON
/// is CHARACTER VARYING, a DEFAULT the returned type, and an argument
/// without FORMAT JSON refused; two calls of the same operands but other
/// clauses are two expressions, as two sums of them.
#[test]
fn where_written_8_23() {
    let db = setup();
    let s = db.session("APP");
    let table = "CREATE TABLE DOCS (ID INTEGER, J VARCHAR(100) CHECK (JSON_EXISTS(J, 'strict $.id' ERROR ON ERROR)), \
                 N INTEGER GENERATED ALWAYS AS (JSON_VALUE(J, 'lax $.n' RETURNING INTEGER)))";
    exec(&s, table).unwrap();
    exec(&s, r#"INSERT INTO DOCS (ID, J) VALUES (1, '{"id":1,"n":5,"tags":["a","b"]}'), (2, '{"id":2,"n":7}')"#).unwrap();
    assert_eq!(s.run(r#"INSERT INTO DOCS (ID, J) VALUES (3, '{"n":7}')"#).unwrap_err().sqlstate().as_str(), "2203A");
    exec(&s, "CREATE VIEW TAGGED AS SELECT ID, N, JSON_QUERY(J, 'lax $.tags' RETURNING VARCHAR(20)) AS TAGS FROM DOCS").unwrap();
    assert_eq!(rows(&s, "SELECT ID, N, TAGS FROM TAGGED ORDER BY ID"), expected(&[r#"1,5,["a","b"]"#, "2,7,NULL"]));
    assert_eq!(rows(&s, "SELECT ID FROM DOCS WHERE JSON_EXISTS(J, 'lax $.tags')"), expected(&["1"]));
    let q = "SELECT SUM(JSON_VALUE(J, 'lax $.n' RETURNING INTEGER)), SUM(JSON_VALUE(J, 'lax $.id' RETURNING INTEGER)), \
             SUM(JSON_VALUE(J, 'lax $.n' RETURNING INTEGER DEFAULT 1 ON EMPTY)) FROM DOCS";
    assert_eq!(rows(&s, q), expected(&["12,3,12"]));
    exec(&s, "CREATE TABLE CK (J VARCHAR(20) CHECK (JSON_VALUE(J, 'lax $.n' RETURNING INTEGER) > 0))").unwrap();
    exec(&s, r#"INSERT INTO CK VALUES ('{"n":1}')"#).unwrap();
    assert_eq!(s.run(r#"INSERT INTO CK VALUES ('{"n":0}')"#).unwrap_err().sqlcode(), -201);
    let p = s.prepare("VALUES JSON_VALUE(?, 'lax $.a' PASSING ? FORMAT JSON AS B RETURNING INTEGER DEFAULT ? ON EMPTY)").unwrap();
    let text = |t: &str| Value::Char(t.to_owned());
    let mut c = s.open_prepared(&p, &[text(r#"{"a":3}"#), text("1"), text("9")]).unwrap();
    assert_eq!(c.fetch().unwrap().map(|r| r[0].to_string()), Some("3".to_owned()));
    let mut c = s.open_prepared(&p, &[text("{}"), text("1"), text("9")]).unwrap();
    assert_eq!(c.fetch().unwrap().map(|r| r[0].to_string()), Some("9".to_owned()));
    assert!(s.prepare("VALUES JSON_VALUE('{}', 'lax $B' PASSING ? AS B)").is_err());
}

/// Verifies: [ISO/IEC 9075-2:2023 §5.2] JSON_EXISTS, JSON_VALUE
/// and JSON_QUERY are the operators only before "(", and the words of
/// their Formats, no key words of SQL-92, only where the Formats have
/// them: so columns of those names are columns, in a WHERE clause too.
/// MC/DC of `at_json` of JSON_VALUE: JSON_VALUE before `(` the operator,
/// before `+` a column, and JSON_QUERY before `(` not JSON_VALUE.
#[test]
fn words_stay_identifiers_5_2() {
    let db = setup();
    let s = db.session("APP");
    exec(
        &s,
        "CREATE TABLE ODD (JSON_VALUE INTEGER, JSON_EXISTS INTEGER, JSON_QUERY INTEGER, ERROR INTEGER, PASSING INTEGER, EMPTY INTEGER)",
    )
    .unwrap();
    exec(&s, "INSERT INTO ODD VALUES (1, 2, 3, 4, 5, 6)").unwrap();
    let q = "SELECT JSON_VALUE + JSON_QUERY + ERROR + PASSING + EMPTY, JSON_VALUE('[7]', 'lax $[0]' RETURNING INTEGER DEFAULT ERROR ON EMPTY) \
             FROM ODD WHERE JSON_EXISTS = 2";
    assert_eq!(rows(&s, q), expected(&["19,7"]));
    let q = "SELECT JSON_VALUE('{}', 'lax $.a' RETURNING INTEGER DEFAULT ERROR ON EMPTY DEFAULT EMPTY ON ERROR) FROM ODD";
    assert_eq!(rows(&s, q), expected(&["4"]));
    assert_eq!(rows(&s, "SELECT JSON_QUERY('[8]', 'lax $[0]') FROM ODD"), expected(&["8"]));
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.45 GR1; §9.46 GR11g ii 6
/// C IX to XIX; Features T840, T865 to T878] A statement's path reads hex
/// integer literals and the casting item methods: integer(), decimal() of
/// a precision and scale, date(), boolean() and string() cast an item by
/// CAST, and an item a method does not take is its exception, 2202W of
/// date() and 2202V of boolean(), which ON ERROR replaces.
#[test]
fn item_methods_t865() {
    let db = setup();
    let s = db.session("APP");
    let q = r#"VALUES (JSON_VALUE('{}', 'lax 0x1F' RETURNING INTEGER), JSON_VALUE('{"a":"12"}', 'lax $.a.integer()' RETURNING INTEGER) + 1,
             JSON_VALUE('"1.256"', 'lax $.decimal(4,2)' RETURNING DECIMAL(4,2)), JSON_VALUE('"2024-02-29"', 'lax $.date()' RETURNING DATE),
             JSON_VALUE('"true"', 'lax $.boolean()' RETURNING BOOLEAN), JSON_VALUE('12', 'lax $.string()') || '!',
             JSON_VALUE('true', 'lax $.date()'))"#;
    assert_eq!(rows(&s, q), expected(&["31,13,1.25,2024-02-29,TRUE,12!,NULL"]));
    let fails = |sql: &str| failed(&s, sql).sqlstate().as_str().to_owned();
    assert_eq!(fails("VALUES JSON_VALUE('true', 'lax $.date()' ERROR ON ERROR)"), "2202W");
    assert_eq!(fails("VALUES JSON_VALUE('1', 'lax $.boolean()' ERROR ON ERROR)"), "2202V");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 SR10, GR11g ii 6 C VII 1 b; §9.51
/// GR5 to GR12; Feature T832] A statement's path reads a string by the
/// `<JSON datetime template>` of datetime(), as a value of the template's
/// implicit datetime data type, cast to the RETURNING type; a string the
/// template does not read is 22031, which ON ERROR replaces; a template
/// that breaks SR10 or a Syntax Rule of §9.52 is refused as the statement
/// is prepared, SQLCODE -103, whatever ON ERROR says.
#[test]
fn datetime_templates_t832() {
    let db = setup();
    let s = db.session("APP");
    let q = r#"VALUES (JSON_VALUE('"2024-02-29"', 'lax $.datetime("YYYY-MM-DD")' RETURNING DATE),
             JSON_VALUE('["29.02.2024 21:30"]', 'lax $.datetime("DD.MM.YYYY HH24:MI")' RETURNING TIMESTAMP(0)),
             JSON_VALUE('"09:30 P.M."', 'lax $.datetime("HH12:MI A.M.").type()'),
             JSON_VALUE('"2024/02/29"', 'lax $.datetime("YYYY-MM-DD")' RETURNING DATE))"#;
    assert_eq!(rows(&s, q), expected(&["2024-02-29,2024-02-29 21:30:00,time without time zone,NULL"]));
    let unread = "VALUES JSON_VALUE('\"2023-02-29\"', 'lax $.datetime(\"YYYY-MM-DD\")' RETURNING DATE ERROR ON ERROR)";
    assert_eq!(refused(&s, unread), broke(-301, "22031", "§9.46 GR11g ii 6 C VII 1 b"));
    let zoned = "VALUES JSON_VALUE('\"2024\"', 'lax $.datetime(\"YYYY TZH\")' NULL ON ERROR)";
    assert_eq!(refused(&s, zoned), broke(-103, "2A000", "§9.46 SR10d"));
    let lexical = "VALUES JSON_VALUE('\"2024\"', 'lax $.datetime(\"yyyy\")' NULL ON ERROR)";
    assert_eq!(refused(&s, lexical), broke(-103, "2A000", "§9.52 SR2"));
}
