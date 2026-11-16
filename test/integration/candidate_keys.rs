//! Candidate keys described [ISO/IEC 9075-2:2023 §4.27, §20.10 GR6d, GR7c
//! i 3, Feature T301]: DESCRIBE OUTPUT's KEY_TYPE, 1 of a result's primary
//! key, 2 of its preferred candidate key, and each column's KEY_MEMBER, of
//! base tables' keys, the deduction rule, joins, groups, set operations,
//! table value constructors and ordinality columns, by the left-most rule;
//! 0 of parameters and subordinate items; neither set by SET DESCRIPTOR.

use turborel::db::{Database, Session};

use crate::common::{dynamic_host, run_with as run, set_text as text};

/// Schema A: DEPT, keyed by ID; STAFF, keyed by ID, of a UNIQUE NOT NULL
/// CODE; TWO, of two UNIQUE NOT NULL columns B and C; PAIR, of a key of two
/// columns; LATE, of a deferrable key; and descriptor D allocated, which
/// DESCRIBE sets again for each statement.
fn company() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A \
         CREATE TABLE DEPT (ID INTEGER PRIMARY KEY, NAME VARCHAR(20)) \
         CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(20), DEPT INTEGER, CODE CHAR(4) NOT NULL UNIQUE) \
         CREATE TABLE TWO (A INTEGER, B INTEGER NOT NULL UNIQUE, C INTEGER NOT NULL UNIQUE) \
         CREATE TABLE PAIR (X INTEGER, Y INTEGER, V INTEGER, PRIMARY KEY (X, Y)) \
         CREATE TABLE LATE (K INTEGER, V INTEGER, CONSTRAINT LATE_KEY PRIMARY KEY (K) DEFERRABLE)",
    )
    .unwrap();
    let s = db.session("A");
    run(&s, "ALLOCATE DESCRIPTOR 'D'", &mut dynamic_host()).unwrap();
    (db, s)
}

