//! The `<search or cycle clause>` of a recursive query [ISO/IEC
//! 9075-2:2023 §7.18, Feature T133; RULES.md D70]: a cycle marked and its
//! path ended, DEPTH FIRST and BREADTH FIRST sequences, both clauses
//! together, the expansion's reading of the recursive operand, and each
//! Syntax Rule refused.

use turborel::db::{Database, Session};
use turborel::error::Code;

/// The rows of `sql`, each its values joined by commas, or the SQLCODE
/// class of its refusal, as it opens or as a row is fetched.
fn outcome(s: &Session, sql: &str) -> Result<Vec<String>, Code> {
    let mut cursor = s.open_cursor(sql).map_err(|e| e.code)?;
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().map_err(|e| e.code)? {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    Ok(out)
}

/// The rows of `sql`, which is not refused.
fn query(s: &Session, sql: &str) -> Vec<String> {
    outcome(s, sql).unwrap_or_else(|code| panic!("{sql} refused: {code:?}"))
}

/// The SQLCODE class of `sql`'s refusal as it is bound, and the rule it
/// cites.
fn broken(s: &Session, sql: &str) -> (Code, String) {
    let e = s.open_cursor(sql).err().unwrap_or_else(|| panic!("{sql} was not refused"));
    (e.code, e.rule.unwrap_or_default())
}

/// The rule the Syntax Rule refusal of `sql` cites.
fn cited(s: &Session, sql: &str) -> String {
    let (code, rule) = broken(s, sql);
    assert_eq!(code, Code::SyntaxRule, "{sql}: {rule}");
    rule
}

/// Schema A: E (A, B), the arcs of a graph of the cycle 1, 2, 3 and the
/// arc 3, 4; and W (X), of the row 2. Its session takes at most 100
/// iterations of a recursive query, so one that would not end fails fast.
fn graph() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE E (A INTEGER, B INTEGER) CREATE TABLE W (X INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO E VALUES (1, 2), (2, 3), (3, 1), (3, 4)").unwrap();
    a.run("INSERT INTO W VALUES (2)").unwrap();
    a.commit().unwrap();
    a.run("SET EXECUTOR.MAX_ITERATIONS = 100").unwrap();
    (db, a)
}

/// The walk of E from 1, of query R (N), with `clause` after it and `then`
/// after the list.
fn walk(clause: &str, then: &str) -> String {
    format!("WITH RECURSIVE R (N) AS (VALUES (1) UNION ALL SELECT E.B FROM R JOIN E ON E.A = R.N) {clause} {then}")
}

/// Counting from 1 to `n`, of query R (N), with `clause` after it.
fn counting(n: u32, clause: &str) -> String {
    format!("WITH RECURSIVE R (N) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT N + 1 FROM R WHERE N < {n}) {clause} SELECT * FROM R")
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.18 SR1, SR6d, SR8] a
/// cycle clause marks each row whose cycle columns its path holds already
/// with the cycle mark value, and every other with the non-cycle mark
/// value, TRUE and FALSE if none is written; the path holds a row of the
/// cycle columns of each row from the anchor's; and a row marked is
/// extended no further, so a walk of a cyclic graph ends. Marks of
/// character strings of length 1, and FALSE and TRUE, mark as written.
#[test]
fn a_cycle_is_marked_and_ends_its_path_7_18() {
    let (_db, a) = graph();
    let marked = query(&a, &walk("CYCLE N SET C USING P", "SELECT N, C, P FROM R"));
    assert_eq!(
        marked,
        [
            "1,FALSE,ARRAY[ROW(1)]",
            "2,FALSE,ARRAY[ROW(1), ROW(2)]",
            "3,FALSE,ARRAY[ROW(1), ROW(2), ROW(3)]",
            "1,TRUE,ARRAY[ROW(1), ROW(2), ROW(3), ROW(1)]",
            "4,FALSE,ARRAY[ROW(1), ROW(2), ROW(3), ROW(4)]",
        ]
    );
    let lettered = walk("CYCLE N SET C TO 'Y' DEFAULT 'N' USING P", "SELECT N, C FROM R WHERE C = 'Y'");
    assert_eq!(query(&a, &lettered), ["1,Y"]);
    let reversed = walk("CYCLE N SET C TO FALSE DEFAULT TRUE USING P", "SELECT COUNT(*) FROM R WHERE C");
    assert_eq!(query(&a, &reversed), ["4"]);
    let pairs = "WITH RECURSIVE R (N, M) AS (VALUES (1, 0) UNION ALL SELECT N, M + 1 FROM R WHERE M < 2) CYCLE N, M SET C USING P SELECT M, C FROM R";
    assert_eq!(query(&a, pairs), ["0,FALSE", "1,FALSE", "2,FALSE"], "of both columns, no row repeats");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.18 SR5c i, SR8; §9.14
/// SR3] [DESIGN] RULES.md D70: DEPTH FIRST's sequence column is the path of
/// rows of the columns BY names from the anchor's; an array, it is ordered
/// by no operation (SQLCODE -103). It grows past the anchor's one row, to
/// 20 rows of 20 iterations, as an array of the greatest maximum
/// cardinality.
#[test]
fn depth_first_is_the_path_7_18() {
    let (_db, a) = graph();
    let acyclic =
        "WITH RECURSIVE R (N) AS (VALUES (1) UNION ALL SELECT E.B FROM R JOIN E ON E.A = R.N WHERE E.B <> 1) SEARCH DEPTH FIRST BY N SET S";
    assert_eq!(
        query(&a, &format!("{acyclic} SELECT N, S FROM R")),
        ["1,ARRAY[ROW(1)]", "2,ARRAY[ROW(1), ROW(2)]", "3,ARRAY[ROW(1), ROW(2), ROW(3)]", "4,ARRAY[ROW(1), ROW(2), ROW(3), ROW(4)]"]
    );
    assert_eq!(cited(&a, &format!("{acyclic} SELECT N, S FROM R ORDER BY S")), "ISO/IEC 9075-2:2023 §9.14 SR3");
    let deep = counting(20, "SEARCH DEPTH FIRST BY N SET S").replace("SELECT * FROM R", "SELECT MAX(CARDINALITY(S)) FROM R");
    assert_eq!(query(&a, &deep), ["20"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.18 SR5c ii, SR8] [DESIGN]
/// RULES.md D70: BREADTH FIRST's sequence column is a row of the level,
/// from 0, and the columns BY names, ordered level by level; the level is
/// INTEGER, so it passes 9. Of a UNION, which takes no array, a walk of an
/// acyclic graph is each row once.
#[test]
fn breadth_first_is_the_level_7_18() {
    let (_db, a) = graph();
    let levels = counting(12, "SEARCH BREADTH FIRST BY N SET S").replace("SELECT * FROM R", "SELECT N, S FROM R ORDER BY S DESC");
    let got = query(&a, &levels);
    assert_eq!(got.first().map(String::as_str), Some("12,ROW(11, 12)"));
    assert_eq!(got.last().map(String::as_str), Some("1,ROW(0, 1)"));
    let distinct = "WITH RECURSIVE R (N) AS (VALUES (1) UNION SELECT E.B FROM R JOIN E ON E.A = R.N WHERE E.B <> 1) SEARCH BREADTH FIRST BY N SET S SELECT R.S.F1 AS L, N FROM R ORDER BY L";
    assert_eq!(query(&a, distinct), ["0,1", "1,2", "2,3", "3,4"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.18 SR7c, SR8; §9.12 SR3]
/// both clauses add the sequence column, then the cycle mark column and
/// the path column, after the `<with column list>`'s; a UNION of the
/// arrays they hold groups by no operation (SQLCODE -103).
#[test]
fn search_and_cycle_together_7_18() {
    let (_db, a) = graph();
    let both = walk("SEARCH BREADTH FIRST BY N SET S CYCLE N SET C TO 'Y' DEFAULT 'N' USING P", "SELECT * FROM R WHERE C = 'Y'");
    assert_eq!(query(&a, &both), ["1,ROW(3, 1),Y,ARRAY[ROW(1), ROW(2), ROW(3), ROW(1)]"]);
    let grouped = "WITH RECURSIVE R (N) AS (VALUES (1) UNION SELECT E.B FROM R JOIN E ON E.A = R.N) CYCLE N SET C USING P SELECT * FROM R";
    assert_eq!(cited(&a, grouped), "ISO/IEC 9075-2:2023 §9.12 SR3");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.18 SR4, SR8] the
/// recursive operand reads the element by its correlation name, with a
/// `<derived column list>` or not, and a `*` of it, or of the element, is
/// the columns of the `<with column list>`, not those the expansion adds;
/// another table's `<qualifier>`.* is that table's. A `*` of a NATURAL
/// join, which gives its common columns once, is not supported (SQLCODE
/// -900), nor is a UNION CORRESPONDING.
#[test]
fn the_recursive_operand_is_read_as_written_7_18() {
    let (_db, a) = graph();
    let renamed = "WITH RECURSIVE R (N) AS (VALUES (1) UNION ALL SELECT E.B FROM R AS Q (M) JOIN E ON E.A = Q.M) CYCLE N SET C USING P SELECT COUNT(*) FROM R";
    assert_eq!(query(&a, renamed), ["5"]);
    let star =
        "WITH RECURSIVE R (N, M) AS (VALUES (1, 0) UNION ALL SELECT * FROM R WHERE N < 3) CYCLE N SET C USING P SELECT N, M, C FROM R";
    assert_eq!(query(&a, star), ["1,0,FALSE", "1,0,TRUE"]);
    let qualified =
        "WITH RECURSIVE R (N) AS (VALUES (1) UNION ALL SELECT Q.* FROM R AS Q WHERE N < 3) CYCLE N SET C USING P SELECT COUNT(*) FROM R";
    assert_eq!(query(&a, qualified), ["2"]);
    let other =
        "WITH RECURSIVE R (N) AS (VALUES (1) UNION ALL SELECT W.* FROM R, W WHERE N < 2) SEARCH DEPTH FIRST BY N SET S SELECT N FROM R";
    assert_eq!(query(&a, other), ["1", "2"]);
    let natural = "WITH RECURSIVE R (X) AS (VALUES (1) UNION ALL SELECT * FROM R NATURAL JOIN W) CYCLE X SET C USING P SELECT * FROM R";
    assert_eq!(broken(&a, natural).0, Code::Unsupported);
    let corresponding = "WITH RECURSIVE R (N) AS (VALUES (1) UNION ALL CORRESPONDING BY (N) SELECT N + 1 AS N FROM R WHERE N < 3) CYCLE N SET C USING P SELECT * FROM R";
    assert_eq!(broken(&a, corresponding).0, Code::Unsupported);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR3j xi, SR3k] an
/// element with a `<search or cycle clause>` is expandable (SQLCODE -103
/// otherwise): recursive, in a WITH RECURSIVE list and of one; referencing
/// no other query of its list; its query a UNION, not one within a WITH
/// of its own, whose first operand does not reference it and whose second
/// is a `<query specification>`.
#[test]
fn an_element_not_expandable_is_refused_7_17() {
    let (_db, a) = graph();
    let sr3k = "ISO/IEC 9075-2:2023 §7.17 SR3k";
    for sql in [
        "WITH X (P) AS (SELECT A FROM E) SEARCH DEPTH FIRST BY P SET S SELECT * FROM X",
        "WITH RECURSIVE X (P) AS (SELECT A FROM E) CYCLE P SET C USING H SELECT * FROM X",
        "WITH RECURSIVE Y (P) AS (VALUES (1)), X (P) AS (SELECT P FROM Y UNION ALL SELECT P + 1 FROM X WHERE P < 3) CYCLE P SET C USING H SELECT * FROM X",
        "WITH RECURSIVE X (P) AS (WITH B (Q) AS (VALUES (1)) SELECT Q FROM B UNION ALL SELECT P + 1 FROM X WHERE P < 3) CYCLE P SET C USING H SELECT * FROM X",
        "WITH RECURSIVE X (P) AS (SELECT P + 1 FROM X WHERE P < 3 UNION ALL VALUES (1)) CYCLE P SET C USING H SELECT * FROM X",
        "WITH RECURSIVE X (P) AS (VALUES (1) UNION ALL (SELECT P + 1 FROM X WHERE P < 3 UNION ALL VALUES (5))) CYCLE P SET C USING H SELECT * FROM X",
    ] {
        assert_eq!(cited(&a, sql), sr3k, "{sql}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.18 SR4, SR5a, SR5b, SR6a,
/// SR6b, SR7c i] each rule on the columns the clauses name refused
/// (SQLCODE -103): the recursive operand reads the element in its FROM
/// clause, not only within a derived table or as a table of a schema; the
/// sequence column is no column of the element; the columns BY and CYCLE
/// name are its columns, none twice. MC/DC of SR6b, the cycle mark and
/// path columns apart and neither the element's: each of the three
/// conditions false alone. MC/DC of SR7c i: the sequence column the cycle
/// mark column alone, the path column alone, and neither.
#[test]
fn the_columns_named_are_refused_by_their_rules_7_18() {
    let (_db, a) = graph();
    let rule = |n: &str| format!("ISO/IEC 9075-2:2023 §7.18 {n}");
    let nested = "WITH RECURSIVE R (N) AS (VALUES (1) UNION ALL SELECT * FROM (SELECT E.B FROM R JOIN E ON E.A = R.N) X) CYCLE N SET C USING P SELECT * FROM R";
    assert_eq!(cited(&a, nested), rule("SR4"));
    a.run("CREATE TABLE R (N INTEGER)").unwrap();
    let schema = "WITH RECURSIVE R (N) AS (VALUES (1) UNION ALL SELECT N FROM A.R, (SELECT N FROM R WHERE N < 3) Y) CYCLE N SET C USING P SELECT * FROM R";
    assert_eq!(cited(&a, schema), rule("SR4"), "A.R is a table, not the query");
    a.rollback().unwrap();
    a.run("SET EXECUTOR.MAX_ITERATIONS = 100").unwrap();
    for (clause, n) in [
        ("SEARCH DEPTH FIRST BY N SET N", "SR5a"),
        ("SEARCH DEPTH FIRST BY Z SET S", "SR5b"),
        ("SEARCH BREADTH FIRST BY N, N SET S", "SR5b"),
        ("CYCLE Z SET C USING P", "SR6a"),
        ("CYCLE N, N SET C USING P", "SR6a"),
        ("CYCLE N SET C USING C", "SR6b"),
        ("CYCLE N SET N USING P", "SR6b"),
        ("CYCLE N SET C USING N", "SR6b"),
        ("SEARCH DEPTH FIRST BY N SET C CYCLE N SET C USING P", "SR7c i"),
        ("SEARCH DEPTH FIRST BY N SET P CYCLE N SET C USING P", "SR7c i"),
    ] {
        assert_eq!(cited(&a, &walk(clause, "SELECT * FROM R")), rule(n), "{clause}");
    }
    assert_eq!(query(&a, &walk("SEARCH DEPTH FIRST BY N SET S CYCLE N SET C USING P", "SELECT COUNT(*) FROM R")), ["5"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.18 SR6c] the mark values
/// are two literals of one type, character strings of length 1 or
/// Boolean, not equal (SQLCODE -103 otherwise): no number, column or
/// UNKNOWN, no string of two characters, not a string and a Boolean.
/// MC/DC of two strings: 'Y' and 'N' taken; 'YY', then 'NN', of length 2
/// alone; 'Y' and 'Y' equal alone. Of two Booleans: TRUE and FALSE taken,
/// TRUE and TRUE equal.
#[test]
fn the_mark_values_are_refused_by_their_rules_7_18() {
    let (_db, a) = graph();
    for marks in [
        "1 DEFAULT 0",
        "N DEFAULT 'N'",
        "UNKNOWN DEFAULT TRUE",
        "'YY' DEFAULT 'N'",
        "'Y' DEFAULT 'NN'",
        "'Y' DEFAULT 'Y'",
        "TRUE DEFAULT TRUE",
        "'Y' DEFAULT FALSE",
    ] {
        let clause = format!("CYCLE N SET C TO {marks} USING P");
        assert_eq!(cited(&a, &walk(&clause, "SELECT * FROM R")), "ISO/IEC 9075-2:2023 §7.18 SR6c", "{marks}");
    }
    for marks in ["'Y' DEFAULT 'N'", "TRUE DEFAULT FALSE", "('1') DEFAULT ('0')"] {
        let clause = format!("CYCLE N SET C TO {marks} USING P");
        assert_eq!(query(&a, &walk(&clause, "SELECT COUNT(*) FROM R")), ["5"], "{marks}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.18, §11.32] a view of a
/// recursive query with a `<search or cycle clause>` reads as the query
/// does, and a prepared statement of one runs again with new values.
#[test]
fn views_and_prepared_statements_read_the_clauses_7_18() {
    let (_db, a) = graph();
    a.run(&format!("CREATE VIEW V AS {}", walk("CYCLE N SET C USING P", "SELECT N, C FROM R"))).unwrap();
    assert_eq!(query(&a, "SELECT N FROM V WHERE C"), ["1"]);
    let prepared = a.prepare(&walk("CYCLE N SET C USING P", "SELECT COUNT(*) FROM R WHERE N = ?")).unwrap();
    let count = |n: i128| {
        let mut c = a.open_prepared(&prepared, &[turborel::value::Value::Exact(turborel::value::Decimal::new(n, 0))]).unwrap();
        c.fetch().unwrap().map(|r| r[0].to_string())
    };
    assert_eq!([count(1), count(4)], [Some("2".to_owned()), Some("1".to_owned())]);
}
