//! Collations [ISO/IEC 9075-2:2023 §4.3.6, §9.15, §10.7, §11.43; RULES.md
//! D60]: what orders and compares by one, and what is refused.

use turborel::db::{Cursor, Database, Session};
use turborel::error::Code;

/// Schema A: P, whose NAME is unique under CASE_INSENSITIVE; W, whose WORD
/// orders by UNICODE; and N, of a plain column. P holds Bob and Ann.
fn collated() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE P (NAME VARCHAR(20) COLLATE CASE_INSENSITIVE UNIQUE, AGE INTEGER)
           CREATE TABLE W (WORD VARCHAR(20) COLLATE UNICODE)
           CREATE TABLE N (S VARCHAR(20))",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO P VALUES ('Bob', 30), ('Ann', 40)").unwrap();
    a.run("INSERT INTO W VALUES ('Zebra'), ('apple'), ('résumé'), ('resume'), ('zoo')").unwrap();
    a.run("INSERT INTO N VALUES ('Zebra'), ('apple'), ('résumé')").unwrap();
    a.commit().unwrap();
    db
}

fn lines(mut cursor: Cursor) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

fn query(s: &Session, sql: &str) -> Vec<String> {
    lines(s.open_cursor(sql).unwrap())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.6, §13.1] ORDER BY is by a
/// column's collation; a column of none by scalar value; COLLATE overrides
/// either, in either direction.
#[test]
fn order_by_follows_the_collation_9_15() {
    let db = collated();
    let a = db.session("A");
    assert_eq!(query(&a, "SELECT WORD FROM W ORDER BY WORD"), ["apple", "resume", "résumé", "Zebra", "zoo"]);
    assert_eq!(query(&a, "SELECT S FROM N ORDER BY S"), ["Zebra", "apple", "résumé"]);
    assert_eq!(query(&a, "SELECT S FROM N ORDER BY S COLLATE UNICODE"), ["apple", "résumé", "Zebra"]);
    assert_eq!(query(&a, "SELECT WORD FROM W ORDER BY WORD COLLATE UCS_BASIC"), ["Zebra", "apple", "resume", "résumé", "zoo"]);
    assert_eq!(query(&a, "SELECT WORD FROM W ORDER BY WORD DESC"), ["zoo", "Zebra", "résumé", "resume", "apple"]);
}

/// Verifies: [EXTENSION] RULES.md D60, [ISO/IEC 9075:1992 §11.7] UNIQUE is by
/// the column's collation: `bob` is a duplicate of `Bob`, refused, and the
/// statement leaves no effect; another name is taken.
#[test]
fn unique_follows_the_collation_11_7() {
    let db = collated();
    let a = db.session("A");
    assert_eq!(a.run("INSERT INTO P VALUES ('bob', 31)").err().map(|e| e.code), Some(Code::Constraint));
    assert_eq!(a.run("INSERT INTO P VALUES ('BOB', 31)").err().map(|e| e.code), Some(Code::Constraint));
    a.rollback().unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM P"), ["2"]);
    a.run("INSERT INTO P VALUES ('Carol', 50)").unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.15; RULES.md D60] a literal
