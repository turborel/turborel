//! The SQL/JSON constructor functions and the `<JSON predicate>` [ISO/IEC
//! 9075-2:2023 §4.48.4; RULES.md D84]: JSON_OBJECT and JSON_ARRAY
//! [§6.34], JSON_OBJECTAGG and JSON_ARRAYAGG [§10.11] and IS JSON [§8.22],
//! with their null clauses, key uniqueness constraints and output
//! clauses, in statements.

use turborel::db::{Database, Session};

use crate::common::{Counted, broke, exec, expected, failed, refused, rows, setup, sorted};

/// Schema APP of `setup`, and P, of names, values and JSON texts, a null
/// name and a null value among them.
fn people() -> (Database, Session) {
    let db = setup();
    let s = db.session("APP");
    exec(&s, "CREATE TABLE P (ID INTEGER, N VARCHAR(10), V INTEGER, J VARCHAR(40))").unwrap();
    let rows = r#"INSERT INTO P VALUES (1, 'a', 10, '{"x":1}'), (2, 'b', NULL, '[1,2]'), (3, NULL, 30, 'oops'), (4, 'd', 40, NULL)"#;
    exec(&s, rows).unwrap();
    (db, s)
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 SR6, GR2, GR5a; §10.12 SR4a;
/// Features T811, T814] JSON_OBJECT is the JSON text of an object of its
/// members, written `KEY name VALUE value`, `name VALUE value` or `name :
/// value`, a value of a string, number or truth value kept, of another
/// type cast to a string, FORMAT JSON parsed, of a JSON-returning function
/// FORMAT JSON implicitly; of none, `{}`; NULL ON NULL, the default, keeps
/// a null value as null, ABSENT ON NULL leaves it out.
#[test]
fn json_object_6_34() {
    let (_db, s) = people();
    let q = "VALUES (JSON_OBJECT('a' VALUE 1, KEY 'b' VALUE 'x', 'c' : TRUE, 'd' : DATE '2024-01-02' RETURNING VARCHAR(60)),
             JSON_OBJECT(RETURNING VARCHAR(5)), JSON_OBJECT('j' VALUE '[1, 2]' FORMAT JSON, 'k' VALUE '[1]' RETURNING VARCHAR(30)),
             JSON_OBJECT('o' VALUE JSON_OBJECT('p' VALUE 1), 'q' VALUE JSON_ARRAY(2) RETURNING VARCHAR(30)))";
    assert_eq!(rows(&s, q), expected(&[r#"{"a":1,"b":"x","c":true,"d":"2024-01-02"},{},{"j":[1,2],"k":"[1]"},{"o":{"p":1},"q":[2]}"#]));
    let q = "SELECT ID, JSON_OBJECT('n' VALUE N, 'v' VALUE V RETURNING VARCHAR(30)), \
             JSON_OBJECT('n' VALUE N, 'v' VALUE V ABSENT ON NULL RETURNING VARCHAR(30)), \
             JSON_OBJECT('v' VALUE V NULL ON NULL WITHOUT UNIQUE KEYS RETURNING VARCHAR(30)) FROM P WHERE ID IN (2, 3) ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&[r#"2,{"n":"b","v":null},{"n":"b"},{"v":null}"#, r#"3,{"n":null,"v":30},{"v":30},{"v":30}"#]));
    assert_eq!(rows(&s, "VALUES JSON_OBJECT('a' VALUE 1, 'a' VALUE 2)"), expected(&[r#"{"a":1,"a":2}"#]));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR2d ii 1 D, GR2d ii 2, GR5a;
/// §9.43 GR2a iii; Feature T830] WITH UNIQUE KEYS makes two members of one
/// name 22030, one left out ABSENT ON NULL too, and a FORMAT JSON value of
/// an object of two members of one key 22037; a null name is 22004, a
/// text longer than the type returned 22001.
#[test]
fn json_object_errors_6_34() {
    let (_db, s) = people();
    let state = |sql: &str| failed(&s, sql).sqlstate().as_str().to_owned();
    assert_eq!(state("VALUES JSON_OBJECT('a' VALUE 1, 'a' VALUE 2 WITH UNIQUE KEYS)"), "22030");
    assert_eq!(state("VALUES JSON_OBJECT('a' VALUE 1, 'a' VALUE CAST(NULL AS INTEGER) ABSENT ON NULL WITH UNIQUE)"), "22030");
    assert_eq!(state(r#"VALUES JSON_OBJECT('a' VALUE '{"b":1,"b":2}' FORMAT JSON WITH UNIQUE KEYS)"#), "22037");
    assert_eq!(rows(&s, r#"VALUES JSON_OBJECT('a' VALUE '{"b":1,"b":2}' FORMAT JSON)"#), expected(&[r#"{"a":{"b":1,"b":2}}"#]));
    assert_eq!(state("SELECT JSON_OBJECT(N VALUE ID) FROM P"), "22004");
    assert_eq!(state("VALUES JSON_OBJECT('a' VALUE '[' FORMAT JSON)"), "22032");
    assert_eq!(state("VALUES JSON_OBJECT('abc' VALUE 1 RETURNING VARCHAR(5))"), "22001");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 SR7, GR3, GR4, GR5a; Feature T811]
/// JSON_ARRAY by enumeration is the JSON text of an array of its elements
/// in order, of none `[]`; ABSENT ON NULL, the default, leaves a null value
/// out, NULL ON NULL keeps it; by query, of the one column of its query's
/// rows in the order of its ORDER BY, of no rows `[]`; a lone scalar
/// subquery is by query \[SR7d\], two are elements.
#[test]
fn json_array_6_34() {
    let (_db, s) = people();
    let q = "VALUES (JSON_ARRAY(1, 'two', CAST(NULL AS INTEGER), '[3]' FORMAT JSON RETURNING VARCHAR(30)), JSON_ARRAY(RETURNING VARCHAR(5)),
             JSON_ARRAY(CAST(NULL AS INTEGER), 1 NULL ON NULL RETURNING VARCHAR(10)), JSON_ARRAY(JSON_ARRAY(1, 2), JSON_OBJECT('k' : 1) RETURNING VARCHAR(30)))";
    assert_eq!(rows(&s, q), expected(&[r#"[1,"two",[3]],[],[null,1],[[1,2],{"k":1}]"#]));
    let q = "VALUES (JSON_ARRAY(SELECT V FROM P ORDER BY ID DESC RETURNING VARCHAR(30)), \
             JSON_ARRAY(SELECT V FROM P ORDER BY ID NULL ON NULL RETURNING VARCHAR(30)), \
             JSON_ARRAY(SELECT V FROM P WHERE ID > 9 RETURNING VARCHAR(5)), \
             JSON_ARRAY((SELECT J FROM P WHERE ID = 2) FORMAT JSON RETURNING VARCHAR(9)), \
             JSON_ARRAY((SELECT V FROM P WHERE ID = 1), (SELECT V FROM P WHERE ID = 3) RETURNING VARCHAR(9)), \
             JSON_ARRAY(VALUES 1, 2 RETURNING VARCHAR(9)))";
    assert_eq!(rows(&s, q), expected(&["[40,30,10],[10,null,30,40],[],[[1,2]],[10,30],[1,2]"]));
    assert_eq!(failed(&s, "VALUES JSON_ARRAY(SELECT J FROM P AS Q FORMAT JSON)").sqlstate().as_str(), "22032");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.11 SR4, SR5, GR3 to GR6; Features
/// T811, T812, T813, T814, T830] JSON_OBJECTAGG and JSON_ARRAYAGG are the
/// JSON text of an object of each row's member, or an array of each row's
/// value, of the rows of the group the FILTER keeps; of no rows the null
/// value; JSON_ARRAYAGG in the order its ORDER BY gives, either direction,
/// ABSENT ON NULL leaving a null value out by default; NULL ON NULL
/// keeping it in an object by default; a null name 22004, two of one
/// name WITH UNIQUE KEYS 22030; over a window too. RULES.md R-TEST-8,
/// the pairs of JSON_ARRAYAGG's decision "ABSENT ON NULL and a null
/// value" (ABSENT and null: left out; NULL ON NULL and null: kept; ABSENT
/// and a number: kept) and of "no rows and no query" (no rows of
/// JSON_ARRAYAGG: null; no rows of JSON_ARRAY by query, of
/// `json_array_6_34`: `[]`; rows: an array).
#[test]
fn json_aggregates_10_11() {
    let (_db, s) = people();
    let q = "SELECT JSON_OBJECTAGG(N VALUE V RETURNING VARCHAR(60)), JSON_OBJECTAGG(N : V ABSENT ON NULL RETURNING VARCHAR(60)), \
             JSON_ARRAYAGG(V ORDER BY ID DESC RETURNING VARCHAR(30)), JSON_ARRAYAGG(V ORDER BY ID NULL ON NULL RETURNING VARCHAR(30)), \
             JSON_ARRAYAGG(J FORMAT JSON ORDER BY ID RETURNING VARCHAR(30)) FILTER (WHERE J IS JSON) FROM P WHERE N IS NOT NULL";
    assert_eq!(rows(&s, q), expected(&[r#"{"a":10,"b":null,"d":40},{"a":10,"d":40},[40,10],[10,null,40],[{"x":1},[1,2]]"#]));
    let q = "SELECT JSON_OBJECTAGG(N VALUE V), JSON_ARRAYAGG(V), JSON_ARRAYAGG(V ORDER BY ID) FROM P WHERE ID > 9";
    assert_eq!(rows(&s, q), expected(&["NULL,NULL,NULL"]));
    exec(&s, "INSERT INTO P VALUES (5, 'a', 50, NULL)").unwrap();
    let q = "SELECT N, JSON_ARRAYAGG(ID ORDER BY ID DESC RETURNING VARCHAR(9)) FROM P WHERE N IS NOT NULL GROUP BY N ORDER BY N";
    assert_eq!(rows(&s, q), expected(&["a,[5,1]", "b,[2]", "d,[4]"]));
    let q = "SELECT ID, JSON_ARRAYAGG(ID ORDER BY ID RETURNING VARCHAR(20)) OVER (ORDER BY ID) FROM P ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&["1,[1]", "2,[1,2]", "3,[1,2,3]", "4,[1,2,3,4]", "5,[1,2,3,4,5]"]));
    assert_eq!(failed(&s, "SELECT JSON_OBJECTAGG(N VALUE V) FROM P").sqlstate().as_str(), "22004");
    let q = "SELECT JSON_OBJECTAGG(N VALUE V WITH UNIQUE KEYS) FROM P WHERE N IS NOT NULL";
    assert_eq!(failed(&s, q).sqlstate().as_str(), "22030");
    assert_eq!(rows(&s, "SELECT JSON_ARRAYAGG(ID ORDER BY N DESC, ID DESC RETURNING VARCHAR(20)) FROM P"), expected(&["[3,4,2,5,1]"]));
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.11; RULES.md D38] JSON_OBJECTAGG and
/// JSON_ARRAYAGG of many groups give the same, past the work area, as in
/// memory.
#[test]
fn json_aggregates_past_the_work_area_10_11() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE M (G INTEGER, NAME VARCHAR(10), K INTEGER)").unwrap();
    let a = db.session("A");
    for first in (0..4000).step_by(500) {
        let values: Vec<String> = (first..first + 500).map(|i| format!("({}, 'n{}', {})", i % 997, i, i)).collect();
        a.run(&format!("INSERT INTO M VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    let sql = "SELECT G, JSON_ARRAYAGG(K ORDER BY K DESC RETURNING VARCHAR(100)), \
               JSON_OBJECTAGG(NAME VALUE K WITH UNIQUE KEYS RETURNING VARCHAR(200)) FROM M GROUP BY G";
    let in_memory = sorted(&a, sql);
    assert_eq!(in_memory.len(), 997);
    assert_eq!(counted.taken(), 0, "the default MEMORY.WORK_BYTES holds them");
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    assert_eq!(sorted(&a, sql), in_memory);
    assert!(counted.taken() > 0, "they spilled");
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.22 SR1 to SR4, GR; Features T821,
/// T822] IS JSON is True of a JSON text of the kind it names, VALUE by
/// default, False of one of another kind or of a string no JSON text,
/// and, WITH UNIQUE KEYS, of an object of two members of one key; Unknown
/// of the null value; IS NOT JSON its negation. It is a predicate where one
/// may be: a WHERE clause, a select list, a CHECK constraint, which it is
/// no potential source of non-determinism of, and a view WITH CHECK
/// OPTION, whose WITH it leaves.
#[test]
fn is_json_8_22() {
    let (_db, s) = people();
    let q = "SELECT ID, J IS JSON, J IS JSON VALUE, J IS JSON OBJECT, J IS NOT JSON ARRAY, J IS JSON SCALAR,
             J FORMAT JSON IS JSON WITHOUT UNIQUE KEYS FROM P ORDER BY ID";
    let shown = [
        "1,TRUE,TRUE,TRUE,TRUE,FALSE,TRUE",
        "2,TRUE,TRUE,FALSE,FALSE,FALSE,TRUE",
        "3,FALSE,FALSE,FALSE,TRUE,FALSE,FALSE",
        "4,NULL,NULL,NULL,NULL,NULL,NULL",
    ];
    assert_eq!(rows(&s, q), expected(&shown));
    let q = r#"VALUES ('{"a":1,"a":2}' IS JSON, '{"a":1,"a":2}' IS JSON WITH UNIQUE KEYS, '{"a":1,"a":2}' IS NOT JSON WITH UNIQUE,
             '1' IS JSON SCALAR, 'null' IS JSON SCALAR, '"s"' IS NOT JSON OBJECT)"#;
    assert_eq!(rows(&s, q), expected(&["TRUE,FALSE,TRUE,TRUE,TRUE,TRUE"]));
    assert_eq!(rows(&s, "SELECT ID FROM P WHERE J IS JSON ARRAY OR J IS NOT JSON ORDER BY ID"), expected(&["2", "3"]));
    exec(&s, "CREATE TABLE CK (J VARCHAR(20) CHECK (J IS JSON OBJECT))").unwrap();
    exec(&s, r#"INSERT INTO CK VALUES ('{"a":1}')"#).unwrap();
    assert_eq!(s.run("INSERT INTO CK VALUES ('[1]')").unwrap_err().sqlcode(), -201);
    exec(&s, "CREATE VIEW OBJECTS AS SELECT J FROM P WHERE J IS JSON OBJECT WITH CHECK OPTION").unwrap();
    assert_eq!(rows(&s, "SELECT J FROM OBJECTS"), expected(&[r#"{"x":1}"#]));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 F, SR7c; §8.22 F; §10.11; §10.12
/// SR3, SR6 to SR8; §10.13 SR2; §9.14 SR3] The binder's refusals, as the
/// statement is prepared: a name no character string, a value of a binary
/// string without FORMAT JSON or of an array, a value FORMAT JSON of no
/// string, ENCODING of a character string, a type returned of no string
/// (SQLCODE -900 of a binary one), a query of more columns than one, an
/// input of IS JSON of no string (-900 of a binary one), a sort key of an
/// array, of each function.
#[test]
fn constructor_refusals_10_12() {
    let (_db, s) = people();
    let unsupported = (-900, "0A000".to_owned(), None);
    let cases = [
        ("VALUES JSON_OBJECT(1 VALUE 2)", broke(-103, "2A000", "§6.34 F")),
        ("VALUES JSON_OBJECT('a' VALUE X'00')", broke(-103, "2A000", "§10.12 SR7")),
        ("VALUES JSON_OBJECT('a' VALUE ARRAY[1])", broke(-103, "2A000", "§10.12 SR3")),
        ("VALUES JSON_ARRAY(1 FORMAT JSON)", broke(-103, "2A000", "§10.12 SR6")),
        ("VALUES JSON_ARRAY('1' FORMAT JSON ENCODING UTF8)", broke(-103, "2A000", "§10.12 SR8")),
        ("VALUES JSON_ARRAY(1 RETURNING INTEGER)", broke(-103, "2A000", "§10.13 SR2")),
        ("VALUES JSON_OBJECT(RETURNING BLOB)", unsupported.clone()),
        ("VALUES JSON_ARRAY(SELECT ID, N FROM P)", broke(-103, "2A000", "§6.34 SR7c")),
        ("VALUES JSON_ARRAY(SELECT ARRAY[ID] FROM P)", broke(-103, "2A000", "§10.12 SR3")),
        ("VALUES 1 IS JSON", broke(-103, "2A000", "§8.22 F")),
        ("VALUES X'00' IS JSON", unsupported.clone()),
        ("VALUES '1' FORMAT JSON ENCODING UTF16 IS JSON", broke(-103, "2A000", "§10.12 SR8")),
        ("SELECT JSON_OBJECTAGG(ID VALUE N) FROM P", broke(-103, "2A000", "§6.34 F")),
        ("SELECT JSON_OBJECTAGG(N VALUE X'00') FROM P", broke(-103, "2A000", "§10.12 SR7")),
        ("SELECT JSON_ARRAYAGG(ID FORMAT JSON) FROM P", broke(-103, "2A000", "§10.12 SR6")),
        ("SELECT JSON_ARRAYAGG(ID ORDER BY ARRAY[ID]) FROM P", broke(-103, "2A000", "§9.14 SR3")),
        ("SELECT JSON_ARRAYAGG(ID RETURNING VARBINARY(9)) FROM P", unsupported),
    ];
    for (sql, refusal) in cases {
        assert_eq!(refused(&s, sql), refusal, "{sql}");
        assert!(s.prepare(sql).is_err(), "{sql} is refused as it is prepared");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34, §8.22, §10.11] The Format's errors:
/// KEY before a colon, a member of no VALUE nor colon, NULL ON of no NULL,
/// WITH UNIQUE of no UNIQUE, IS of no JSON after FORMAT JSON, IS JSON of a
/// row, are each SQLCODE -102. And each decision of two conditions of the
/// reading, by its pairs [RULES.md R-TEST-8]: FORMAT and JSON (IS JSON
/// after `J FORMAT JSON`, an item `ID FORMAT`, an item `ID JSON`), IS and
/// JSON (`J IS JSON`, `J IS NULL`), NULL and ON (`NULL ON NULL`, `NULL`
/// alone, `ON` alone), WITH and UNIQUE (`WITH UNIQUE`, WITH CHECK OPTION,
/// UNIQUE alone), RETURNING and no operand following (`RETURNING
/// VARCHAR(9)`, a column RETURNING as a name).
#[test]
fn constructor_syntax_6_34() {
    let (_db, s) = people();
    for bad in [
        "VALUES JSON_OBJECT(KEY 'a' : 1)",
        "VALUES JSON_OBJECT('a' 1)",
        "VALUES JSON_ARRAY(1 NULL)",
        "VALUES JSON_ARRAY(1 ON NULL)",
        "VALUES JSON_OBJECT('a' VALUE 1 UNIQUE)",
        "VALUES JSON_OBJECT('a' VALUE 1 WITH KEYS)",
        "VALUES '1' FORMAT JSON IS NULL",
        "VALUES ('1', '2') IS JSON",
    ] {
        assert_eq!(failed(&s, bad).sqlcode(), -102, "{bad}");
    }
    assert_eq!(rows(&s, "SELECT ID FORMAT, ID JSON FROM P WHERE ID = 1"), expected(&["1,1"]));
    assert_eq!(rows(&s, "SELECT ID FROM P WHERE J IS NULL AND J FORMAT JSON IS NOT JSON VALUE"), expected(&[]));
    exec(&s, "CREATE TABLE R (RETURNING VARCHAR(5))").unwrap();
    exec(&s, "INSERT INTO R VALUES ('k')").unwrap();
    let q =
        "SELECT JSON_OBJECT(RETURNING VALUE 1 RETURNING VARCHAR(9)), JSON_OBJECT(RETURNING : 2), JSON_ARRAY(RETURNING VARCHAR(9)) FROM R";
    assert_eq!(rows(&s, q), expected(&[r#"{"k":1},{"k":2},[]"#]));
}

/// Verifies: [ISO/IEC 9075-2:2023 §5.2] JSON_OBJECT, JSON_ARRAY,
/// JSON_OBJECTAGG and JSON_ARRAYAGG are the functions only before "(",
/// and ABSENT, KEYS, SCALAR, OBJECT and JSON words only where the Formats
/// have them: so columns of those names are columns.
#[test]
fn constructor_words_stay_identifiers_5_2() {
    let (_db, s) = people();
    exec(
        &s,
        "CREATE TABLE W (JSON_OBJECT INTEGER, JSON_ARRAY INTEGER, JSON_ARRAYAGG INTEGER, ABSENT INTEGER, SCALAR INTEGER, JSON INTEGER)",
    )
    .unwrap();
    exec(&s, "INSERT INTO W VALUES (1, 2, 3, 4, 5, 6)").unwrap();
    let q = "SELECT JSON_OBJECT + JSON_ARRAY + JSON_ARRAYAGG + ABSENT + SCALAR + JSON, JSON_ARRAY(ABSENT, SCALAR ABSENT ON NULL) FROM W";
    assert_eq!(rows(&s, q), expected(&["21,[4,5]"]));
}
