//! Row goals (RULES.md D552, `physical/goals.rs`): the first rows a FETCH
//! FIRST reads, passed through filters, joins that keep their left
//! operand's order and streamed groupings to an ordered read below, which
//! is then priced by the share of it read; and the rows each plan gives,
//! alike with the goal and without.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;

use crate::common::rows;

/// The rows of R.
const ROWS: i64 = 10_000;

/// R of the row store, keyed by K, X 1 in nine rows of ten and else one of
/// many rare values, and S keyed by K of every tenth key, of schema G,
/// their statistics gathered.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION G
           CREATE TABLE R (K INTEGER PRIMARY KEY, X INTEGER, V VARCHAR(20))
           CREATE TABLE S (K INTEGER PRIMARY KEY, W INTEGER)",
    )
    .unwrap();
    let g = db.session("G");
    for first in (0..ROWS).step_by(1000) {
        let values: Vec<String> = (first..first + 1000).map(|k| format!("({k}, {}, 'v {k}')", if k % 10 == 0 { k } else { 1 })).collect();
        g.run(&format!("INSERT INTO R VALUES {}", values.join(", "))).unwrap();
        let tenth: Vec<String> = (first..first + 1000).step_by(10).map(|k| format!("({k}, {})", k % 7)).collect();
        g.run(&format!("INSERT INTO S VALUES {}", tenth.join(", "))).unwrap();
        g.commit().unwrap();
    }
    db.analyze("G.R").unwrap();
    db.analyze("G.S").unwrap();
    (db, g)
}

/// The physical plan EXPLAIN shows of `sql`.
fn plan(g: &Session, sql: &str) -> String {
    let shown = g.explain(sql, &Params::new()).unwrap();
    shown.split("-- physical:").nth(1).unwrap_or(&shown).to_owned()
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR7] [DESIGN] RULES.md D552: of
/// FETCH FIRST over ORDER BY of the key and a filter, the key's index is
/// read in its order when the filter keeps most rows, so few are read
/// before the first are found, and the rows are sorted when it keeps few;
/// the rows are those the query asks either way.
///
/// Decision: the goal divided by the filter's share of rows. The
/// pair: a filter keeping all rows but one value's (the index, no sort)
/// against one keeping a rare value's (a sort).
#[test]
fn a_goal_passes_through_a_filter() {
    let (_db, g) = tables();
    let common = plan(&g, "SELECT K, V FROM R WHERE X <> 70 ORDER BY K FETCH FIRST 10 ROWS ONLY");
    assert!(common.contains("IndexScan") && !common.contains("Sort"), "{common}");
    let rare = plan(&g, "SELECT K, V FROM R WHERE X = 70 ORDER BY K FETCH FIRST 10 ROWS ONLY");
    assert!(rare.contains("Sort"), "{rare}");
    assert_eq!(rows(&g, "SELECT K FROM R WHERE X <> 0 ORDER BY K FETCH FIRST 3 ROWS ONLY").unwrap(), ["1", "2", "3"]);
    assert_eq!(rows(&g, "SELECT K FROM R WHERE X = 70 ORDER BY K FETCH FIRST 3 ROWS ONLY").unwrap(), ["70"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR7] [DESIGN] RULES.md D552: of
/// FETCH FIRST over ORDER BY of a join's left key, the goal passes to the
/// left operand divided by the rows each of its rows is estimated to give,
/// so it is read in the key's order and not sorted; and over a filter of
/// the join's rows, divided by its share too.
#[test]
fn a_goal_passes_to_a_joins_left_operand() {
    let (_db, g) = tables();
    let joined = "SELECT R.K, S.W FROM R, S WHERE R.K = S.K ORDER BY R.K FETCH FIRST 5 ROWS ONLY";
    let shown = plan(&g, joined);
    assert!(!shown.contains("Sort"), "{shown}");
    assert_eq!(rows(&g, joined).unwrap(), ["0,0", "10,3", "20,6", "30,2", "40,5"]);
    let filtered = "SELECT R.K, S.W FROM R, S WHERE R.K = S.K AND R.X + S.W > 1 ORDER BY R.K FETCH FIRST 3 ROWS ONLY";
    assert_eq!(rows(&g, filtered).unwrap(), ["10,3", "20,6", "30,2"]);
}