/// yields to a column's collation, in =, IN, BETWEEN and CASE, and the index
/// on the column finds the row; an explicit COLLATE overrides it.
#[test]
fn a_literal_yields_to_a_column_9_15() {
    let db = collated();
    let a = db.session("A");
    assert_eq!(query(&a, "SELECT AGE FROM P WHERE NAME = 'BOB'"), ["30"]);
    assert_eq!(query(&a, "SELECT AGE FROM P WHERE 'bOb' = NAME"), ["30"]);
    assert_eq!(query(&a, "SELECT AGE FROM P WHERE NAME IN ('x', 'ann')"), ["40"]);
    assert_eq!(query(&a, "SELECT AGE FROM P WHERE NAME BETWEEN 'A' AND 'AZ'"), ["40"]);
    assert_eq!(query(&a, "SELECT CASE NAME WHEN 'BOB' THEN 1 ELSE 0 END, AGE FROM P ORDER BY AGE"), ["1,30", "0,40"]);
    assert_eq!(query(&a, "SELECT AGE FROM P WHERE NAME COLLATE UCS_BASIC = 'BOB'").len(), 0);
    assert_eq!(query(&a, "SELECT AGE FROM P WHERE NAME COLLATE UCS_BASIC = 'Bob'"), ["30"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.15 SR4b, SR4e] two columns
/// of different collations compare only under an explicit COLLATE; two
/// different explicit ones are refused.
#[test]
fn collations_that_conflict_are_refused_9_15() {
    let db = collated();
    let a = db.session("A");
    let code = |sql: &str| a.open_cursor(sql).err().map(|e| e.code);
    assert_eq!(code("SELECT 1 FROM P, W WHERE NAME = WORD"), Some(Code::SyntaxRule), "SR4e");
    assert_eq!(code("SELECT 1 FROM P, N WHERE NAME = S"), Some(Code::SyntaxRule), "SR4e: of the default");
    let both = "SELECT 1 FROM P, W WHERE NAME COLLATE UNICODE = WORD COLLATE CASE_INSENSITIVE";
    assert_eq!(code(both), Some(Code::SyntaxRule), "SR4b");
    assert_eq!(query(&a, "SELECT 1 FROM P, W WHERE NAME = WORD COLLATE UNICODE").len(), 0);
    assert_eq!(code("SELECT 1 FROM P WHERE AGE COLLATE UNICODE = 1"), Some(Code::SyntaxRule), "of no string");
    assert_eq!(code("SELECT 1 FROM P WHERE NAME COLLATE NO_SUCH = 'x'"), Some(Code::SyntaxRule), "§10.7 SR1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.2 GR3d] GROUP BY, DISTINCT,
/// UNION, EXCEPT and INTERSECT take strings equal by a collation as one,
/// showing one of the strings themselves, not a sort key; MIN and MAX order
/// by it; COUNT(DISTINCT) counts by it.
#[test]
fn equal_strings_are_one_by_a_collation() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (C VARCHAR(9) COLLATE CASE_INSENSITIVE, K INTEGER) CREATE TABLE U (C VARCHAR(9) COLLATE CASE_INSENSITIVE)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES ('a', 1), ('A', 2), ('b', 3), ('Z', 4)").unwrap();
    a.run("INSERT INTO U VALUES ('B')").unwrap();
    assert_eq!(query(&a, "SELECT C, COUNT(*) FROM T GROUP BY C ORDER BY C"), ["a,2", "b,1", "Z,1"]);
    assert_eq!(
        query(&a, "SELECT C, COUNT(*) FROM T GROUP BY C COLLATE UCS_BASIC ORDER BY C COLLATE UCS_BASIC"),
        ["A,1", "Z,1", "a,1", "b,1"]
    );
    assert_eq!(query(&a, "SELECT DISTINCT C FROM T ORDER BY C"), ["a", "b", "Z"]);
    assert_eq!(query(&a, "SELECT C FROM T UNION SELECT C FROM U ORDER BY 1"), ["a", "b", "Z"]);
    assert_eq!(query(&a, "SELECT C FROM T EXCEPT SELECT C FROM U ORDER BY 1"), ["a", "Z"]);
    assert_eq!(query(&a, "SELECT C FROM T INTERSECT SELECT C FROM U"), ["b"]);
    assert_eq!(query(&a, "SELECT MAX(C), MIN(C) FROM T"), ["Z,a"]);
    assert_eq!(query(&a, "SELECT COUNT(DISTINCT C) FROM T"), ["3"]);
    assert_eq!(query(&a, "SELECT MAX(C COLLATE UCS_BASIC), MAX(C) FROM T"), ["b,Z"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.5] LIKE under
/// CASE_INSENSITIVE folds both sides; under UNICODE it is refused (SQLCODE
/// -900), an implementation-defined choice (IA060); COLLATE UCS_BASIC is
/// exact.
#[test]
fn like_follows_the_collation_8_5() {
    let db = collated();
    let a = db.session("A");
    assert_eq!(query(&a, "SELECT AGE FROM P WHERE NAME LIKE 'B%'"), ["30"]);
    assert_eq!(query(&a, "SELECT AGE FROM P WHERE NAME LIKE 'b%'"), ["30"]);
    assert_eq!(query(&a, "SELECT AGE FROM P WHERE NAME COLLATE UCS_BASIC LIKE 'b%'").len(), 0);
    assert_eq!(a.open_cursor("SELECT 1 FROM W WHERE WORD LIKE 'a%'").err().map(|e| e.code), Some(Code::Unsupported));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.43 GR5, §4.3.2] a
/// collation defined from another takes its order, and its pad unless it
/// says one; NO PAD counts trailing spaces, in a comparison and UNIQUE.
#[test]
fn create_collation_11_43() {
    let db = collated();
    let a = db.session("A");
    a.run("CREATE COLLATION TIGHT FOR SQL_TEXT FROM CASE_INSENSITIVE NO PAD").unwrap();
    a.run("CREATE TABLE E (C VARCHAR(9) COLLATE A.TIGHT UNIQUE)").unwrap();
    a.run("INSERT INTO E VALUES ('a'), ('a ')").unwrap();
    assert_eq!(a.run("INSERT INTO E VALUES ('A')").err().map(|e| e.code), Some(Code::Constraint));
    a.rollback().unwrap();
    a.run("CREATE COLLATION TIGHT FOR SQL_TEXT FROM CASE_INSENSITIVE NO PAD").unwrap();
    assert_eq!(a.run("CREATE COLLATION TIGHT FOR SQL_TEXT FROM UNICODE").err().map(|e| e.code), Some(Code::SyntaxRule), "SR3");
    assert_eq!(a.run("CREATE COLLATION X FOR SQL_TEXT FROM NO_SUCH").err().map(|e| e.code), Some(Code::SyntaxRule), "SR4");
    assert_eq!(a.run("CREATE COLLATION X FOR LATIN1 FROM UNICODE").err().map(|e| e.code), Some(Code::Unsupported), "one character set");
    assert_eq!(a.run("CREATE TABLE F (C INTEGER COLLATE UNICODE)").err().map(|e| e.code), Some(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.15] a join of two columns of
/// one collation matches strings equal by it, by a hash join; NATURAL too,
/// and a LEFT join; a view and a derived table keep their columns'
/// collations; a subquery's IN and EXISTS compare by them.
#[test]
fn joins_and_subqueries_follow_the_collation_9_15() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE L (K VARCHAR(9) COLLATE CASE_INSENSITIVE, V INTEGER)
           CREATE TABLE R (K VARCHAR(9) COLLATE CASE_INSENSITIVE, W INTEGER)
           CREATE VIEW V AS SELECT K FROM L",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO L VALUES ('a', 1), ('b', 2)").unwrap();
    a.run("INSERT INTO R VALUES ('A', 10), ('c', 30)").unwrap();
    assert_eq!(query(&a, "SELECT L.K, W FROM L, R WHERE L.K = R.K"), ["a,10"]);
    assert_eq!(query(&a, "SELECT V, W FROM L JOIN R ON L.K = R.K"), ["1,10"]);
    assert_eq!(query(&a, "SELECT V, W FROM L LEFT JOIN R ON L.K = R.K ORDER BY V"), ["1,10", "2,NULL"]);
    assert_eq!(query(&a, "SELECT V, W FROM L NATURAL JOIN R"), ["1,10"]);
    assert_eq!(query(&a, "SELECT K FROM V WHERE K = 'A'"), ["a"]);
    assert_eq!(query(&a, "SELECT K FROM (SELECT K FROM L) AS D WHERE K = 'B'"), ["b"]);
    assert_eq!(query(&a, "SELECT V FROM L WHERE K IN (SELECT K FROM R)"), ["1"]);
    assert_eq!(query(&a, "SELECT V FROM L WHERE K = ANY (SELECT K FROM R)"), ["1"]);
    assert_eq!(query(&a, "SELECT V FROM L WHERE (K, V) = (SELECT 'A', 1 FROM R WHERE W = 10)"), ["1"]);
    assert_eq!(query(&a, "SELECT V FROM L WHERE UNIQUE (SELECT K FROM R UNION ALL SELECT 'a' FROM R)").len(), 0);
    assert_eq!(query(&a, "SELECT V FROM L WHERE V = 1 AND K MATCH (SELECT K FROM R)"), ["1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.15 SR4a; ISO/IEC 9075:1992
/// §11.8] a foreign key matches by its columns' collation, one of a
/// different collation is refused, and a cascade follows a change only if
/// the collation sees one. MATCH PARTIAL of a collated column is refused.
#[test]
fn foreign_keys_follow_the_collation_9_15() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE P (K VARCHAR(9) COLLATE CASE_INSENSITIVE PRIMARY KEY)
           CREATE TABLE C (K VARCHAR(9) COLLATE CASE_INSENSITIVE REFERENCES P ON UPDATE CASCADE ON DELETE CASCADE)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO P VALUES ('bob')").unwrap();
    a.run("INSERT INTO C VALUES ('BOB')").unwrap();
    assert_eq!(a.run("INSERT INTO C VALUES ('ann')").err().map(|e| e.code), Some(Code::Constraint));
    a.rollback().unwrap();
    a.run("INSERT INTO P VALUES ('bob')").unwrap();
    a.run("INSERT INTO C VALUES ('BOB')").unwrap();
    a.run("UPDATE P SET K = 'Bob'").unwrap();
    assert_eq!(query(&a, "SELECT K FROM C"), ["BOB"], "equal by the collation: no change to cascade");
    a.run("UPDATE P SET K = 'zed'").unwrap();
    assert_eq!(query(&a, "SELECT K FROM C"), ["zed"]);
    a.run("DELETE FROM P").unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM C"), ["0"]);
    a.commit().unwrap();
    let other = "CREATE TABLE D (K VARCHAR(9) COLLATE UNICODE REFERENCES P)";
    assert_eq!(a.run(other).err().map(|e| e.code), Some(Code::SyntaxRule), "SR4a");
    let partial = "CREATE TABLE D (K VARCHAR(9) COLLATE CASE_INSENSITIVE REFERENCES P MATCH PARTIAL)";
    assert_eq!(a.run(partial).err().map(|e| e.code), Some(Code::Unsupported));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.44 SR4, GR2] DROP COLLATION
/// reverts its columns to the character set's collation, the rows copied
/// and the UNIQUE index rebuilt; it fails and leaves all as it was if that
/// breaks a constraint; RESTRICT is refused while a view names it, and
/// CASCADE takes the view.
#[test]
fn drop_collation_reverts_its_columns_11_44() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE COLLATION FOLD FOR SQL_TEXT FROM CASE_INSENSITIVE NO PAD
           CREATE TABLE T (C VARCHAR(9) COLLATE A.FOLD UNIQUE)
           CREATE VIEW V AS SELECT C FROM T WHERE C COLLATE A.FOLD = 'x'",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES ('a'), ('b ')").unwrap();
    a.commit().unwrap();
    assert_eq!(a.run("DROP COLLATION A.FOLD RESTRICT").err().map(|e| e.code), Some(Code::SyntaxRule), "SR4");
    a.rollback().unwrap();
    a.run("DROP COLLATION A.FOLD CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(a.open_cursor("SELECT C FROM V").err().map(|e| e.code), Some(Code::SyntaxRule), "the view went");
    a.rollback().unwrap();
    assert_eq!(query(&a, "SELECT C FROM T WHERE C = 'A'").len(), 0, "of the default now");
    assert_eq!(query(&a, "SELECT C FROM T WHERE C = 'a'"), ["a"]);
    a.run("INSERT INTO T VALUES ('A')").unwrap();
    assert_eq!(a.run("INSERT INTO T VALUES ('b')").err().map(|e| e.code), Some(Code::Constraint), "PAD SPACE, the default's");
    a.rollback().unwrap();
    assert_eq!(a.run("DROP COLLATION A.NOTHING CASCADE").err().map(|e| e.code), Some(Code::SyntaxRule), "SR2");
    a.rollback().unwrap();

    db.session("A").run("CREATE COLLATION TIGHT FOR SQL_TEXT FROM UCS_BASIC NO PAD").unwrap();
    let a = db.session("A");
    a.run("CREATE TABLE E (C VARCHAR(9) COLLATE A.TIGHT UNIQUE)").unwrap();
    a.run("INSERT INTO E VALUES ('a'), ('a ')").unwrap();
    a.commit().unwrap();
    assert_eq!(a.run("DROP COLLATION A.TIGHT CASCADE").err().map(|e| e.code), Some(Code::Constraint), "two rows equal once padded");
    a.rollback().unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM E"), ["2"]);
    a.run("INSERT INTO E VALUES ('a  ')").unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.36; ISO/IEC 9075-2:2023 §10.7
/// AR1] COLLATE needs USAGE on a collation a definition made; a built-in
/// is everyone's; GRANT gives it, and REVOKE takes it back.
#[test]
fn usage_on_a_collation_10_7() {
    let db = collated();
    let a = db.session("A");
    a.run("CREATE COLLATION MINE FOR SQL_TEXT FROM UNICODE").unwrap();
    a.run("GRANT SELECT ON N TO B").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    let refused = b.open_cursor("SELECT 1 FROM A.N WHERE 'x' COLLATE A.MINE = 'y'");
    assert_eq!(refused.err().map(|e| e.code), Some(Code::Privilege));
    b.rollback().unwrap();
    a.run("GRANT USAGE ON COLLATION A.MINE TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&b, "SELECT 1 FROM A.N WHERE 'x' COLLATE A.MINE = 'X'").len(), 0);
    let seen = "SELECT COLLATION_NAME FROM INFORMATION_SCHEMA.COLLATIONS WHERE COLLATION_SCHEMA = 'A'";
    assert_eq!(query(&b, seen), ["MINE"], "§21.2: what it may use");
    b.rollback().unwrap();
    a.run("REVOKE USAGE ON COLLATION A.MINE FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(b.open_cursor("SELECT 1 FROM A.N WHERE 'x' COLLATE A.MINE = 'y'").err().map(|e| e.code), Some(Code::Privilege));
    assert_eq!(b.open_cursor("SELECT 1 FROM A.N WHERE 'x' COLLATE UNICODE = 'y'").map(|_| ()).err().map(|e| e.code), None, "a built-in");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.15 SR4c, §19.10] a session
/// collation overrides a column's in the session's own statements, but not
/// an explicit COLLATE; a view keeps what it was defined with; SET NO
/// COLLATION ends it; a name that is none is 2H000; a statement prepared
/// keeps the collation it was prepared under.
#[test]
fn a_session_collation_19_10() {
    let db = collated();
    let a = db.session("A");
    a.run("CREATE VIEW SEEN AS SELECT S FROM N WHERE S = 'zebra'").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT S FROM N WHERE S = 'zebra'").len(), 0);
    let before = a.prepare("SELECT S FROM N WHERE S = 'zebra'").unwrap();
    a.run("SET COLLATION 'CASE_INSENSITIVE'").unwrap();
    assert_eq!(query(&a, "SELECT S FROM N WHERE S = 'zebra'"), ["Zebra"]);
    assert_eq!(query(&a, "SELECT S FROM N WHERE S COLLATE UCS_BASIC = 'zebra'").len(), 0, "SR4b first");
    assert_eq!(query(&a, "SELECT S FROM SEEN").len(), 0, "a view is as it was defined");
    assert_eq!(lines(a.open_prepared(&before, &[]).unwrap()).len(), 0, "prepared before it");
    let after = a.prepare("SELECT S FROM N WHERE S = 'zebra'").unwrap();
    a.run("SET NO COLLATION").unwrap();
    assert_eq!(lines(a.open_prepared(&after, &[]).unwrap()), ["Zebra"], "prepared under it");
    assert_eq!(query(&a, "SELECT S FROM N WHERE S = 'zebra'").len(), 0);
    let refused = a.run("SET COLLATION 'NOTHING'").err().map(|e| e.sqlstate().as_str().to_owned());
    assert_eq!(refused.as_deref(), Some("2H000"), "GR1a");
    assert_eq!(a.run("SET COLLATION 1").err().map(|e| e.code), Some(Code::SyntaxRule), "SR1");
    a.run("SET COLLATION ' UNICODE ' FOR SQL_TEXT").unwrap();
    assert_eq!(a.open_cursor("SELECT S FROM N WHERE S LIKE 'a%'").err().map(|e| e.code), Some(Code::Unsupported), "LIKE too");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.4 GR13] COLLATION FOR names a
/// value's own collation, a form COLLATE takes, whatever the session's;
/// of a column of none, the character set's.
#[test]
fn collation_for_6_4() {
    let db = collated();
    let a = db.session("A");
    assert_eq!(query(&a, "SELECT COLLATION FOR (NAME) FROM P WHERE AGE = 30"), ["CASE_INSENSITIVE"]);
    assert_eq!(query(&a, "SELECT COLLATION FOR (S) FROM N WHERE S = 'apple'"), ["SQL_TEXT"]);
    assert_eq!(query(&a, "SELECT COLLATION FOR (S COLLATE UNICODE) FROM N WHERE S = 'apple'"), ["UNICODE"]);
    assert_eq!(a.open_cursor("SELECT COLLATION FOR (AGE) FROM P").err().map(|e| e.code), Some(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] RULES.md R-ERR-5, [ISO/IEC 9075-2:2023 §11.44 SR4] a
/// DROP COLLATION refused leaves no effect, in the transaction that goes on:
/// its table's rows and collation are as they were, for a statement after
/// (a defect: RESTRICT reverted the columns before it refused).
#[test]
fn a_drop_collation_refused_leaves_no_effect() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE COLLATION TIGHT FOR SQL_TEXT FROM CASE_INSENSITIVE NO PAD
           CREATE TABLE T (C VARCHAR(9) COLLATE A.TIGHT)
           CREATE VIEW V AS SELECT C FROM T WHERE C COLLATE A.TIGHT = 'x'",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES ('Bob')").unwrap();
    assert_eq!(a.run("DROP COLLATION A.TIGHT RESTRICT").err().map(|e| e.code), Some(Code::SyntaxRule));
    assert_eq!(query(&a, "SELECT C FROM T WHERE C = 'BOB'"), ["Bob"], "of the collation still");
    a.run("DROP COLLATION A.TIGHT CASCADE").unwrap();
    assert_eq!(query(&a, "SELECT C FROM T WHERE C = 'Bob'"), ["Bob"], "the row kept, reverted");
    assert_eq!(query(&a, "SELECT C FROM T WHERE C = 'BOB'").len(), 0);
}

/// Verifies: [EXTENSION] RULES.md R-ERR-5, [ISO/IEC 9075-2:2023 §11.44 GR2] a
/// DROP COLLATION that fails on its second table, its first done, leaves the
/// transaction able only to roll back (SQLCODE -913), and the rollback
/// leaves both tables' rows and collations as they were.
#[test]
fn a_drop_collation_failing_on_a_later_table_only_rolls_back() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE COLLATION TIGHT FOR SQL_TEXT FROM UCS_BASIC NO PAD
           CREATE TABLE T1 (C VARCHAR(9) COLLATE A.TIGHT)
           CREATE TABLE T2 (C VARCHAR(9) COLLATE A.TIGHT UNIQUE)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T1 VALUES ('kept')").unwrap();
    a.run("INSERT INTO T2 VALUES ('a'), ('a ')").unwrap();
    a.commit().unwrap();
    assert_eq!(a.run("DROP COLLATION A.TIGHT CASCADE").err().map(|e| e.code), Some(Code::Constraint), "equal once padded");
    assert_eq!(a.commit().err().map(|e| e.code), Some(Code::CommitFailed));
    a.rollback().ok();
    assert_eq!(query(&a, "SELECT C FROM T1"), ["kept"]);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM T2"), ["2"]);
    assert_eq!(query(&a, "SELECT COLLATION FOR (C) FROM T1"), ["A.TIGHT"]);
}
