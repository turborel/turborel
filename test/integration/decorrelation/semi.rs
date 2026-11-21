//! Subqueries of existence made semi joins and antijoins (RULES.md D540,
//! D541): each query's rows are those of the subquery run for each row,
//! and of the subquery unnested as an outer join.

use turborel::db::{Database, Session};
use turborel::optimizer::SEMI_JOINS;
use turborel::params::Params;
use turborel::value::{Decimal, Value};

use super::{both, outcome, random_tables};
use crate::common::set_rule;

/// The forms made semi joins or antijoins, each with the operator EXPLAIN
/// shows, over R and S of `random_tables`.
const SEMI: &[(&str, &str)] = &[
    ("SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K)", "Semi"),
    ("SELECT K, V FROM R WHERE NOT EXISTS (SELECT * FROM S WHERE S.K = R.K)", "Anti"),
    ("SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K AND S.W > R.V)", "Semi"),
    ("SELECT K, V FROM R WHERE NOT EXISTS (SELECT * FROM S WHERE S.K = R.K AND S.W <> R.V)", "Anti"),
    ("SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K AND S.C = R.C)", "Semi"),
    ("SELECT K, V FROM R WHERE NOT EXISTS (SELECT * FROM S WHERE S.K = R.K AND S.C = R.C AND S.W > 0)", "Anti"),
    ("SELECT K, V FROM R WHERE V IN (SELECT W FROM S WHERE S.K = R.K)", "Semi"),
    ("SELECT K, V FROM R WHERE V IN (SELECT W FROM S)", "Semi"),
    ("SELECT K, V FROM R WHERE (K, V) IN (SELECT K, W FROM S)", "Semi"),
    ("SELECT K, V FROM R WHERE V NOT IN (SELECT W FROM S WHERE S.K = R.K)", "Anti"),
    ("SELECT K, V FROM R WHERE V = ANY (SELECT W FROM S WHERE S.K = R.K)", "Semi"),
    ("SELECT K, V FROM R WHERE V < ALL (SELECT W FROM S WHERE S.K = R.K)", "Anti"),
    ("SELECT K, V FROM R WHERE V >= SOME (SELECT W FROM S WHERE S.C = R.C)", "Semi"),
    ("SELECT K, V FROM R WHERE V > 0 AND EXISTS (SELECT DISTINCT W FROM S WHERE S.K = R.K)", "Semi"),
    ("SELECT K, W FROM S AS R WHERE EXISTS (SELECT * FROM R AS X WHERE X.K = R.K)", "Semi"),
    ("SELECT K, W FROM S AS R WHERE NOT EXISTS (SELECT * FROM R AS X WHERE X.K = R.K AND X.V < R.W)", "Anti"),
    ("SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.W > R.V)", "Semi"),
    ("SELECT K, V FROM R WHERE NOT EXISTS (SELECT * FROM S WHERE S.W > R.V)", "Anti"),
    ("SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K AND EXISTS (SELECT * FROM R AS X WHERE X.V = S.W))", "Semi"),
    ("SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K) AND NOT EXISTS (SELECT * FROM S WHERE S.W = R.V)", "Anti"),
    ("SELECT K FROM R GROUP BY K HAVING EXISTS (SELECT * FROM S WHERE S.K = R.K AND S.W > 3)", "Semi"),
    ("SELECT K, V FROM R WHERE EXISTS (SELECT S.K FROM S, R AS X WHERE S.K = R.K AND X.V = S.W)", "Semi"),
];

/// Forms left as they were: EXISTS under OR, which a semi join does not
/// decide alone, and a subquery that can raise, whose rows a semi join
/// may leave unread.
const NOT_SEMI: &[&str] = &[
    "SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K) OR V > 2",
    "SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K AND 10 / S.W > 1)",
    "SELECT K, (V IN (SELECT W FROM S WHERE S.K = R.K)) FROM R",
];

