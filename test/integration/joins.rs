//! Joins (RULES.md D35): the rows of a product that its conditions keep,
//! however the planner computes them. These are `[DESIGN]` requirements on
//! the plan; the standard governs only the rows [X3.135-1986 §5.20 GR2b,
//! §5.21 GR2]. Each query is checked against the same query written so that
//! the plan is a nested loop: `+ 0` makes an equality one that can raise an
//! error, which the planner leaves to be evaluated on every pair.

use turborel::db::{Database, Session};
use turborel::params::Params;

use crate::common::{rows, sorted};

/// Schema J: numbers exact (A), approximate (B) and exact again (E), and
/// strings of two lengths (C, D), with nulls and repeated keys.
fn joins() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION J
           CREATE TABLE A (K DECIMAL(5,2), N INTEGER)
           CREATE TABLE B (K FLOAT, N INTEGER)
           CREATE TABLE E (K INTEGER, N INTEGER)
           CREATE TABLE C (S CHAR(4), N INTEGER)
           CREATE TABLE D (S CHAR(2), N INTEGER)",
    )
    .unwrap();
    let s = db.session("J");
    for sql in [
        "INSERT INTO A VALUES (1.50, 1)",
        "INSERT INTO A VALUES (2, 2)",
        "INSERT INTO A VALUES (NULL, 3)",
        "INSERT INTO A VALUES (3, 4)",
        "INSERT INTO A VALUES (2.00, 5)",
        "INSERT INTO B VALUES (1.5E0, 10)",
        "INSERT INTO B VALUES (2E0, 20)",
        "INSERT INTO B VALUES (NULL, 30)",
        "INSERT INTO B VALUES (4E0, 40)",
        "INSERT INTO B VALUES (2E0, 50)",
        "INSERT INTO E VALUES (2, 7)",
        "INSERT INTO E VALUES (3, 8)",
        "INSERT INTO C VALUES ('AB', 1)",
        "INSERT INTO C VALUES ('AB  ', 2)",
        "INSERT INTO C VALUES ('X', 3)",
        "INSERT INTO D VALUES ('AB', 4)",
        "INSERT INTO D VALUES ('X ', 5)",
        "INSERT INTO D VALUES (NULL, 6)",
    ] {
        s.run(sql).unwrap();
    }
    s.commit().unwrap();
    db
}

/// The rows of `sql`, sorted, checked against those of `nested`, the same
/// query planned as a nested loop; and the plan of `sql`. No ORDER BY is
/// written, so the rows may come in any order.
fn same_rows(s: &Session, sql: &str, nested: &str) -> (Vec<String>, String) {
    let (found, reference) = (sorted(s, sql), sorted(s, nested));
    assert_eq!(found, reference, "{sql}");
    let plan = s.physical_plan(sql, &Params::new()).unwrap();
    assert!(!s.physical_plan(nested, &Params::new()).unwrap().contains("HashJoin"), "{nested} is a nested loop");
    (found, plan)
}

/// Verifies: [DESIGN] RULES.md D35: an equality of a column of each operand
/// of a product is a hash join, which keeps the pairs whose values are equal
/// [X3.135-1986 §5.11]: an exact and an approximate number by algebraic
/// value (GR4), strings with padding (GR5), never a null (GR2), each
/// repeated key with each.
#[test]
fn equalities_are_hash_joins() {
    let db = joins();
    let s = db.session("J");
    let cases = [
        ("SELECT A.N, B.N FROM A, B WHERE A.K = B.K", "SELECT A.N, B.N FROM A, B WHERE A.K + 0 = B.K", 5),
        ("SELECT A.N, B.N FROM A, B WHERE B.K = A.K", "SELECT A.N, B.N FROM A, B WHERE B.K + 0 = A.K", 5),
        ("SELECT A.N, E.N FROM A, E WHERE A.K = E.K", "SELECT A.N, E.N FROM A, E WHERE A.K + 0 = E.K", 3),
        ("SELECT C.N, D.N FROM C, D WHERE C.S = D.S", "SELECT C.N, D.N FROM C, D WHERE C.S = D.S AND C.N + 0 > 0", 3),
        (
            "SELECT A.N, B.N FROM A, B WHERE A.K = B.K AND (A.N = 1 OR B.N = 50)",
            "SELECT A.N, B.N FROM A, B WHERE A.K + 0 = B.K AND (A.N = 1 OR B.N = 50)",
            3,
        ),
        (
            "SELECT A.N, B.N, E.N FROM A, B, E WHERE A.K = B.K AND B.K = E.K",
            "SELECT A.N, B.N, E.N FROM A, B, E WHERE A.K + 0 = B.K AND B.K = E.K",
            4,
        ),
    ];
    for (sql, nested, n) in cases {
        let (found, plan) = same_rows(&s, sql, nested);
        assert_eq!(found.len(), n, "{sql}: {found:?}");
        assert!(plan.contains("HashJoin"), "{sql}: {plan}");
    }
}

