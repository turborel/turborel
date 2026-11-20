//! SQL-invoked functions inlined (RULES.md D316), and the subqueries of
//! their bodies unnested in the invoking query.

use turborel::db::Database;
use turborel::error::Code;
use turborel::optimizer::INLINE_FUNCTIONS;
use turborel::params::Params;

use super::{both, unnested};

/// Schema A: R and S, and functions over S: of a `<return statement>`,
/// of a compound statement of it alone, of one that is not, and some that
/// may not be inlined.
fn functions() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE R (K INTEGER, V SMALLINT) CREATE TABLE S (K INTEGER, W INTEGER)").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO R VALUES (1, 1), (2, 300), (3, 1), (NULL, 2), (1, 0)").unwrap();
    s.run("INSERT INTO S VALUES (1, 2), (1, NULL), (2, 1), (4, 5), (4, 6), (3, 40000)").unwrap();
    for f in [
        "CREATE FUNCTION LEAST_W (X INTEGER) RETURNS INTEGER DETERMINISTIC READS SQL DATA RETURN (SELECT MIN(W) FROM S WHERE S.K = X)",
        "CREATE FUNCTION HOW_MANY (X INTEGER) RETURNS INTEGER DETERMINISTIC READS SQL DATA BEGIN RETURN (SELECT COUNT(*) FROM S WHERE S.K = X); END",
        "CREATE FUNCTION ONE_W (X INTEGER) RETURNS SMALLINT DETERMINISTIC READS SQL DATA RETURN (SELECT W FROM S WHERE S.K = X)",
        "CREATE FUNCTION PAST (X INTEGER, Y SMALLINT) RETURNS INTEGER DETERMINISTIC READS SQL DATA RETURN (SELECT COUNT(*) FROM S WHERE S.K = X AND S.W > Y)",
        "CREATE FUNCTION MAYBE (X INTEGER) RETURNS INTEGER NOT DETERMINISTIC READS SQL DATA RETURN (SELECT MIN(W) FROM S WHERE S.K = X)",
        "CREATE FUNCTION NULLS (X INTEGER) RETURNS INTEGER DETERMINISTIC READS SQL DATA RETURNS NULL ON NULL INPUT RETURN (SELECT COUNT(*) FROM S WHERE S.K = X)",
        "CREATE FUNCTION LATER (X INTEGER) RETURNS INTEGER DETERMINISTIC READS SQL DATA BEGIN DECLARE N INTEGER; SET N = X; RETURN (SELECT COUNT(*) FROM S WHERE S.K = N); END",
        "CREATE FUNCTION UNREAD (X INTEGER, Y INTEGER) RETURNS INTEGER DETERMINISTIC READS SQL DATA RETURN (SELECT COUNT(*) FROM S WHERE S.K = X)",
    ] {
        s.run(f).unwrap();
    }
    s.commit().unwrap();
    db
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 GR4a ii, GR8b ix 2, GR10a iii;
/// §16.2 GR1] (RULES.md D316) a deterministic function that reads SQL-data
/// and returns one value, of a `<return statement>` or a compound
/// statement of that statement alone, is inlined, and its correlated
/// subquery unnested: the rows, and the errors of storing an argument in
/// its parameter and the value in the result type, the same as invoking
/// it; a function that is not deterministic, a null-call function, a body
/// of more statements and a parameter its body does not read are invoked
/// as before.
#[test]
fn functions_are_inlined_and_unnested() {
    let db = functions();
    let s = db.session("A");
    let cases: &[(&str, bool)] = &[
        ("SELECT K, LEAST_W(K) FROM R", true),
        ("SELECT K, HOW_MANY(K) FROM R", true),
        ("SELECT K, V FROM R WHERE HOW_MANY(K) > 1", true),
        ("SELECT K, PAST(K, V) FROM R", true),
        ("SELECT K, LEAST_W(K + 1) + HOW_MANY(K) FROM R", true),
        ("SELECT K, ONE_W(K) FROM R WHERE K = 2", true),
        ("SELECT K, ONE_W(K) FROM R WHERE K = 3", true),
        ("SELECT K, ONE_W(K) FROM R WHERE K = 1", true),
        ("SELECT K, PAST(K, K * 20000) FROM R", true),
        ("SELECT K, MAYBE(K) FROM R", false),
        ("SELECT K, NULLS(K) FROM R", false),
        ("SELECT K, LATER(K) FROM R", false),
        ("SELECT K, UNREAD(K, 1 / (K - 1)) FROM R", false),
    ];
    for (q, inlined) in cases {
        assert_eq!(unnested(&s, q), *inlined, "{q}");
        let plan = s.explain(q, &Params::new()).unwrap();
        assert_eq!(plan.lines().any(|l| l.starts_with("-- rules:") && l.contains(INLINE_FUNCTIONS)), *inlined, "{plan}");
        let (on, off) = both(&db, &s, q);
        assert_eq!(on, off, "{q}");
    }
    let (on, _) = both(&db, &s, "SELECT K, ONE_W(K) FROM R WHERE K = 1");
    assert_eq!(on, Err(Code::Cardinality));
}
