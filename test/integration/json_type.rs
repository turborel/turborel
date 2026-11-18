//! The JSON type [ISO/IEC 9075-2:2023 §4.8; Features T801, T802, T851,
//! T860 to T864, T879 to T882; RULES.md D89]: columns of it, its
//! values made by JSON ( ), JSON_SCALAR, the constructors and JSON_QUERY,
//! read by the query operators and the simplified accessor, serialized by
//! JSON_SERIALIZE and CAST; compared, grouped, ordered and kept distinct;
//! kept across a restart and a failure; and passed as parameters.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::value::json::Json;
use turborel::value::{DataType, Value};

use crate::common::Scratch;

/// Each row of `sql`'s result, its values shown and joined by commas.
fn rows(s: &Session, sql: &str) -> Vec<String> {
    let mut c = s.open_cursor(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap_or_else(|e| panic!("{sql}: {e}")) {
        out.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    out
}

/// The SQLCODE and SQLSTATE of the error `sql` raises, prepared or run:
/// a query opened and read, another statement run.
fn fails(s: &Session, sql: &str) -> (i32, String) {
    let query = sql.starts_with("SELECT") || sql.starts_with("VALUES");
    let read = if query {
        s.open_cursor(sql).and_then(|mut c| {
            while c.fetch()?.is_some() {}
            Ok(())
        })
    } else {
        s.run(sql).map(drop)
    };
    let e = read.expect_err(sql);
    (e.sqlcode(), e.sqlstate().as_str().to_owned())
}

/// Schema A: DOCS of a key and a JSON column, of four documents and a
/// null.
fn docs() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE DOCS (ID INTEGER PRIMARY KEY, J JSON)").unwrap();
    let a = db.session("A");
    a.run(
        r#"INSERT INTO DOCS VALUES (1, JSON('{"name":"Ada","tags":["x","y"],"n":1}')), (3, JSON('[1,2,3]')),
           (4, JSON_SCALAR('plain')), (5, NULL)"#,
    )
    .unwrap();
    a.run(r#"INSERT INTO DOCS VALUES (2, '{"name":"Bob","n":2.50}')"#).unwrap();
    a.commit().unwrap();
    (db, a)
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.1, §4.8.1, §6.40, §6.41; Features
/// T801] a column of type JSON takes the values JSON ( ) and JSON_SCALAR
/// make, and, \[EXTENSION\], a character string's JSON text; its
/// descriptor is JSON, in the information schema too; a value is shown as
/// its JSON text, its members in the order written.
#[test]
fn json_columns_hold_json_values_4_8() {
    let (_db, a) = docs();
    assert_eq!(
        rows(&a, "SELECT ID, J FROM DOCS ORDER BY ID"),
        [r#"1,{"name":"Ada","tags":["x","y"],"n":1}"#, r#"2,{"name":"Bob","n":2.50}"#, "3,[1,2,3]", r#"4,"plain""#, "5,NULL"]
    );
    let q = "SELECT DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'DOCS' AND COLUMN_NAME = 'J'";
    assert_eq!(rows(&a, q), ["JSON"]);
    assert_eq!(fails(&a, "INSERT INTO DOCS VALUES (6, 'not json')"), (-301, "22032".to_owned()));
    assert_eq!(fails(&a, "INSERT INTO DOCS VALUES (6, 1)").0, -103);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.40 SR4, GR3; §9.42; Feature T802]
/// JSON ( ) parses WITHOUT UNIQUE KEYS unless WITH UNIQUE KEYS is
/// written, FORMAT JSON optional (T851); of the null value the null
/// value; of no JSON text SQLSTATE 22032, of two members of one key WITH
/// UNIQUE KEYS 22037.
#[test]
fn json_parse_6_40() {
    let (_db, a) = docs();
    assert_eq!(
        rows(&a, r#"VALUES (JSON('{"a":1,"a":2}'), JSON('[true]' FORMAT JSON), JSON(CAST(NULL AS VARCHAR(5))))"#),
        [r#"{"a":1,"a":2},[true],NULL"#]
    );
    assert_eq!(rows(&a, r#"VALUES JSON('{"a":1}' WITH UNIQUE KEYS)"#), [r#"{"a":1}"#]);
    assert_eq!(fails(&a, r#"VALUES JSON('{"a":1,"a":2}' WITH UNIQUE KEYS)"#).1, "22037");
    assert_eq!(fails(&a, "VALUES JSON('[1,')").1, "22032");
    assert_eq!(fails(&a, "VALUES JSON(1)").0, -103);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.41 SR2, GR2] JSON_SCALAR makes a
/// scalar of a character string, a number, a truth value or a datetime,
/// the null value of the null value, no SQL/JSON null; refuses a binary
/// string, SQLCODE -103. A datetime scalar has no JSON text [§9.43 GR2a
/// ii], so JSON_SERIALIZE of it raises 22032.
#[test]
fn json_scalar_6_41() {
    let (_db, a) = docs();
    let q = "VALUES (JSON_SCALAR('a\"b'), JSON_SCALAR(12.50), JSON_SCALAR(1.5E0), JSON_SCALAR(TRUE), JSON_SCALAR(CAST(NULL AS INTEGER)))";
    assert_eq!(rows(&a, q), [r#""a\"b",12.50,1.5E0,true,NULL"#]);
    assert_eq!(rows(&a, "VALUES JSON_VALUE(JSON_SCALAR(DATE '2024-02-29'), 'lax $' RETURNING DATE)"), ["2024-02-29"]);
    assert_eq!(fails(&a, "VALUES JSON_SERIALIZE(JSON_SCALAR(DATE '2024-02-29'))").1, "22032");
    assert_eq!(fails(&a, "VALUES JSON_SCALAR(X'01')").0, -103);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.37, §6.38 SR1, §9.43] JSON_SERIALIZE
/// gives a value's JSON text, of CHARACTER VARYING(32767) or the type
/// RETURNING names, FORMAT JSON optional (T851); 22001 if longer than the
/// type; RETURNING JSON and an operand of no JSON type are refused,
/// SQLCODE -103.
#[test]
fn json_serialize_6_37() {
    let (_db, a) = docs();
    assert_eq!(rows(&a, "SELECT JSON_SERIALIZE(J) FROM DOCS WHERE ID = 3"), ["[1,2,3]"]);
    assert_eq!(rows(&a, "SELECT JSON_SERIALIZE(J RETURNING VARCHAR(10) FORMAT JSON) || '!' FROM DOCS WHERE ID = 3"), ["[1,2,3]!"]);
    assert_eq!(rows(&a, "SELECT JSON_SERIALIZE(J) FROM DOCS WHERE ID = 5"), ["NULL"]);
    assert_eq!(fails(&a, "SELECT JSON_SERIALIZE(J RETURNING CHAR(3)) FROM DOCS WHERE ID = 3").1, "22001");
    assert_eq!(fails(&a, "VALUES JSON_SERIALIZE(JSON('1') RETURNING JSON)").0, -103);
    assert_eq!(fails(&a, "VALUES JSON_SERIALIZE('[1]')").0, -103);
    let q = "CREATE VIEW V AS SELECT JSON_SERIALIZE(J) AS S, JSON('1') AS P FROM DOCS";
    a.run(q).unwrap();
    let q = "SELECT COLUMN_NAME, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'V' ORDER BY 1";
    assert_eq!(rows(&a, q), ["P,JSON,NULL", "S,CHARACTER VARYING,32767"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.13 Table 6, GR22] CAST takes JSON to
/// JSON; \[EXTENSION\] a character string to JSON, parsed, and JSON to a
/// character string, its JSON text; any other type to or from JSON is
/// refused as the statement is prepared, SQLCODE -103.
#[test]
fn json_casts_6_13() {
    let (_db, a) = docs();
    let q = r#"VALUES (CAST('{"a" : [1, 2]}' AS JSON), CAST(JSON('[1]') AS VARCHAR(9)) || '!', CAST(JSON('2') AS JSON))"#;
    assert_eq!(rows(&a, q), [r#"{"a":[1,2]},[1]!,2"#]);
    assert_eq!(fails(&a, "VALUES CAST(JSON('1') AS INTEGER)").0, -103);
    assert_eq!(fails(&a, "VALUES CAST(1 AS JSON)").0, -103);
    assert_eq!(fails(&a, "VALUES CAST('[' AS JSON)").1, "22032");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.14, §10.12 SR5, §9.47; §6.35 SR1a,
/// SR3, GR5a; §6.28] JSON_VALUE, JSON_QUERY and JSON_EXISTS read a value
/// of the JSON type, FORMAT JSON implicit or written (T851); JSON_QUERY of
/// one returns JSON unless RETURNING says otherwise, its sequence made an
/// item [§9.44], and OMIT QUOTES then is refused, SQLCODE -103; a PASSING
/// argument of JSON is its item.
#[test]
fn query_operators_read_json_values() {
    let (_db, a) = docs();
    let q = "SELECT ID, JSON_VALUE(J, 'lax $.name'), JSON_VALUE(J FORMAT JSON, 'lax $.n' RETURNING DECIMAL(5,2)), \
             JSON_EXISTS(J, 'lax $.tags[1]') FROM DOCS WHERE ID < 4 ORDER BY ID";
    assert_eq!(rows(&a, q), ["1,Ada,1.00,TRUE", "2,Bob,2.50,FALSE", "3,NULL,NULL,FALSE"]);
    let q = "SELECT JSON_QUERY(J, 'lax $.tags'), JSON_QUERY(J, 'lax $.tags[*]' WITH WRAPPER), \
             JSON_QUERY(J, 'lax $.tags' RETURNING VARCHAR(20)) FROM DOCS WHERE ID = 1";
    assert_eq!(rows(&a, q), [r#"["x","y"],["x","y"],["x","y"]"#]);
    a.run("CREATE VIEW Q AS SELECT JSON_QUERY(J, 'lax $.tags') AS T FROM DOCS").unwrap();
    let q = "SELECT DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'Q'";
    assert_eq!(rows(&a, q), ["JSON"]);
    assert_eq!(fails(&a, "SELECT JSON_QUERY(J, 'lax $' OMIT QUOTES) FROM DOCS").0, -103);
    let q = r#"VALUES JSON_VALUE('{"a":5}', 'lax $.a ? (@ == $X.k)' PASSING JSON('{"k":5}') AS X)"#;
    assert_eq!(rows(&a, q), ["5"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.22 GR2b] IS JSON of a value of the
/// JSON type is not parsed: True of its kind, WITH UNIQUE KEYS too, False
/// of another kind, Unknown of the null value.
#[test]
fn is_json_of_json_values_8_22() {
    let (_db, a) = docs();
    let q = "SELECT ID, J IS JSON, J IS JSON OBJECT WITH UNIQUE KEYS, J IS JSON ARRAY, J IS NOT JSON SCALAR FROM DOCS ORDER BY ID";
    assert_eq!(
        rows(&a, q),
        ["1,TRUE,TRUE,FALSE,TRUE", "2,TRUE,TRUE,FALSE,TRUE", "3,TRUE,FALSE,TRUE,TRUE", "4,TRUE,FALSE,FALSE,FALSE", "5,NULL,NULL,NULL,NULL"]
    );
    assert_eq!(rows(&a, r#"VALUES JSON('{"a":1,"a":2}') IS JSON WITH UNIQUE KEYS"#), ["TRUE"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 SR2a, GR2d ii 1 F II, GR5b;
/// §10.11 SR1a] a constructor or a JSON aggregate of a JSON value returns
/// JSON unless RETURNING says otherwise, the value its item, not text;
/// RETURNING JSON is written too (T851 FORMAT JSON).
#[test]
fn constructors_return_json_6_34() {
    let (_db, a) = docs();
    let q = "SELECT JSON_OBJECT('d': J), JSON_ARRAY(J, 'x') FROM DOCS WHERE ID = 3";
    assert_eq!(rows(&a, q), [r#"{"d":[1,2,3]},[[1,2,3],"x"]"#]);
    a.run(
        "CREATE VIEW C AS SELECT JSON_ARRAY(J) AS J1, JSON_ARRAY(ID) AS J2, JSON_ARRAY(ID RETURNING JSON FORMAT JSON) AS J3, \
           JSON_ARRAYAGG(J ORDER BY ID) AS J4, JSON_OBJECTAGG(CAST(ID AS VARCHAR(2)) VALUE J) AS J5 FROM DOCS GROUP BY ID",
    )
    .unwrap();
    let q = "SELECT COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'C' ORDER BY 1";
    assert_eq!(rows(&a, q), ["J1,JSON", "J2,CHARACTER VARYING", "J3,JSON", "J4,JSON", "J5,JSON"]);
    assert_eq!(rows(&a, "SELECT JSON_ARRAYAGG(J ORDER BY ID) FROM DOCS WHERE ID IN (3, 4)"), [r#"[[1,2,3],"plain"]"#]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.36 SR2, SR3; §6.6 SR6, SR11;
/// Features T860 to T864] the simplified accessor of a JSON column, by
/// its identifier chain (a delimited identifier its key as written, a
/// regular one upper case), of a table's column, of an element reference
/// and of an item method, typed or not, and the ops SQL has none of, `.*`,
/// `[*]`, a quoted key and `last`, read as written; JSON_QUERY WITH
/// CONDITIONAL ARRAY WRAPPER NULL ON EMPTY NULL ON ERROR, of the JSON
/// type, or JSON_VALUE of the method's SQL type.
#[test]
fn simplified_accessor_6_36() {
    let (_db, a) = docs();
    a.run(r#"INSERT INTO DOCS VALUES (6, JSON('{"NAME":"upper","o":{"p":[10,20,30]}}'))"#).unwrap();
    let q = r#"SELECT D.J."name", DOCS.J.NAME, DOCS.J."tags"[1], D.J."tags".size(), D.J."n".double() FROM DOCS D, DOCS WHERE D.ID = DOCS.ID AND DOCS.ID IN (1, 6) ORDER BY DOCS.ID"#;
    let got = rows(&a, q);
    assert_eq!(got, [r#""Ada",NULL,"y",2,1E0"#, "NULL,\"upper\",NULL,NULL,NULL"]);
    let q = "SELECT J.'o'.'p'[last], J.'o'.*, J.'o'.'p'[*], J.'o'.'p'[0 to 1] FROM DOCS WHERE ID = 6";
    assert_eq!(rows(&a, q), ["30,[10,20,30],[10,20,30],[10,20]"]);
    a.run("CREATE VIEW ACC AS SELECT J.\"tags\" AS T, J.\"tags\".size() AS S, J.\"n\".string() AS X FROM DOCS").unwrap();
    let q = "SELECT COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'ACC' ORDER BY 1";
    assert_eq!(rows(&a, q), ["S,BIGINT", "T,JSON", "X,CHARACTER VARYING"]);
    assert_eq!(fails(&a, "SELECT ID.'a' FROM DOCS").0, -103);
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.2 GR10, §4.8.2; Features T879,
/// T880] values of the JSON type compare: equal of equivalent items,
/// whatever their members' order or the form of a number; ordered null,
/// numbers, strings, truth values, arrays, objects; with no other type,
/// SQLCODE -103.
#[test]
fn json_values_compare_8_2() {
    let (_db, a) = docs();
    let q = r#"VALUES (JSON('{"a":1,"b":[2]}') = JSON('{"b":[2.0],"a":1.00}'), JSON('1') < JSON('"0"'),
             JSON('"a"') < JSON('"a "'), JSON('true') < JSON('[]'), JSON('[1,2]') < JSON('[2]'), JSON('[]') < JSON('{}'),
             JSON('null') < JSON('-1E+300'), JSON('"x"') <> JSON('"x "'))"#;
    assert_eq!(rows(&a, q), ["TRUE,TRUE,TRUE,TRUE,TRUE,TRUE,TRUE,TRUE"]);
    assert_eq!(fails(&a, "SELECT ID FROM DOCS WHERE J = 1").0, -103);
    assert_eq!(fails(&a, "SELECT ID FROM DOCS WHERE J = '[1,2,3]'").0, -103);
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.8.2, §7.12, §7.17, §7.13; Features
/// T879 to T882] GROUP BY, DISTINCT, ORDER BY, UNION, a multiset's
/// grouping and an index key take JSON values by their comparison: equal
/// values one group, in the order §8.2 gives.
#[test]
fn json_values_group_and_order() {
    let (_db, a) = docs();
    a.run(r#"INSERT INTO DOCS VALUES (7, JSON('{"n":1,"tags":["x","y"],"name":"Ada"}')), (8, JSON('[1.0,2,3]'))"#).unwrap();
    let q = "SELECT J, COUNT(*) FROM DOCS GROUP BY J ORDER BY J";
    assert_eq!(
        rows(&a, q),
        [r#""plain",1"#, "[1,2,3],2", r#"{"name":"Ada","tags":["x","y"],"n":1},2"#, r#"{"name":"Bob","n":2.50},1"#, "NULL,1"]
    );
    assert_eq!(rows(&a, "SELECT COUNT(DISTINCT J) FROM DOCS"), ["4"]);
    assert_eq!(rows(&a, "SELECT J FROM DOCS WHERE ID = 3 UNION SELECT J FROM DOCS WHERE ID = 8"), ["[1,2,3]"]);
    assert_eq!(rows(&a, "SELECT ID FROM DOCS ORDER BY J DESC NULLS LAST, ID FETCH FIRST 2 ROWS ONLY"), ["2", "1"]);
    assert_eq!(rows(&a, "VALUES CARDINALITY(SET(MULTISET[JSON('[1]'), JSON('[1.0]'), JSON('2')]))"), ["2"]);
    a.run("CREATE TABLE K (J JSON PRIMARY KEY)").unwrap();
    a.run(r#"INSERT INTO K VALUES (JSON('{"a":1,"b":2}'))"#).unwrap();
    let (code, _) = fails(&a, r#"INSERT INTO K VALUES (JSON('{"b":2,"a":1.0}'))"#);
    assert_ne!(code, 0);
    assert_eq!(rows(&a, r#"SELECT J FROM K WHERE J = JSON('{"b":2,"a":1}')"#), [r#"{"a":1,"b":2}"#]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §20.7; RULES.md D89] a `?` of
/// a JSON site is of the JSON type and takes a value of it or a
/// character string's JSON text; a JSON value is a result read whole.
#[test]
fn json_parameters() {
    let (_db, a) = docs();
    let p = a.prepare("INSERT INTO DOCS VALUES (?, ?)").unwrap();
    assert_eq!(p.parameters().get(1).map(|t| t.clone().data_type()), Some(DataType::Json));
    let made = Json::parse(r#"{"k":[true,null]}"#, turborel::json::Keys::Any).unwrap();
    a.execute_prepared(&p, &[Value::Exact(turborel::value::Decimal::new(9, 0)), Value::Json(made.clone())]).unwrap();
    a.execute_prepared(&p, &[Value::Exact(turborel::value::Decimal::new(10, 0)), Value::Char("[4]".to_owned())]).unwrap();
    let q = a.prepare("SELECT J FROM DOCS WHERE J = ?").unwrap();
    let mut c = a.open_prepared(&q, &[Value::Json(made.clone())]).unwrap();
    let row = c.fetch().unwrap().unwrap();
    assert_eq!(row.first().map(ToString::to_string), Some(made.to_string()));
    assert_eq!(rows(&a, "SELECT J FROM DOCS WHERE ID = 10"), ["[4]"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.41 GR2b; ISO/IEC 9075:1992 §17.6
/// GR5] JSON_SCALAR of a `?` cast to a type is the scalar of the value it
/// is given, cast to that type: 3 given CAST(? AS INTEGER) is the JSON
/// number 3, not a data exception 22005 of a cast to a number of no
/// digits, and 2.5 given CAST(? AS DECIMAL(3,1)) the number 2.5.
#[test]
fn json_scalar_of_a_cast_parameter() {
    let (_db, a) = docs();
    let read = |sql: &str, arg: Value| {
        let p = a.prepare(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
        let mut c = a.open_prepared(&p, &[arg]).unwrap_or_else(|e| panic!("{sql}: {e}"));
        c.fetch().unwrap().unwrap().first().map(ToString::to_string)
    };
    let three = Value::Exact(turborel::value::Decimal::new(3, 0));
    assert_eq!(read("VALUES JSON_SCALAR(CAST(? AS INTEGER))", three).as_deref(), Some("3"));
    let half = Value::Exact(turborel::value::Decimal::new(25, 1));
    assert_eq!(read("VALUES JSON_SERIALIZE(JSON_SCALAR(CAST(? AS DECIMAL(3,1))))", half).as_deref(), Some("2.5"));
}

/// Verifies: [DESIGN] RULES.md D89, D25: JSON values, short and
/// long, kept in a directory are read back after the database closes and
/// after a failure from its log alone; a long one beyond a page and the
/// 32767 characters of a string type.
#[test]
fn json_values_survive_restart_and_failure() {
    let long = format!("[{}]", (0..20_000).map(|i| i.to_string()).collect::<Vec<_>>().join(","));
    for closed in [true, false] {
        let scratch = Scratch::new(if closed { "json-closed" } else { "json-failed" });
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let db = Database::new();
        db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE D (ID INTEGER PRIMARY KEY, J JSON)").unwrap();
        let a = db.session("A");
        a.run(r#"INSERT INTO D VALUES (1, JSON('{"b":1,"a":[true]}'))"#).unwrap();
        a.run(&format!("INSERT INTO D VALUES (2, JSON(CAST('{long}' AS CLOB(1M))))")).unwrap();
        a.commit().unwrap();
        if closed {
            db.close().unwrap();
        }
        drop(a);
        drop(db);
        let db = Database::new();
        db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        let a = db.session("A");
        assert_eq!(rows(&a, "SELECT J FROM D WHERE ID = 1"), [r#"{"b":1,"a":[true]}"#]);
        assert_eq!(rows(&a, "SELECT J.size(), JSON_VALUE(J, 'lax $[19999]') FROM D WHERE ID = 2"), ["20000,19999"]);
        a.commit().unwrap();
        db.close().unwrap();
    }
}