/// Verifies: [DESIGN] RULES.md D15: a condition that can raise an error is
/// evaluated on every pair of the product, so it is not a hash join, which
/// would evaluate it only on the pairs whose keys are equal.
#[test]
fn a_condition_that_can_raise_is_not_a_hash_join() {
    let db = joins();
    let s = db.session("J");
    let plan = s.physical_plan("SELECT A.N FROM A, B WHERE A.K = B.K AND A.N / B.N > 0", &Params::new()).unwrap();
    assert!(plan.contains("NestedLoop") && !plan.contains("HashJoin"), "{plan}");
}

/// Verifies: [DESIGN] RULES.md D35: the operands of a product are joined in
/// the order estimated cheapest, not the FROM clause's: two large tables
/// each joined to a small one are joined through it, not paired first; the
/// rows, and their columns' order, are the same. A table whose order an
/// ORDER BY asks for stays first, where its order can be kept.
#[test]
fn products_are_joined_in_the_cheapest_order() {
    let db = large_and_small();
    let s = db.session("J");
    let sql = "SELECT T.V, U.W, S.K FROM T, U, S WHERE T.K = S.K AND U.K = S.K";
    let nested = "SELECT T.V, U.W, S.K FROM T, U, S WHERE T.K + 0 = S.K AND U.K = S.K";
    let (found, plan) = same_rows(&s, sql, nested);
    assert_eq!(found, vec!["10,15,5", "14,21,7"]);
    assert!(!plan.contains("NestedLoop"), "T and U are not paired: {plan}");
    let ordered = "SELECT T.K, U.W FROM T, U, S WHERE T.K = S.K AND U.K = S.K ORDER BY 2 DESC";
    let plan = s.physical_plan(ordered, &Params::new()).unwrap();
    let first = plan.lines().find(|l| l.contains("Scan J.")).unwrap_or_default();
    assert!(first.contains("J.U"), "U is read first: {plan}");
    assert_eq!(rows(&s, ordered), Ok(vec!["7,21".into(), "5,15".into()]));
}

/// Verifies: [DESIGN] RULES.md D35, `PREDICATE_TRANSITIVITY`: a comparison
/// of a column equated with another is made of the other too [X3.135-1986
/// §5.11 GR4], so U's index answers `T.K = 7` through `T.K = U.K`, and a
/// range on T restricts U; the rows are those of the nested loop.
#[test]
fn comparisons_carry_across_equalities() {
    let db = large_and_small();
    db.create_index("U_K", "J.U", &["K"], "btree").unwrap();
    let s = db.session("J");
    let (found, plan) =
        same_rows(&s, "SELECT T.V, U.W FROM T, U WHERE T.K = U.K AND T.K = 7", "SELECT T.V, U.W FROM T, U WHERE T.K + 0 = U.K AND T.K = 7");
    assert_eq!(found, vec!["14,21"]);
    assert!(plan.contains("IndexScan J.U using U_K"), "{plan}");
    let (found, plan) =
        same_rows(&s, "SELECT U.W FROM T, U WHERE U.K = T.K AND 298 <= T.K", "SELECT U.W FROM T, U WHERE U.K + 0 = T.K AND 298 <= T.K");
    assert_eq!(found, vec!["894", "897"]);
    assert!(plan.contains("U.K >= 298"), "{plan}");
}

/// Schema J: tables T and U of 300 rows, keys 0 to 299, and S of three
/// keys, two of them in T and U.
fn large_and_small() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION J
           CREATE TABLE T (K INTEGER NOT NULL, V INTEGER)
           CREATE TABLE U (K INTEGER NOT NULL, W INTEGER)
           CREATE TABLE S (K INTEGER NOT NULL)",
    )
    .unwrap();
    let s = db.session("J");
    for k in 0..300 {
        s.run(&format!("INSERT INTO T VALUES ({k}, {})", k * 2)).unwrap();
        s.run(&format!("INSERT INTO U VALUES ({k}, {})", k * 3)).unwrap();
    }
    for k in [5, 7, 400] {
        s.run(&format!("INSERT INTO S VALUES ({k})")).unwrap();
    }
    s.commit().unwrap();
    db
}