/// `sql` prepared and described OUTPUT into descriptor D, WITH NESTING if
/// `nesting`: its KEY_TYPE, then each item's KEY_MEMBER, joined by commas.
fn keyed(s: &Session, sql: &str, nesting: bool) -> String {
    let mut p = dynamic_host();
    text(&mut p, sql);
    run(s, "PREPARE Q FROM TEXT", &mut p).unwrap();
    let with = if nesting { "WITH NESTING" } else { "" };
    run(s, &format!("DESCRIBE OUTPUT Q USING SQL DESCRIPTOR 'D' {with}"), &mut p).unwrap();
    run(s, "GET DESCRIPTOR 'D' N = COUNT, VAL = KEY_TYPE", &mut p).unwrap();
    let count: i64 = p.value("N").to_string().parse().unwrap();
    let mut out = vec![p.value("VAL").to_string()];
    for n in 1..=count {
        run(s, &format!("GET DESCRIPTOR 'D' VALUE {n} N = KEY_MEMBER"), &mut p).unwrap();
        out.push(p.value("N").to_string());
    }
    out.join(",")
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.27, §20.10 GR6d, GR7c i 3,
/// §4.26.3, §4.26.15, Feature T301] of a base table's primary key selected,
/// KEY_TYPE 1 and its column a member; of its PRIMARY KEY left out, its
/// UNIQUE of a column NOT NULL the preferred candidate key, 2; of two such,
/// the left-most of the result's columns; a primary key before a left-most
/// UNIQUE; a UNIQUE's column, a strong candidate key, before a column left
/// of it that determines it; a deferrable key none, all the columns then
/// the key; a subordinate item a member of none.
#[test]
fn a_base_tables_keys_are_described_4_27() {
    let (_db, s) = company();
    assert_eq!(keyed(&s, "SELECT NAME, ID FROM STAFF", false), "1,0,1");
    assert_eq!(keyed(&s, "SELECT NAME, CODE FROM STAFF", false), "2,0,1");
    assert_eq!(keyed(&s, "SELECT C, B FROM TWO", false), "2,1,0");
    assert_eq!(keyed(&s, "SELECT B, A, C FROM TWO", false), "2,1,0,0");
    assert_eq!(keyed(&s, "SELECT CODE, ID FROM STAFF", false), "1,0,1");
    assert_eq!(keyed(&s, "SELECT A, B FROM TWO WHERE A = C", false), "2,0,1");
    assert_eq!(keyed(&s, "SELECT V, K FROM LATE", false), "2,1,1");
    assert_eq!(keyed(&s, "SELECT ID, ROW(NAME, DEPT) FROM STAFF", true), "1,1,0,0,0");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.2, §4.26.12, §4.27] a
/// candidate key within a BPK-set is one by the deduction rule: of a key
/// of two columns one fixed by WHERE, the other alone the primary key, and
/// of a key fixed by a parameter the empty set; a
/// computed column not deterministic leaves the key no candidate key.
#[test]
fn a_candidate_key_within_a_key_is_one_4_27() {
    let (_db, s) = company();
    assert_eq!(keyed(&s, "SELECT X, Y, V FROM PAIR", false), "1,1,1,0");
    assert_eq!(keyed(&s, "SELECT X, Y, V FROM PAIR WHERE X = 1", false), "1,0,1,0");
    assert_eq!(keyed(&s, "SELECT ID, NAME FROM STAFF WHERE ID = ?", false), "1,0,0", "of at most one row, the empty set");
    assert_eq!(keyed(&s, "SELECT ID, (SELECT MAX(ID) FROM DEPT) FROM STAFF", false), "2,1,1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.7, §4.26.11, §4.26.12,
/// §4.27] of a join, a key of an operand that determines every column and
/// is not null is the result's: of an inner and a LEFT join, the first's;
/// of a RIGHT join, whose first operand nulls extend, none, all the columns
/// then the key; of a FROM of two tables, none but of a WHERE by which a
/// table's key determines every column.
#[test]
fn a_joins_keys_are_described_4_26_7() {
    let (_db, s) = company();
    let joined = |kind: &str| keyed(&s, &format!("SELECT S.ID, D.ID, D.NAME FROM STAFF S {kind} JOIN DEPT D ON S.DEPT = D.ID"), false);
    assert_eq!(joined("INNER"), "1,1,0,0");
    assert_eq!(joined("LEFT"), "1,1,0,0");
    assert_eq!(joined("RIGHT"), "2,1,1,0");
    assert_eq!(keyed(&s, "SELECT S.ID FROM STAFF S, DEPT D", false), "2,1");
    assert_eq!(keyed(&s, "SELECT D.NAME, S.ID FROM STAFF S, DEPT D WHERE S.DEPT = D.ID", false), "1,0,1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.13, §4.27] of a grouped
/// table, grouping columns within a BPK-set are its primary key, and else
/// they are a strong candidate key, the preferred one, though a column
/// left of them determines them too; of no grouping
/// columns, the empty set; of grouping sets, a union, none but all the
/// columns.
#[test]
fn a_grouped_tables_keys_are_described_4_26_13() {
    let (_db, s) = company();
    assert_eq!(keyed(&s, "SELECT COUNT(*), ID FROM STAFF GROUP BY ID", false), "1,0,1");
    assert_eq!(keyed(&s, "SELECT COUNT(*), DEPT FROM STAFF GROUP BY DEPT", false), "2,0,1");
    assert_eq!(keyed(&s, "SELECT K, V FROM LATE WHERE K = V GROUP BY V", false), "2,0,1");
    assert_eq!(keyed(&s, "SELECT COUNT(*) FROM STAFF", false), "1,0");
    assert_eq!(keyed(&s, "SELECT DEPT, NAME, COUNT(*) FROM STAFF GROUP BY ROLLUP (DEPT, NAME)", false), "2,1,1,1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.26.6, §4.26.8, §4.26.16,
/// §4.27] UNION has no key, all its columns the preferred candidate key;
/// EXCEPT its first operand's; INTERSECT either's; a table value
/// constructor's none (IA203), of one row the empty set; an ordinality
/// column the primary key of its collection derived table.
#[test]
fn queries_keys_are_described_4_26_16() {
    let (_db, s) = company();
    assert_eq!(keyed(&s, "SELECT ID, NAME FROM STAFF UNION SELECT ID, NAME FROM STAFF", false), "2,1,1");
    assert_eq!(keyed(&s, "SELECT ID, NAME FROM STAFF EXCEPT SELECT ID, NAME FROM STAFF WHERE ID = 1", false), "1,1,0");
    assert_eq!(keyed(&s, "SELECT NAME, ID FROM STAFF INTERSECT SELECT NAME, DEPT FROM STAFF", false), "1,0,1");
    assert_eq!(keyed(&s, "VALUES (1, 2), (3, 4)", false), "2,1,1");
    assert_eq!(keyed(&s, "VALUES (1, 2)", false), "2,0,0");
    assert_eq!(keyed(&s, "SELECT E, N FROM UNNEST(ARRAY[5, 6]) WITH ORDINALITY AS U (E, N)", false), "1,0,1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.10 GR6d, GR7c ii 3, §20.5
/// SR2, SR3a] DESCRIBE INPUT sets KEY_TYPE 0 and each parameter's
/// KEY_MEMBER 0; SET DESCRIPTOR sets neither, SQLCODE -103.
#[test]
fn parameters_and_set_descriptor_have_no_key_20_10() {
    let (_db, s) = company();
    let mut p = dynamic_host();
    text(&mut p, "SELECT ID FROM STAFF WHERE ID = ?");
    run(&s, "PREPARE Q FROM TEXT", &mut p).unwrap();
    run(&s, "DESCRIBE OUTPUT Q USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    run(&s, "DESCRIBE INPUT Q USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    run(&s, "GET DESCRIPTOR 'D' N = KEY_TYPE", &mut p).unwrap();
    assert_eq!(p.value("N").to_string(), "0");
    run(&s, "GET DESCRIPTOR 'D' VALUE 1 N = KEY_MEMBER", &mut p).unwrap();
    assert_eq!(p.value("N").to_string(), "0");
    assert_eq!(run(&s, "SET DESCRIPTOR 'D' KEY_TYPE = 1", &mut p), Err("2A000".to_owned()), "§20.5 SR2");
    assert_eq!(run(&s, "SET DESCRIPTOR 'D' VALUE 1 KEY_MEMBER = 1", &mut p), Err("2A000".to_owned()), "§20.5 SR3a");
}
