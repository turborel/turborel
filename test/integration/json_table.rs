//! JSON_TABLE, the `<JSON table>` [ISO/IEC 9075-2:2023 §7.11]: a JSON
//! text's items as rows, its columns of each, nested to any depth and
//! joined by its plan, laterally over the FROM items before it, with
//! PASSING, ON EMPTY and ON ERROR, in statements.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::value::Value;

use crate::common::{failed, rows};

/// Schema J: table D of an INTEGER ID and a VARCHAR(200) DOC, holding
/// (1, an order of two items), (2, an order of none), (3, null) and
/// (4, text that is no JSON text).
fn docs() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION J CREATE TABLE D (ID INTEGER, DOC VARCHAR(200))").unwrap();
    let s = db.session("J");
    s.run(r#"INSERT INTO D VALUES (1, '{"no":7,"items":[{"sku":"a","qty":2},{"sku":"b","qty":5}]}')"#).unwrap();
    s.run(r#"INSERT INTO D VALUES (2, '{"no":8,"items":[]}'), (3, NULL), (4, '{oops')"#).unwrap();
    s.commit().unwrap();
    db
}

/// The rows of `sql`, in order, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|code| panic!("{sql}: {code:?}"))
}

/// The SQLSTATE `sql` fails with.
fn state(s: &Session, sql: &str) -> String {
    failed(s, sql).sqlstate().as_str().to_owned()
}

/// The SQLCODE and the rule `sql` is refused with.
fn refused(s: &Session, sql: &str) -> (i32, String) {
    let e = failed(s, sql);
    (e.sqlcode(), e.rule.unwrap_or_default())
}

/// `refused` of a Syntax Rule of ISO/IEC 9075-2:2023 `cited`.
fn rule(cited: &str) -> (i32, String) {
    (-103, format!("ISO/IEC 9075-2:2023 {cited}"))
}