/// The physical plan of `sql`.
fn physical(s: &Session, sql: &str) -> String {
    let plan = s.explain(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"));
    plan.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// The rows of `sql` with OPTIMIZER.SEMI_JOINS OFF, unnested otherwise.
fn joined_otherwise(db: &Database, s: &Session, sql: &str) -> Result<Vec<String>, turborel::error::Code> {
    set_rule(db, SEMI_JOINS, false);
    let rows = outcome(s, sql);
    set_rule(db, SEMI_JOINS, true);
    rows
}

/// The rows of `sql` read by batches, counted and grouped as a set
/// function reads them, and read row by row (EXECUTOR.BATCHES OFF).
fn both_paths(s: &Session, sql: &str) -> (Result<Vec<String>, turborel::error::Code>, Result<Vec<String>, turborel::error::Code>) {
    let counted = format!("SELECT COUNT(*), MIN(K), MAX(K) FROM ({sql}) AS Q");
    let batched = outcome(s, &counted);
    s.run("SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let rows = outcome(s, &counted);
    s.run("SET EXECUTOR.BATCHES = 'ON'").unwrap();
    (batched, rows)
}

/// Verifies: [X3.135-1986 §5.16 GR2, §5.17 GR2, GR3] (RULES.md D540, D541)
/// every form made a semi join or an antijoin shows it in EXPLAIN and has
/// the rows the subquery has run for each row, and unnested as an outer
/// join: over random tables of nulls, duplicates and trailing spaces,
/// with the right built and the left, of an index and of none, and with a
/// work area small enough that the hashed rows spill.
#[test]
fn semi_joins_match_the_subqueries() {
    for seed in [0x5E41_0000_0000_0001, 0x5E41_0000_0000_0002, 0x5E41_0000_0000_0003] {
        let db = random_tables(seed);
        let s = db.session("A");
        for indexed in [false, true] {
            if indexed {
                s.commit().unwrap();
                s.run("CREATE INDEX SK ON S (K)").unwrap();
                s.run("CREATE INDEX RK ON R (K)").unwrap();
                s.commit().unwrap();
            }
            for (q, op) in SEMI {
                let shown = physical(&s, q);
                assert!(shown.contains(op) && !shown.contains("Subquery"), "{q}\n{shown}");
                let (on, off) = both(&db, &s, q);
                assert_eq!(on, off, "{q}");
                assert_eq!(on, joined_otherwise(&db, &s, q), "{q}");
                let (batched, rows) = both_paths(&s, q);
                assert_eq!(batched, rows, "by batches: {q}");
            }
            for q in NOT_SEMI {
                let shown = physical(&s, q);
                assert!(!shown.contains("SemiJoin") && !shown.contains("Anti"), "{q}\n{shown}");
                let (on, off) = both(&db, &s, q);
                assert_eq!(on, off, "{q}");
            }
        }
        db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(65_536, 0))).unwrap();
        for (q, _) in SEMI {
            let (on, off) = both(&db, &s, q);
            assert_eq!(on, off, "spilled: {q}");
            let (batched, rows) = both_paths(&s, q);
            assert_eq!(batched, rows, "spilled, by batches: {q}");
        }
    }
}

/// Verifies: [ISO/IEC 9075:1992 §8.7 GR2; X3.135-1986 §5.16 GR2] NOT IN
/// as an antijoin keeps a row only if no value of its binding's set
/// equals or may equal it: a null among them, or its own null of a set
/// not empty, keeps none; of an empty set it keeps the row, null or not.
#[test]
fn not_in_with_nulls_as_an_antijoin() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE R (K INTEGER, V INTEGER) CREATE TABLE S (K INTEGER, W INTEGER)").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO R VALUES (1, 1), (2, 1), (3, 1), (NULL, 1), (2, NULL), (3, NULL)").unwrap();
    s.run("INSERT INTO S VALUES (1, 2), (1, NULL), (2, 1), (4, 5), (4, 6)").unwrap();
    let q = "SELECT K, V FROM R WHERE V NOT IN (SELECT W FROM S WHERE S.K = R.K)";
    assert!(physical(&s, q).contains("Anti"), "{q}");
    let want: Vec<String> = ["-,[1]", "[3],-", "[3],[1]"].iter().map(|r| (*r).to_owned()).collect();
    let (on, off) = both(&db, &s, q);
    assert_eq!(on, Ok(want.clone()));
    assert_eq!(off, Ok(want));
}

/// Verifies: [DESIGN] (RULES.md D540) the TPC-H query 4 and 21 shapes are
/// planned as semi joins and antijoins: EXISTS of the orders' line items
/// a semi join hashed on the order key, and query 21's NOT EXISTS of
/// another supplier's late line an antijoin whose condition beyond the
/// key is evaluated only on the pairs the key keeps.
#[test]
fn tpch_shapes_explained() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION H CREATE TABLE ORDERS (O_ORDERKEY INTEGER, O_ORDERPRIORITY CHAR(8))
         CREATE TABLE LINEITEM (L_ORDERKEY INTEGER, L_SUPPKEY INTEGER, L_COMMITDATE INTEGER, L_RECEIPTDATE INTEGER)",
    )
    .unwrap();
    let s = db.session("H");
    for o in 0..40 {
        s.run(&format!("INSERT INTO ORDERS VALUES ({o}, 'P{}')", o % 5)).unwrap();
        for l in 0..(o % 4) {
            s.run(&format!("INSERT INTO LINEITEM VALUES ({o}, {l}, {}, {})", o + l, o + 2 * l)).unwrap();
        }
    }
    s.commit().unwrap();
    let q4 = "SELECT O_ORDERPRIORITY, COUNT(*) FROM ORDERS WHERE EXISTS
        (SELECT * FROM LINEITEM WHERE L_ORDERKEY = O_ORDERKEY AND L_COMMITDATE < L_RECEIPTDATE) GROUP BY O_ORDERPRIORITY";
    let shown = physical(&s, q4);
    assert!(shown.contains("SemiJoin") && shown.contains("hashed on"), "{shown}");
    let q21 = "SELECT L1.L_SUPPKEY FROM LINEITEM AS L1 WHERE L1.L_RECEIPTDATE > L1.L_COMMITDATE
        AND EXISTS (SELECT * FROM LINEITEM AS L2 WHERE L2.L_ORDERKEY = L1.L_ORDERKEY AND L2.L_SUPPKEY <> L1.L_SUPPKEY)
        AND NOT EXISTS (SELECT * FROM LINEITEM AS L3 WHERE L3.L_ORDERKEY = L1.L_ORDERKEY AND L3.L_SUPPKEY <> L1.L_SUPPKEY
            AND L3.L_RECEIPTDATE > L3.L_COMMITDATE)";
    let shown = physical(&s, q21);
    assert!(shown.contains("SemiJoin") && shown.contains("AntiJoin") && !shown.contains("Subquery"), "{shown}");
    for q in [q4, q21] {
        let (on, off) = both(&db, &s, q);
        assert_eq!(on, off, "{q}");
    }
}
