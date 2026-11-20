//! A join's method chosen by its regret (RULES.md D386): an index
//! nested-loop join is kept where its probes stay cheap even if the left
//! operand gives PLANNER.ESTIMATE_MARGIN times its estimate, and a hash
//! join taken where they would not; the rows are the same either way
//! [X3.135-1986 §5.20 GR2b]. These are `[DESIGN]` requirements on the
//! plan; the standard governs only the rows.

use turborel::db::{Database, Session};
use turborel::params::Params;

use crate::common::{exec, sorted};

/// The rows of each table.
const ROWS: i32 = 6_000;

/// Schema J: L of `ROWS` rows, K from 0, G of ten values; R of as many,
/// K indexed but not unique; U of as many, K its primary key.
fn tables() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION J
           CREATE TABLE L (K INTEGER NOT NULL, G INTEGER NOT NULL)
           CREATE TABLE R (K INTEGER NOT NULL, V INTEGER)
           CREATE TABLE U (K INTEGER NOT NULL PRIMARY KEY, V INTEGER)",
    )
    .unwrap();
    let s = db.session("J");
    let values = |of: &dyn Fn(i32) -> String| (0..ROWS).map(of).collect::<Vec<_>>().join(", ");
    s.run(&format!("INSERT INTO L VALUES {}", values(&|k| format!("({k}, {})", k % 10)))).unwrap();
    for table in ["R", "U"] {
        s.run(&format!("INSERT INTO {table} VALUES {}", values(&|k| format!("({k}, {})", k % 7)))).unwrap();
    }
    s.commit().unwrap();
    db.create_index("R_K", "J.R", &["K"], "btree").unwrap();
    db.maintain().unwrap();
    db
}

/// Whether `sql` is planned as an index nested-loop join, PLANNER.
/// ESTIMATE_MARGIN being `margin` and PLANNER.JOIN_ENUMERATION `how`;
/// its rows.
fn planned(s: &Session, sql: &str, (margin, how): (&str, &str)) -> (bool, Vec<String>) {
    exec(s, &format!("SET PLANNER.ESTIMATE_MARGIN = {margin}")).unwrap();
    exec(s, &format!("SET PLANNER.JOIN_ENUMERATION = '{how}'")).unwrap();
    let plan = s.explain(sql, &Params::new()).unwrap();
    let probed = plan.contains("IndexNestedLoop");
    assert!(probed || plan.contains("HashJoin"), "{sql}: {plan}");
    (probed, sorted(s, sql))
}

/// Verifies: [DESIGN] RULES.md D386: as the left's estimate grows, a
/// margin of 1 keeps the index join as long as its probes are cheaper at
/// the estimate, and the default margin hands it to the hash join sooner,
/// at some estimate where the index join is cheaper but would be dearer
/// at eight times it; a margin only ever moves the choice to the hash
/// join; and every plan gives the same rows. MC/DC of the regret rule's
/// choice: the margin alone decides it at that estimate.
#[test]
fn the_margin_hands_a_risky_index_join_to_the_hash_join() {
    let db = tables();
    let s = db.session("J");
    for how in ["ADAPTIVE", "GREEDY"] {
        let mut decided = false;
        for n in [25, 50, 100, 200, 400, 800, 1_600, 3_200] {
            let sql = format!("SELECT L.K, R.V FROM L, R WHERE L.K = R.K AND L.K < {n}");
            let (trusting, rows) = planned(&s, &sql, ("1", how));
            let (wary, same) = planned(&s, &sql, ("8", how));
            assert_eq!(rows, same, "{how} {n}");
            assert_eq!(rows.len(), usize::try_from(n).unwrap(), "{how} {n}");
            assert!(trusting || !wary, "{how} {n}: the margin moved the choice to the index join");
            decided |= trusting && !wary;
        }
        assert!(decided, "{how}: no estimate where the margin chose the hash join");
    }
}

/// Verifies: [DESIGN] RULES.md D386: a probe of a unique key from a
/// filtered small table stays an index join under the default margin,
/// its probes cheap even at eight times its rows.
#[test]
fn a_small_left_probing_a_key_stays_an_index_join() {
    let db = tables();
    let s = db.session("J");
    for how in ["ADAPTIVE", "GREEDY"] {
        let sql = "SELECT L.K, U.V FROM L, U WHERE L.K = U.K AND L.K < 5";
        let (probed, rows) = planned(&s, sql, ("8", how));
        assert!(probed, "{how}");
        assert_eq!(rows, ["0,0", "1,1", "2,2", "3,3", "4,4"], "{how}");
    }
}
