//! Values of structured types compared by their orderings [ISO/IEC
//! 9075-2:2023 §4.13, §8.2 GR, §9.10 to §9.14, §11.65]: by MAP, each
//! comparison, grouping and ordering operation compares the map function's
//! values; by RELATIVE, a comparison is the relative function's value with
//! 0, and a sort or a grouping is refused; by STATE, values compare by
//! their attributes; of EQUALS ONLY, no ordering operation is taken.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::rows;

/// Schema A: PT of X and Y, ordered by MAP of X + Y; table T of four
/// points whose keys are 6, 6, 1 and 18.
fn mapped() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    for s in [
        "CREATE TYPE PT AS (X INTEGER, Y INTEGER) NOT FINAL",
        "CREATE FUNCTION PK (P PT) RETURNS INTEGER CONTAINS SQL DETERMINISTIC RETURN P.X + P.Y",
        "CREATE ORDERING FOR PT ORDER FULL BY MAP WITH FUNCTION PK (PT)",
        "CREATE TABLE T (K INTEGER, P PT)",
        "INSERT INTO T VALUES (1, NEW PT().X(1).Y(5)), (2, NEW PT().X(3).Y(3)), (3, NEW PT().X(0).Y(1)), (4, NEW PT().X(9).Y(9))",
    ] {
        a.run(s).unwrap_or_else(|e| panic!("{s}: {e}"));
    }
    a.commit().unwrap();
    (db, a)
}

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// Asserts `sql` in `s`, read by a cursor if a query, is refused with
/// SQLCODE class `code`, SQLSTATE `state` and a message citing `cited`;
/// rolls back.
fn refused(s: &Session, sql: &str, (code, state, cited): (Code, &str, &str)) {
    let e = if sql.starts_with("SELECT") { s.open_cursor(sql).err() } else { s.run(sql).err() };
    let e = e.unwrap_or_else(|| panic!("{sql} was not refused"));
    let _ = s.rollback();
    let msg = e.to_string();
    assert_eq!((e.code, e.sqlstate().as_str()), (code, state), "{sql}: {msg}");
    assert!(msg.contains(cited), "{sql}: {msg} does not cite {cited}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.2 GR, §9.11, §11.65] Values of a
/// type ordered by MAP compare by their map function's values: = and <>,
/// BETWEEN, IS DISTINCT FROM, IN and quantified comparisons over a
/// subquery, EXISTS of a correlated equality, NULLIF and a simple CASE all
/// take (1, 5) and (3, 3), both of key 6, as equal.
#[test]
fn comparisons_are_of_the_keys_8_2() {
    let (_db, a) = mapped();
    assert_eq!(query(&a, "SELECT K FROM T WHERE P = NEW PT().X(6).Y(0) ORDER BY K"), ["1", "2"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE P <> NEW PT().X(6).Y(0) ORDER BY K"), ["3", "4"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE P BETWEEN NEW PT().X(0).Y(2) AND NEW PT().X(3).Y(3) ORDER BY K"), ["1", "2"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE P IS DISTINCT FROM NEW PT().X(6).Y(0) ORDER BY K"), ["3", "4"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE P IN (SELECT P FROM T WHERE K = 2) ORDER BY K"), ["1", "2"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE P = ANY (SELECT P FROM T WHERE K = 1) ORDER BY K"), ["1", "2"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE P > ALL (SELECT P FROM T WHERE K < 3) ORDER BY K"), ["4"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE P IN (NEW PT().X(6).Y(0), NEW PT().X(0).Y(0)) ORDER BY K"), ["1", "2"]);
    assert_eq!(query(&a, "SELECT K FROM T T1 WHERE EXISTS (SELECT 1 FROM T T2 WHERE T2.P = T1.P AND T2.K <> T1.K) ORDER BY K"), ["1", "2"]);
    assert_eq!(query(&a, "SELECT NULLIF(P, NEW PT().X(6).Y(0)) IS NULL FROM T ORDER BY K"), ["TRUE", "TRUE", "FALSE", "FALSE"]);
    assert_eq!(query(&a, "SELECT CASE P WHEN NEW PT().X(6).Y(0) THEN 1 ELSE 0 END FROM T ORDER BY K"), ["1", "1", "0", "0"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.12, §9.14] A grouping or ordering
/// operation of values ordered by MAP compares their keys: ORDER BY, a
/// window's ORDER BY and PARTITION BY, GROUP BY, DISTINCT, COUNT(DISTINCT),
/// MAX and MIN, GREATEST, UNION, EXCEPT, the UNIQUE predicate, an
/// aggregate's ORDER BY, and a row pattern's PARTITION BY.
#[test]
fn groupings_and_orderings_are_of_the_keys_9_12() {
    let (_db, a) = mapped();
    assert_eq!(query(&a, "SELECT K FROM T ORDER BY P, K"), ["3", "1", "2", "4"]);
    assert_eq!(query(&a, "SELECT K, RANK() OVER (ORDER BY P) FROM T ORDER BY K"), ["1,2", "2,2", "3,1", "4,4"]);
    assert_eq!(query(&a, "SELECT K, COUNT(*) OVER (PARTITION BY P) FROM T ORDER BY K"), ["1,2", "2,2", "3,1", "4,1"]);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM T GROUP BY P ORDER BY 1"), ["1", "1", "2"]);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM (SELECT DISTINCT P FROM T) AS D"), ["3"]);
    assert_eq!(query(&a, "SELECT COUNT(DISTINCT P) FROM T"), ["3"]);
    assert_eq!(query(&a, "SELECT MAX(P).X, MIN(P).X FROM T"), ["9,0"]);
    assert_eq!(query(&a, "SELECT GREATEST(P, NEW PT().X(4).Y(4)).X FROM T ORDER BY K"), ["4", "4", "4", "9"]);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM (SELECT P FROM T UNION SELECT P FROM T) AS U"), ["3"]);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM (SELECT P FROM T EXCEPT SELECT P FROM T WHERE K = 1) AS E"), ["2"]);
    assert_eq!(query(&a, "SELECT UNIQUE (SELECT P FROM T WHERE K < 3) FROM T WHERE K = 1"), ["FALSE"]);
    assert_eq!(query(&a, "SELECT LISTAGG(CAST(K AS VARCHAR(3)), ',') WITHIN GROUP (ORDER BY P, K) FROM T"), ["3,1,2,4"]);
    assert_eq!(query(&a, "SELECT ARRAY_AGG(K ORDER BY P, K) FROM T"), ["ARRAY[3, 1, 2, 4]"]);
    let pattern = "SELECT K, N FROM T MATCH_RECOGNIZE (PARTITION BY P ORDER BY K MEASURES COUNT(*) AS N \
                   ALL ROWS PER MATCH PATTERN (A+) DEFINE A AS TRUE) AS R ORDER BY K";
    assert_eq!(query(&a, pattern), ["1,1", "2,2", "3,1", "4,1"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.13 GR, §9.12] Grouping sets of a
/// type ordered by MAP group by its keys: ROLLUP, GROUPING SETS with the
/// empty set, and CUBE.
#[test]
fn grouping_sets_are_of_the_keys_7_13() {
    let (_db, a) = mapped();
    assert_eq!(
        query(&a, "SELECT PK(P), COUNT(*), GROUPING(P) FROM T GROUP BY ROLLUP (P) ORDER BY 3, 1"),
        ["1,1,0", "6,2,0", "18,1,0", "NULL,4,1"]
    );
    assert_eq!(
        query(&a, "SELECT COUNT(*) FROM T GROUP BY GROUPING SETS ((P), (K, P), ()) ORDER BY 1"),
        ["1", "1", "1", "1", "1", "1", "2", "4"]
    );
    assert_eq!(
        query(&a, "SELECT COUNT(*) FROM T GROUP BY CUBE (P, K) HAVING GROUPING(K) = 1 AND GROUPING(P) = 0 ORDER BY 1"),
        ["1", "1", "2"]
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.11, §9.13] The elements of a
/// multiset of a type ordered by MAP compare by their keys: SET, MEMBER,
/// SUBMULTISET, IS A SET, MULTISET UNION DISTINCT and EXCEPT, the equality
/// of two multisets, COLLECT(DISTINCT) and INTERSECTION; SET of a null
/// multiset is null.
#[test]
fn multiset_elements_are_of_the_keys_9_13() {
    let (_db, a) = mapped();
    let (six, other) = ("NEW PT().X(1).Y(5)", "NEW PT().X(6).Y(0)");
    let one = |sql: String| query(&a, &format!("SELECT {sql} FROM T WHERE K = 1"));
    assert_eq!(one(format!("CARDINALITY(SET(MULTISET[{six}, {other}]))")), ["1"]);
    assert_eq!(one(format!("NEW PT().X(3).Y(3) MEMBER OF MULTISET[{six}]")), ["TRUE"]);
    assert_eq!(one(format!("MULTISET[{six}] SUBMULTISET OF MULTISET[{other}]")), ["TRUE"]);
    assert_eq!(one(format!("MULTISET[{six}, {other}] IS A SET")), ["FALSE"]);
    assert_eq!(one(format!("CARDINALITY(MULTISET[{six}] MULTISET UNION DISTINCT MULTISET[{other}])")), ["1"]);
    assert_eq!(one(format!("CARDINALITY(MULTISET[{six}] MULTISET EXCEPT MULTISET[{other}])")), ["0"]);
    assert_eq!(one(format!("MULTISET[{six}] = MULTISET[{other}]")), ["TRUE"]);
    assert_eq!(one("CARDINALITY(SET(CAST(NULL AS PT MULTISET))) IS NULL".to_owned()), ["TRUE"]);
    assert_eq!(query(&a, "SELECT CARDINALITY(COLLECT(DISTINCT P)) FROM T"), ["3"]);
    assert_eq!(query(&a, "SELECT CARDINALITY(INTERSECTION(M)) FROM (SELECT MULTISET[P] FROM T WHERE K < 3) AS D (M)"), ["1"]);
    assert_eq!(
        query(
            &a,
            "SELECT CARDINALITY(MULTISET(SELECT P FROM T) MULTISET INTERSECT DISTINCT MULTISET[NEW PT().X(5).Y(1)]) FROM T WHERE K = 1"
        ),
        ["1"]
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.2 GR, §9.12 SR3, §9.14 SR3, §11.65]
/// Of a type ordered by RELATIVE, `x op y` is `RF(x, y) op 0`, of = and
/// of < alike; a sort or a grouping of its values, which have no key, is
/// valid SQL refused as RULES.md D81 says (SQLCODE -900).
#[test]
fn relative_orderings_compare_by_the_function_11_65() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    for s in [
        "CREATE TYPE PT AS (X INTEGER, Y INTEGER) NOT FINAL",
        "CREATE FUNCTION PR (P PT, Q PT) RETURNS INTEGER CONTAINS SQL DETERMINISTIC \
         RETURN CASE WHEN P.X + P.Y < Q.X + Q.Y THEN -1 WHEN P.X + P.Y > Q.X + Q.Y THEN 1 ELSE 0 END",
        "CREATE ORDERING FOR PT ORDER FULL BY RELATIVE WITH FUNCTION PR (PT, PT)",
        "CREATE TABLE T (K INTEGER, P PT)",
        "INSERT INTO T VALUES (1, NEW PT().X(1).Y(5)), (2, NEW PT().X(3).Y(3)), (3, NEW PT().X(0).Y(1))",
    ] {
        a.run(s).unwrap_or_else(|e| panic!("{s}: {e}"));
    }
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT K FROM T WHERE P = NEW PT().X(6).Y(0) ORDER BY K"), ["1", "2"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE P < NEW PT().X(2).Y(2) ORDER BY K"), ["3"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE P IN (SELECT P FROM T WHERE K = 1) ORDER BY K"), ["1", "2"]);
    for sql in ["SELECT K FROM T ORDER BY P", "SELECT COUNT(*) FROM T GROUP BY P", "SELECT COUNT(DISTINCT P) FROM T"] {
        refused(&a, sql, (Code::Unsupported, "0A000", "RULES.md D81"));
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.13, §9.10 SR3, §9.14 SR3, §11.65
/// SR6a ii] An ordering by STATE is EQUALS ONLY: its values compare for
/// equality by their attributes, and no ordering operation takes them;
/// a type of no ordering is not even compared with =.
#[test]
fn state_and_equals_only_orderings_11_65() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    for s in [
        "CREATE TYPE E AS (X INTEGER, Y INTEGER) NOT FINAL",
        "CREATE ORDERING FOR E EQUALS ONLY BY STATE",
        "CREATE TYPE N AS (X INTEGER) NOT FINAL",
        "CREATE TABLE T (K INTEGER, E E, N N)",
        "INSERT INTO T VALUES (1, NEW E().X(2).Y(1), NEW N().X(2)), (2, NEW E().X(1).Y(2), NEW N().X(1))",
    ] {
        a.run(s).unwrap_or_else(|e| panic!("{s}: {e}"));
    }
    a.commit().unwrap();
    refused(&a, "CREATE ORDERING FOR N ORDER FULL BY STATE", (Code::SyntaxRule, "2A000", "§11.65 SR6a ii"));
    assert_eq!(query(&a, "SELECT K FROM T WHERE E = NEW E().X(1).Y(2)"), ["2"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE E <> NEW E().X(1).Y(2)"), ["1"]);
    refused(&a, "SELECT K FROM T ORDER BY E", (Code::SyntaxRule, "2A000", "§9.14 SR3"));
    refused(&a, "SELECT K FROM T WHERE N = N", (Code::SyntaxRule, "2A000", "§9.10 SR3"));
}
