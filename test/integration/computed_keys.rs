//! \[DESIGN\] (RULES.md D1086) A join on an equality of an expression of
//! one operand's columns with one of the other's is a hash join, each
//! key computed once a row, of the row store and the column store, at
//! one degree and at four.

use turborel::db::{Database, Session};
use turborel::params::Params;

use crate::common::{exec, sorted};

/// Q, of a key K and a number D equal to it, and X, of a tag TAG and a
/// key K, each of the row store and of the column store (`store`): Q of
/// K from 0 to 89 and a null, X of K from 0 to 59, its tag K mod 3 but
/// null of every seventh row, and a row of a null key.
fn tables(store: &str) -> (Database, Session) {
    let db = Database::new();
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE Q (K INTEGER, D DECIMAL(12,2)) {store}
           CREATE TABLE X (TAG INTEGER, K INTEGER) {store}"
    ))
    .unwrap();
    let a = db.session("A");
    let q: Vec<String> = (0..90_i64).map(|k| format!("({k}, {k}.00)")).chain(["(NULL, NULL)".to_owned()]).collect();
    a.run(&format!("INSERT INTO Q VALUES {}", q.join(", "))).unwrap();
    let tag = |k: i64| if k % 7 == 0 { "NULL".to_owned() } else { (k % 3).to_string() };
    let x: Vec<String> = (0..60_i64).map(|k| format!("({}, {k})", tag(k))).chain(["(1, NULL)".to_owned()]).collect();
    a.run(&format!("INSERT INTO X VALUES {}", x.join(", "))).unwrap();
    a.commit().unwrap();
    (db, a)
}

/// Each join on computed keys, the same query whose equality is of
/// approximate arithmetic that can raise, left a nested loop, and how
/// many rows it gives: one side computed, both, a key cast to the other's
/// type, and a LEFT join, whose rows of a null key find no match.
const CASES: [(&str, &str, usize); 4] = [
    ("SELECT Q.K, X.K FROM Q, X WHERE Q.K = X.TAG + X.K", "SELECT Q.K, X.K FROM Q, X WHERE Q.K + 0E0 = X.TAG + X.K", 51),
    ("SELECT Q.K, X.K FROM Q, X WHERE Q.K * 2 = X.TAG + X.K", "SELECT Q.K, X.K FROM Q, X WHERE Q.K * 2 + 0E0 = X.TAG + X.K", 24),
    (
        "SELECT Q.K, X.K FROM Q, X WHERE Q.D = CAST(X.K AS DECIMAL(12,2))",
        "SELECT Q.K, X.K FROM Q, X WHERE Q.D + 0E0 = CAST(X.K AS DECIMAL(12,2))",
        60,
    ),
    (
        "SELECT X.K, COUNT(Q.K) FROM X LEFT JOIN Q ON Q.K = X.TAG + X.K GROUP BY X.K",
        "SELECT X.K, COUNT(Q.K) FROM X LEFT JOIN Q ON Q.K + 0E0 = X.TAG + X.K GROUP BY X.K",
        61,
    ),
];

/// Checks each of `CASES` of `a`: its rows those of its nested loop, as
/// many as it says, and its plan a hash join, `at` naming the run.
fn joined_alike(a: &Session, at: &str) {
    for (sql, nested, n) in CASES {
        let found = sorted(a, sql);
        exec(a, "SET OPTIMIZER.HOIST_RAISING_EXPRESSIONS = 'OFF'").unwrap();
        let reference = sorted(a, nested);
        let nested_plan = a.physical_plan(nested, &Params::new()).unwrap();
        exec(a, "SET OPTIMIZER.HOIST_RAISING_EXPRESSIONS = 'ON'").unwrap();
        assert!(!hashed(&nested_plan), "{at}: {nested} is a nested loop: {nested_plan}");
        assert_eq!(found, reference, "{at}: {sql}");
        assert_eq!(found.len(), n, "{at}: {sql}: {found:?}");
        let plan = a.physical_plan(sql, &Params::new()).unwrap();
        assert!(hashed(&plan) && !plan.contains("NestedLoop"), "{at}: {sql}: {plan}");
    }
}

/// Whether `plan` holds a hash join, an inner one or an outer one hashed
/// on its keys.
fn hashed(plan: &str) -> bool {
    plan.contains("HashJoin") || plan.contains("hashed on")
}

/// Verifies: [DESIGN] RULES.md D1086: an equality of an expression of one
/// operand's columns with one of the other's, `Q.K = X.TAG + X.K`, both
/// sides computed, a key cast to the other's type, and the condition of a
/// LEFT join, is a hash join on the computed keys, of the row store and
/// of the column store, at one degree and in parallel at four; its rows
/// are its nested loop's, a null key matching none [X3.135-1986 §5.11
/// GR2].
#[test]
fn an_equality_of_computed_keys_is_a_hash_join() {
    for store in ["", "USING columnstore"] {
        let (_db, a) = tables(store);
        joined_alike(&a, &format!("{store} degree 1"));
        for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 16", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
            exec(&a, set).unwrap();
        }
        joined_alike(&a, &format!("{store} degree 4"));
    }
}