/// A JSON table of the text `doc`, of the clauses `rest` after its path
/// `path`.
fn table(doc: &str, path: &str, rest: &str) -> String {
    format!("SELECT * FROM JSON_TABLE('{doc}', '{path}' {rest}) AS JT")
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.11 SR1e, SR1f, SR2, GR1e ii 1 to 3;
/// Feature T821] A row of each item of the row pattern, in order: an
/// ordinality column its ordinal from 1, an exact numeric of scale 0
/// (IV158, INTEGER); a regular column its path's item cast to its type;
/// a formatted column FORMAT JSON its path's sequence serialized; a column
/// of no path takes `$.` and its name as a JSON string, a regular
/// identifier's in upper case [SR1e v 2].
#[test]
fn columns_of_each_kind_7_11() {
    let db = docs();
    let s = db.session("J");
    let doc = r#"[{"a":1,"b":"x","A":10},{"a":2.5,"b":"yy","A":20}]"#;
    let q = table(
        doc,
        "lax $[*]",
        r#"COLUMNS (N FOR ORDINALITY, "a" DECIMAL(3,1), A INTEGER, B VARCHAR(5) PATH 'lax $.b', J VARCHAR(40) FORMAT JSON PATH 'lax $')"#,
    );
    assert_eq!(query(&s, &q), [r#"1,1.0,10,x,{"a":1,"b":"x","A":10}"#, r#"2,2.5,20,yy,{"a":2.5,"b":"yy","A":20}"#]);
    let view = format!("CREATE VIEW V AS {}", table(doc, "lax $[*]", "COLUMNS (N FOR ORDINALITY, B VARCHAR(5) PATH 'lax $.b')"));
    s.run(&view).unwrap();
    let types = "SELECT COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'V' ORDER BY ORDINAL_POSITION";
    assert_eq!(query(&s, types), ["N,INTEGER", "B,CHARACTER VARYING"]);
    let renamed = "SELECT X.P, X.Q FROM JSON_TABLE('[3]', 'lax $[*]' COLUMNS (N FOR ORDINALITY, V INTEGER PATH 'lax $')) AS X (P, Q)";
    assert_eq!(query(&s, renamed), ["1,3"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.11 GR1a, GR1e ii; §9.48 GR4b, GR4c]
/// A null context item is an empty table; an empty sequence too; the
/// SQL/JSON null and a missing member are null in a column, NULL ON
/// EMPTY being implicit [SR1e iii].
#[test]
fn nulls_and_empty_sequences_7_11() {
    let db = docs();
    let s = db.session("J");
    let q = "SELECT * FROM JSON_TABLE(CAST(NULL AS VARCHAR(5)), 'lax $' COLUMNS (V INTEGER PATH 'lax $')) AS JT";
    assert_eq!(query(&s, q), Vec::<String>::new());
    assert_eq!(query(&s, &table("[]", "lax $[*]", "COLUMNS (V INTEGER PATH 'lax $')")), Vec::<String>::new());
    let q = table(r#"[null,{"v":null},{}]"#, "lax $[*]", "COLUMNS (N FOR ORDINALITY, V INTEGER PATH 'lax $.v')");
    assert_eq!(query(&s, &q), ["1,NULL", "2,NULL", "3,NULL"]);
}

/// The nested document of NOTE 251's shape: a root of two NESTED
/// COLUMNS siblings, `xs` and `ys`, the clauses `plan` after them.
fn siblings(xs: &str, ys: &str, plan: &str) -> String {
    let doc = format!(r#"{{"id":1,"xs":{xs},"ys":{ys}}}"#);
    let columns = "COLUMNS (ID INTEGER PATH 'lax $.id', \
                   NESTED PATH 'lax $.xs[*]' AS PX COLUMNS (X INTEGER PATH 'lax $'), \
                   NESTED PATH 'lax $.ys[*]' AS PY COLUMNS (Y VARCHAR(3) PATH 'lax $'))";
    format!("SELECT * FROM JSON_TABLE('{doc}', 'lax $' AS PR {columns} {plan}) AS JT")
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.11 SR1i, SR1k iii 2 to 6; NOTE 243,
/// NOTE 244, NOTE 251; Features T827, T838] The implicit plan is the
/// parent OUTER the UNION of its children, PLAN (PR OUTER (PX UNION PY)):
/// each child's rows, the other's columns null, then the parent's row
/// null extended of none. PLAN DEFAULT (CROSS) takes the product of the
/// siblings, PLAN DEFAULT (INNER) drops a parent of no child rows, both
/// choices in either order.
#[test]
fn default_plans_7_11() {
    let db = docs();
    let s = db.session("J");
    assert_eq!(query(&s, &siblings("[1,2]", r#"["a"]"#, "")), ["1,1,NULL", "1,2,NULL", "1,NULL,a"]);
    assert_eq!(query(&s, &siblings("[1,2]", r#"["a"]"#, "PLAN DEFAULT (OUTER, UNION)")), ["1,1,NULL", "1,2,NULL", "1,NULL,a"]);
    assert_eq!(query(&s, &siblings("[1,2]", r#"["a","b"]"#, "PLAN DEFAULT (CROSS)")), ["1,1,a", "1,1,b", "1,2,a", "1,2,b"]);
    assert_eq!(query(&s, &siblings("[1]", "[]", "PLAN DEFAULT (CROSS)")), ["1,NULL,NULL"], "OUTER keeps the parent of an empty product");
    assert_eq!(query(&s, &siblings("[1]", "[]", "PLAN DEFAULT (CROSS, INNER)")), Vec::<String>::new());
    assert_eq!(query(&s, &siblings("[]", "[]", "")), ["1,NULL,NULL"]);
    assert_eq!(query(&s, &siblings("[]", "[]", "PLAN DEFAULT (INNER)")), Vec::<String>::new());
    assert_eq!(query(&s, &siblings("[]", r#"["a"]"#, "PLAN DEFAULT (UNION, INNER)")), ["1,NULL,a"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.11 SR1j, SR1k; Feature T824] A
/// specific plan joins as it says: OUTER and INNER, UNION and CROSS,
/// parenthesized, each path name once.
#[test]
fn specific_plans_7_11() {
    let db = docs();
    let s = db.session("J");
    assert_eq!(query(&s, &siblings("[1]", r#"["a"]"#, "PLAN (PR OUTER (PX UNION PY))")), ["1,1,NULL", "1,NULL,a"]);
    assert_eq!(query(&s, &siblings("[1,2]", r#"["a"]"#, "PLAN (PR INNER (PY CROSS PX))")), ["1,1,a", "1,2,a"]);
    assert_eq!(query(&s, &siblings("[1]", "[]", "PLAN (PR INNER (PX CROSS PY))")), Vec::<String>::new());
    assert_eq!(query(&s, &siblings("[]", "[]", "PLAN (PR OUTER (PX CROSS PY))")), ["1,NULL,NULL"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.11 SR1g, SR1i iv to ix, SR1k iii 6
/// B II, GR1e ii 4] NESTED COLUMNS nest to any depth, each of the items
/// of its parent's row, PATH optional; the ordinal is of each parent's
/// row anew; columns are in the order written at every depth [SR1l];
/// PLAN names three levels.
#[test]
fn deep_nesting_7_11() {
    let db = docs();
    let s = db.session("J");
    let doc = r#"{"o":[{"k":"p","l":[{"m":[1,2]},{"m":[]}]},{"k":"q","l":[]}]}"#;
    let columns = "COLUMNS (NESTED 'lax $.o[*]' AS A COLUMNS (K VARCHAR(2) PATH 'lax $.k', \
                   NESTED PATH 'lax $.l[*]' AS B COLUMNS (BN FOR ORDINALITY, NESTED 'lax $.m[*]' AS C COLUMNS (M INTEGER PATH 'lax $', \
                   CN FOR ORDINALITY))))";
    let all = ["p,1,1,1", "p,1,2,2", "p,2,NULL,NULL", "q,NULL,NULL,NULL"];
    let q = format!("SELECT K, BN, M, CN FROM JSON_TABLE('{doc}', 'lax $' AS R {columns}) AS JT");
    assert_eq!(query(&s, &q), all);
    let q = format!("SELECT K, BN, M, CN FROM JSON_TABLE('{doc}', 'lax $' AS R {columns} PLAN (R OUTER (A OUTER (B OUTER C)))) AS JT");
    assert_eq!(query(&s, &q), all);
    let q = format!("SELECT K, BN, M, CN FROM JSON_TABLE('{doc}', 'lax $' AS R {columns} PLAN (R INNER (A INNER (B INNER C)))) AS JT");
    assert_eq!(query(&s, &q), ["p,1,1,1", "p,1,2,2"]);
    let q = format!("SELECT K, BN, M, CN FROM JSON_TABLE('{doc}', 'lax $' AS R {columns} PLAN (R OUTER (A INNER (B OUTER C)))) AS JT");
    assert_eq!(query(&s, &q), ["p,1,1,1", "p,1,2,2", "p,2,NULL,NULL"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.11 SR1g i, SR1h, SR1j i, SR1j ii,
/// SR1k iii 6 B II] A specific plan is refused, SQLCODE -103, of a nested
/// columns clause of no path name, of a path name missing, unknown or
/// twice, of one joined to a parent it is not nested in, and of one joined
/// to none; two columns or paths of one name are refused, plan or none.
#[test]
fn plans_refused_7_11() {
    let db = docs();
    let s = db.session("J");
    assert_eq!(refused(&s, &siblings("[]", "[]", "PLAN (PR OUTER PX)")), rule("§7.11 SR1j i"));
    assert_eq!(refused(&s, &siblings("[]", "[]", "PLAN (PR OUTER (PX UNION PZ))")), rule("§7.11 SR1j i"));
    assert_eq!(refused(&s, &siblings("[]", "[]", "PLAN (PR OUTER (PX UNION PX UNION PY))")), rule("§7.11 SR1j i"));
    assert_eq!(refused(&s, &siblings("[]", "[]", "PLAN (PX OUTER (PR UNION PY))")), rule("§7.11 SR1j ii"));
    assert_eq!(refused(&s, &siblings("[]", "[]", "PLAN (PR UNION PX UNION PY)")), rule("§7.11 SR1k iii 6 B II"));
    let nested = "SELECT * FROM JSON_TABLE('{}', 'lax $' AS R COLUMNS (NESTED 'lax $.a' AS A COLUMNS (NESTED 'lax $.b' AS B \
                  COLUMNS (V INTEGER))) PLAN (R OUTER (A UNION B))) AS JT";
    assert_eq!(refused(&s, nested), rule("§7.11 SR1k iii 6 B II"));
    let unnamed = "SELECT * FROM JSON_TABLE('{}', 'lax $' AS R COLUMNS (NESTED 'lax $.a' COLUMNS (V INTEGER)) PLAN (R)) AS JT";
    assert_eq!(refused(&s, unnamed), rule("§7.11 SR1g i"));
    let rootless = "SELECT * FROM JSON_TABLE('{}', 'lax $' COLUMNS (V INTEGER) PLAN (R)) AS JT";
    assert_eq!(refused(&s, rootless), rule("§7.11 SR1j i"));
    assert_eq!(refused(&s, &table("{}", "lax $", "COLUMNS (V INTEGER, V INTEGER)")), rule("§7.11 SR1h"));
    let shared = "SELECT * FROM JSON_TABLE('{}', 'lax $' AS V COLUMNS (V INTEGER)) AS JT";
    assert_eq!(refused(&s, shared), rule("§7.11 SR1h"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.11 SR1e ii, SR1f ii, iv, vii, SR1e
/// vi; §9.46 SR4; §10.14 SR1; §9.16 SR1u] The Syntax Rules of the
/// columns and operands, SQLCODE -103: a regular column of a type JSON_VALUE
/// does not return; a formatted column without FORMAT JSON, of no string
/// type, or of a quotes behavior WITH a wrapper; a column path that names
/// a variable; a row pattern's variable no argument has; a context item
/// of no string type; a JSON table in a constraint. DEFAULT of a
/// formatted column is no Format's, -102.
#[test]
fn syntax_rules_7_11() {
    let db = docs();
    let s = db.session("J");
    assert_eq!(refused(&s, &table("{}", "lax $", "COLUMNS (V INTERVAL DAY)")).0, -103);
    assert_eq!(refused(&s, &table("{}", "lax $", "COLUMNS (V VARCHAR(9) WITH WRAPPER)")), rule("§7.11 SR1f ii 1"));
    assert_eq!(refused(&s, &table("{}", "lax $", "COLUMNS (V INTEGER FORMAT JSON)")), rule("§7.11 SR1f iv"));
    assert_eq!(refused(&s, &table("{}", "lax $", "COLUMNS (V VARCHAR(9) FORMAT JSON WITH WRAPPER KEEP QUOTES)")), rule("§7.11 SR1f vii"));
    assert_eq!(refused(&s, &table("{}", "lax $", "PASSING 1 AS X COLUMNS (V INTEGER PATH 'lax $X')")).0, -103);
    assert_eq!(refused(&s, &table("{}", "lax $X", "COLUMNS (V INTEGER)")).0, -103);
    assert_eq!(refused(&s, "SELECT * FROM JSON_TABLE(1, 'lax $' COLUMNS (V INTEGER)) AS JT").0, -103);
    assert_eq!(failed(&s, &table("{}", "lax $", "COLUMNS (V VARCHAR(9) FORMAT JSON DEFAULT 'x' ON EMPTY)")).sqlcode(), -102);
    let check = "CREATE TABLE C (K VARCHAR(9) CHECK (EXISTS (SELECT * FROM JSON_TABLE(K, 'lax $' COLUMNS (V INTEGER)) AS JT)))";
    assert_eq!(s.run(check).unwrap_err().sqlcode(), -103);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.11 SR1b, GR1e i; §9.47; Feature
/// T825] An exception condition of the context item's text or of a row
/// pattern makes its primitive an empty table, EMPTY ON ERROR implicit;
/// ERROR ON ERROR raises it: 22032 of no JSON text, 2203A of a member
/// strict mode does not find. A nested row pattern's leaves its parent's
/// row null extended.
#[test]
fn table_error_behavior_7_11() {
    let db = docs();
    let s = db.session("J");
    assert_eq!(query(&s, &table("{oops", "lax $", "COLUMNS (V INTEGER)")), Vec::<String>::new());
    assert_eq!(query(&s, &table("{oops", "lax $", "COLUMNS (V INTEGER) EMPTY ON ERROR")), Vec::<String>::new());
    assert_eq!(state(&s, &table("{oops", "lax $", "COLUMNS (V INTEGER) ERROR ON ERROR")), "22032");
    assert_eq!(query(&s, &table("{}", "strict $.a", "COLUMNS (V INTEGER)")), Vec::<String>::new());
    assert_eq!(state(&s, &table("{}", "strict $.a", "COLUMNS (V INTEGER) ERROR ON ERROR")), "2203A");
    let nested = r"COLUMNS (K INTEGER PATH 'lax $.k', NESTED 'strict $.n[*]' COLUMNS (V INTEGER PATH 'lax $'))";
    assert_eq!(query(&s, &table(r#"{"k":1}"#, "lax $", nested)), ["1,NULL"]);
    assert_eq!(state(&s, &format!("{} ERROR ON ERROR) AS JT", table(r#"{"k":1}"#, "lax $", nested).trim_end_matches(") AS JT"))), "2203A");
    let dated = "COLUMNS (NESTED 'lax $' COLUMNS (V INTEGER PATH 'lax $.a'))";
    assert_eq!(query(&s, &table("\"2024-02-29\"", "lax $.datetime()", dated)), ["NULL"], "an item of no JSON text chains the null value");
    assert_eq!(state(&s, &table("\"2024-02-29\"", "lax $.datetime()", &format!("{dated} ERROR ON ERROR"))), "22032");
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.11 SR1e iii, iv, GR1e ii 2; §9.48
/// GR4, GR5; Features T825, T826] Each behavior of a regular column: NULL
/// ON EMPTY and NULL ON ERROR implicit, ERROR ON ERROR implicit of a table
/// ERROR ON ERROR; ERROR raises 22035 of no item, 22034 of two, 2203F of
/// an array, 2203G of a type not cast, 22018 of a cast that fails; DEFAULT
/// gives its value cast to the column's type, of any value expression.
#[test]
fn regular_column_behaviors_7_11() {
    let db = docs();
    let s = db.session("J");
    let doc = r#"{"two":[1,2],"arr":[1],"t":true,"s":"x"}"#;
    let of = |column: &str| table(doc, "lax $", &format!("COLUMNS ({column})"));
    assert_eq!(query(&s, &of("V INTEGER PATH 'lax $.none', W INTEGER PATH 'lax $.two[*]', X INTEGER PATH 'lax $.s'")), ["NULL,NULL,NULL"]);
    assert_eq!(state(&s, &of("V INTEGER PATH 'lax $.none' ERROR ON EMPTY ERROR ON ERROR")), "22035");
    assert_eq!(query(&s, &of("V INTEGER PATH 'lax $.none' ERROR ON EMPTY")), ["NULL"], "ON ERROR replaces ERROR ON EMPTY's exception");
    assert_eq!(state(&s, &of("V INTEGER PATH 'lax $.two[*]' ERROR ON ERROR")), "22034");
    assert_eq!(state(&s, &of("V INTEGER PATH 'lax $.arr' ERROR ON ERROR")), "2203F");
    assert_eq!(state(&s, &of("V INTEGER PATH 'lax $.t' ERROR ON ERROR")), "2203G");
    assert_eq!(state(&s, &of("V INTEGER PATH 'lax $.s' ERROR ON ERROR")), "22018");
    assert_eq!(state(&s, &format!("{} ERROR ON ERROR) AS JT", of("V INTEGER PATH 'lax $.s'").trim_end_matches(") AS JT"))), "22018");
    let defaults = "V INTEGER PATH 'lax $.none' DEFAULT 4 + 1 ON EMPTY, W INTEGER PATH 'lax $.s' DEFAULT '-1' ON ERROR, \
                    X INTEGER PATH 'lax $.none' ERROR ON EMPTY DEFAULT 6 ON ERROR, Y INTEGER PATH 'lax $.t' NULL ON ERROR";
    assert_eq!(query(&s, &of(defaults)), ["5,-1,6,NULL"]);
    assert_eq!(state(&s, &of("V INTEGER PATH 'lax $.arr' DEFAULT 'x' ON ERROR")), "22018");
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.11 SR1f v to xi, GR1e ii 3; §9.49]
/// A formatted column FORMAT JSON serializes its sequence: WITHOUT ARRAY
/// implicit, so two items are null, or 22034 ERROR ON ERROR; WITH
/// [UNCONDITIONAL] WRAPPER and WITH CONDITIONAL WRAPPER wrap them; OMIT
/// QUOTES gives a string bare; EMPTY ARRAY and EMPTY OBJECT ON EMPTY;
/// 22001 of a text longer than its type.
#[test]
fn formatted_columns_7_11() {
    let db = docs();
    let s = db.session("J");
    let doc = r#"{"two":[1,2],"o":{"a":1},"s":"x"}"#;
    let of = |column: &str| table(doc, "lax $", &format!("COLUMNS ({column})"));
    let wraps = "A VARCHAR(20) FORMAT JSON PATH 'lax $.two[*]', B VARCHAR(20) FORMAT JSON PATH 'lax $.two[*]' WITH WRAPPER, \
                 C VARCHAR(20) FORMAT JSON PATH 'lax $.o' WITH CONDITIONAL ARRAY WRAPPER, \
                 D VARCHAR(20) FORMAT JSON PATH 'lax $.o' WITH UNCONDITIONAL WRAPPER, E VARCHAR(20) FORMAT JSON PATH 'lax $.s', \
                 F VARCHAR(20) FORMAT JSON PATH 'lax $.s' OMIT QUOTES ON SCALAR STRING";
    assert_eq!(query(&s, &of(wraps)), [r#"NULL,[1,2],{"a":1},[{"a":1}],"x",x"#]);
    assert_eq!(state(&s, &of("A VARCHAR(20) FORMAT JSON PATH 'lax $.two[*]' ERROR ON ERROR")), "22034");
    let empty = "A VARCHAR(9) FORMAT JSON PATH 'lax $.none' EMPTY ARRAY ON EMPTY, \
                 B VARCHAR(9) FORMAT JSON PATH 'lax $.none' EMPTY OBJECT ON EMPTY, \
                 C VARCHAR(9) FORMAT JSON PATH 'lax $.two' EMPTY ARRAY ON ERROR";
    assert_eq!(query(&s, &of(empty)), ["[],{},[1,2]"]);
    assert_eq!(state(&s, &of("A VARCHAR(3) FORMAT JSON PATH 'lax $.o' ERROR ON ERROR")), "22001");
    assert_eq!(state(&s, &of("A VARCHAR(9) FORMAT JSON PATH 'lax $.none' ERROR ON EMPTY ERROR ON ERROR")), "22035");
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.11 SR1a iv, SR1k iii 6 D; §9.46
/// GR11b; Feature T823] PASSING arguments are named variables of the row
/// pattern and of nested ones, a value or FORMAT JSON text, a regular
/// identifier's name in upper case.
#[test]
fn passing_7_11() {
    let db = docs();
    let s = db.session("J");
    let q = r"SELECT * FROM JSON_TABLE('[1,5,9]', 'lax $[*] ? (@ > $LOW)' PASSING 4 AS LOW COLUMNS (V INTEGER PATH 'lax $')) AS JT";
    assert_eq!(query(&s, q), ["5", "9"]);
    let q = r#"SELECT * FROM JSON_TABLE('{"a":[1,2,3]}', 'lax $' PASSING '{"n":2}' FORMAT JSON AS "o"
               COLUMNS (NESTED 'lax $.a[*] ? (@ >= $o.n)' COLUMNS (V INTEGER PATH 'lax $'))) AS JT"#;
    assert_eq!(query(&s, q), ["2", "3"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 SR5, SR20a; §7.10 GR1d; §7.11
/// SR1m] A JSON table is lateral: its context item and DEFAULTs read the
/// FROM items before it, of each of their rows; a LEFT join keeps a row
/// of none of its rows, null extended; the null and the bad document give
/// none; a `?` is its context item, CHARACTER VARYING.
#[test]
fn lateral_7_11() {
    let db = docs();
    let s = db.session("J");
    let columns = "COLUMNS (SKU VARCHAR(3) PATH 'lax $.sku', QTY INTEGER PATH 'lax $.qty', \
                   DFLT INTEGER PATH 'lax $.none' DEFAULT D.ID * 100 ON EMPTY)";
    let q = format!("SELECT D.ID, JT.* FROM D, JSON_TABLE(D.DOC, 'lax $.items[*]' {columns}) AS JT ORDER BY D.ID, SKU");
    assert_eq!(query(&s, &q), ["1,a,2,100", "1,b,5,100"]);
    let q = format!("SELECT D.ID, SKU FROM D LEFT JOIN JSON_TABLE(D.DOC, 'lax $.items[*]' {columns}) AS JT ON QTY > 2 ORDER BY D.ID");
    assert_eq!(query(&s, &q), ["1,b", "2,NULL", "3,NULL", "4,NULL"]);
    let q = "SELECT D.ID, N FROM D CROSS JOIN JSON_TABLE(D.DOC, 'lax $.no' COLUMNS (N INTEGER PATH 'lax $')) AS JT WHERE N > 7";
    assert_eq!(query(&s, q), ["2,8"]);
    let p = s.prepare("SELECT V FROM JSON_TABLE(?, 'lax $[*]' COLUMNS (V INTEGER PATH 'lax $')) AS JT").unwrap();
    let mut c = s.open_prepared(&p, &[Value::Char("[4,5]".into())]).unwrap();
    assert_eq!(c.fetch().unwrap().map(|r| r[0].to_string()), Some("4".to_owned()));
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.11 GR1e ii 2; §9.48 GR4c, GR5d]
/// [DESIGN] A DEFAULT is evaluated only of a row whose column takes it: a
/// DEFAULT that would divide by zero raises 22012 only of a row its
/// column's path finds nothing in, or the cast of what it finds fails.
#[test]
fn defaults_are_lazy_7_11() {
    let db = docs();
    let s = db.session("J");
    let of = |doc: &str, behavior: &str| {
        format!("SELECT V FROM JSON_TABLE('{doc}', 'lax $[*]' COLUMNS (V INTEGER PATH 'lax $.v' DEFAULT 1 / 0 {behavior})) AS JT")
    };
    assert_eq!(query(&s, &of(r#"[{"v":1},{"v":2}]"#, "ON EMPTY")), ["1", "2"]);
    assert_eq!(state(&s, &of(r#"[{"v":1},{}]"#, "ON EMPTY")), "22012");
    assert_eq!(query(&s, &of(r#"[{"v":1},{}]"#, "ON ERROR")), ["1", "NULL"]);
    assert_eq!(state(&s, &of(r#"[{"v":"x"}]"#, "ON ERROR")), "22012");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.11] EXPLAIN shows a JSON
/// table, its row pattern, its width and its plan, the implicit one
/// spelled out, a path of no name by its row pattern.
#[test]
fn explain_shows_json_table_7_11() {
    let db = docs();
    let s = db.session("J");
    let q = "SELECT * FROM D, JSON_TABLE(D.DOC, 'lax $' AS R COLUMNS (N INTEGER PATH 'lax $.no', \
             NESTED 'lax $.items[*]' COLUMNS (Q INTEGER PATH 'lax $.qty'))) AS JT";
    let plan = s.explain(q, &Params::new()).unwrap();
    assert!(plan.contains("JSON_TABLE 'lax $' of 2 columns PLAN (R OUTER ('lax $.items[*]'))"), "{plan}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.11 SR1e ii, SR1f ii 2, ix; §10.12
/// SR5] A context item of the JSON type is read as it is, FORMAT JSON
/// implicit; a column of the JSON type is formatted, its value of the JSON
/// type, KEEP QUOTES; OMIT QUOTES and DEFAULT of it are refused, -103.
#[test]
fn json_typed_7_11() {
    let db = docs();
    let s = db.session("J");
    let q = r#"SELECT * FROM JSON_TABLE(CAST('{"a":[1,"x"]}' AS JSON), 'lax $' COLUMNS (A JSON PATH 'lax $.a', S JSON PATH 'lax $.a[1]')) AS JT"#;
    assert_eq!(query(&s, q), [r#"[1,"x"],"x""#]);
    let omit = r"SELECT * FROM JSON_TABLE('{}', 'lax $' COLUMNS (A JSON PATH 'lax $' OMIT QUOTES)) AS JT";
    assert_eq!(refused(&s, omit), rule("§7.11 SR1f ix"));
    let default = r"SELECT * FROM JSON_TABLE('{}', 'lax $' COLUMNS (A JSON PATH 'lax $' DEFAULT '1' ON EMPTY)) AS JT";
    assert_eq!(refused(&s, default), rule("§7.11 SR1e ii"));
}
