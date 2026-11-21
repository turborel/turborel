//! The planner's cost model (RULES.md D650 to D659): the order a filter
//! judges its conjuncts in, what operators past their work grant cost,
//! the degree a part will run at, and the rows a runtime filter leaves,
//! each seen in the plan EXPLAIN shows and each plan's rows the same.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::value::{Decimal, Value};

use crate::common::{both_ways, exec, fetched, sorted};

/// Rows of T.
const ROWS: usize = 2_000;

/// Schema Q: T, of `ROWS` rows, A its key, B one of 100, C one of 7.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION Q CREATE TABLE T (A INTEGER, B INTEGER, C INTEGER)").unwrap();
    let s = db.session("Q");
    let rows: Vec<String> = (0..ROWS).map(|k| format!("({k}, {}, {})", k % 100, k % 7)).collect();
    s.run(&format!("INSERT INTO T VALUES {}", rows.join(", "))).unwrap();
    s.commit().unwrap();
    (db, s)
}

/// The line of `plan`'s physical plan showing its filter.
fn filter_line(plan: &str) -> &str {
    let physical = plan.split("-- physical:").nth(1).unwrap_or_else(|| panic!("no physical plan in {plan}"));
    physical.lines().find(|l| l.contains("Filter")).unwrap_or_else(|| panic!("no filter in {plan}"))
}

/// Verifies: [DESIGN] (RULES.md D650) a filter judges first the conjunct
/// of least rank, its cost over the rows it drops: the equality, which
/// keeps fewest rows, before the order comparison written ahead of it;
/// and gives the same rows by batches and row by row.
#[test]
fn a_filter_judges_its_selective_conjunct_first() {
    let (_db, s) = tables();
    let sql = "SELECT A FROM T WHERE A > 5 AND B = 7";
    let plan = s.explain(sql, &Params::new()).unwrap();
    let line = filter_line(&plan);
    let (b, a) = (line.find("B = 7").unwrap(), line.find("A > 5").unwrap());
    assert!(b < a, "{plan}");
    let (batched, by_rows) = both_ways(&s, sql);
    assert_eq!(batched.len(), 20);
    assert_eq!(by_rows, batched);
}

/// Verifies: [DESIGN] (RULES.md D650, D15) a filter one of whose
/// conjuncts can raise an error keeps the written order, and raises the
/// error of every row the standard judges it of.
#[test]
fn a_filter_that_can_raise_keeps_the_written_order() {
    let (_db, s) = tables();
    let sql = "SELECT A FROM T WHERE A > 5 AND 10 / C > 1 AND B = 7";
    let plan = s.explain(sql, &Params::new()).unwrap();
    let line = filter_line(&plan);
    let (a, b) = (line.find("A > 5").unwrap(), line.find("B = 7").unwrap());
    assert!(a < b, "{plan}");
    assert!(fetched(&s, sql).is_err());
}

/// Rows of L and R.
const JOINED: i32 = 6_000;
/// Rows of W, whose rows outgrow the least work area sorted.
const WIDE: i32 = 16_000;

/// Schema K: L of `JOINED` rows of the column store, whose parts gain
/// their degree, K and F from 0; R of as many, K
/// indexed; W of `WIDE` rows, K indexed and scattered, P a text of 600
/// characters; the statistics gathered, so W's rows are known to be wide.
fn joined() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION K
           CREATE TABLE L (K INTEGER NOT NULL, F INTEGER NOT NULL) USING columnstore
           CREATE TABLE R (K INTEGER NOT NULL, V INTEGER)
           CREATE TABLE W (K INTEGER NOT NULL, P VARCHAR(2000))",
    )
    .unwrap();
    let s = db.session("K");
    let values = |of: &dyn Fn(i32) -> String| (0..JOINED).map(of).collect::<Vec<_>>().join(", ");
    s.run(&format!("INSERT INTO L VALUES {}", values(&|k| format!("({k}, {k})")))).unwrap();
    s.run(&format!("INSERT INTO R VALUES {}", values(&|k| format!("({k}, {})", k % 7)))).unwrap();
    let wide = "w".repeat(600);
    for first in (0..WIDE).step_by(1_000) {
        let rows: Vec<String> = (first..first + 1_000).map(|k| format!("({}, '{wide}')", (k * 7_919) % WIDE)).collect();
        s.run(&format!("INSERT INTO W VALUES {}", rows.join(", "))).unwrap();
    }
    s.commit().unwrap();
    db.create_index("R_K", "K.R", &["K"], "btree").unwrap();
    db.create_index("W_K", "K.W", &["K"], "btree").unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(8, 0))).unwrap();
    db.maintain().unwrap();
    db
}
/// Whether `sql` is planned as an index nested-loop join at
/// EXECUTOR.DEGREE `degree`, a part of more than 500 rows run in
/// parallel and the estimates trusted; its rows.
fn probed_at(s: &Session, sql: &str, degree: usize) -> (bool, Vec<String>) {
    for set in ["EXECUTOR.PARALLEL_MIN_ROWS = 0", "EXECUTOR.MORSEL_ROWS = 500", "PLANNER.ESTIMATE_MARGIN = 1"] {
        exec(s, &format!("SET {set}")).unwrap();
    }
    exec(s, &format!("SET EXECUTOR.DEGREE = {degree}")).unwrap();
    let plan = s.explain(sql, &Params::new()).unwrap();
    let probed = plan.contains("IndexNestedLoop");
    assert!(probed || plan.contains("HashJoin"), "{sql}: {plan}");
    (probed, sorted(s, sql))
}

/// Verifies: [DESIGN] (RULES.md D652, D291) a join's method is chosen
/// seeing the degree its part will run at: as the left's rows grow, the
/// hash join, whose read of the indexed table and hashing eight threads
/// share, is taken at EXECUTOR.DEGREE 8 where one thread still probes the
/// index; a degree never moves the choice to the index join; and every
/// plan gives the same rows. MC/DC of the degree: it alone decides the
/// choice at that estimate.
#[test]
fn a_parallel_part_hands_the_index_join_to_the_hash_join_sooner() {
    let db = joined();
    let s = db.session("K");
    let mut decided = false;
    for n in [25, 50, 100, 200, 400, 800, 1_600, 3_200] {
        let sql = format!("SELECT L.K, R.V FROM L, R WHERE L.K = R.K AND L.F < {n}");
        let (alone, rows) = probed_at(&s, &sql, 1);
        let (shared, same) = probed_at(&s, &sql, 8);
        assert_eq!(rows, same, "{n}");
        assert_eq!(rows.len(), usize::try_from(n).unwrap(), "{n}");
        assert!(alone || !shared, "{n}: the degree moved the choice to the index join");
        decided |= alone && !shared;
    }
    assert!(decided, "no estimate where the degree chose the hash join");
}

/// Whether `sql` is planned with a sort, MEMORY.WORK_BYTES set by
/// `work`; its rows.
fn sorted_at(s: &Session, sql: &str, work: &str) -> (bool, Vec<String>) {
    exec(s, &format!("SET MEMORY.WORK_BYTES {work}")).unwrap();
    let plan = s.explain(sql, &Params::new()).unwrap();
    let physical = plan.split("-- physical:").nth(1).unwrap_or_default().to_owned();
    (physical.contains("Sort"), crate::common::rows(s, sql).unwrap())
}

/// Verifies: [DESIGN] (RULES.md D651) a sort is priced past its grant: as
/// the rows of W read grow, a scan and a sort of them is taken over the
/// index giving their order, but later where the work area is the least
/// and W's wide rows would be spilled; a smaller work area never moves the
/// choice to the sort; and every plan gives the same rows. MC/DC of the
/// grant: it alone decides the choice at that estimate.
#[test]
fn a_sort_past_its_grant_gives_way_to_the_index_order() {
    let db = joined();
    let s = db.session("K");
    let mut decided = false;
    for n in [1_000, 2_000, 4_000, 8_000, 12_000, 16_000] {
        let sql = format!("SELECT K, P FROM W WHERE K < {n} ORDER BY K");
        let (held, rows) = sorted_at(&s, &sql, "TO DEFAULT");
        let (spilled, same) = sorted_at(&s, &sql, "= 4194304");
        assert_eq!(rows, same, "{n}");
        assert_eq!(rows.len(), usize::try_from(n).unwrap(), "{n}");
        assert!(held || !spilled, "{n}: the least grant moved the choice to the sort");
        decided |= held && !spilled;
    }
    assert!(decided, "no estimate where the grant chose the index");
}

/// Verifies: [DESIGN] (RULES.md D651, D550) a hashed grouping is priced
/// past its grant: G's 500 groups of 8 rows each are hashed and their
/// groups sorted within the default grant, but past a grant of 64 KiB
/// their rows would be partitioned and spilled, and the rows are sorted
/// and streamed instead; the rows are the same.
#[test]
fn a_grouping_past_its_grant_streams_over_a_sort() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION G CREATE TABLE S (G INTEGER, V INTEGER)").unwrap();
    let s = db.session("G");
    let rows: Vec<String> = (0..4_000).map(|k| format!("({}, {k})", (k * 7) % 500)).collect();
    s.run(&format!("INSERT INTO S VALUES {}", rows.join(", "))).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    let sql = "SELECT G, COUNT(*), SUM(V) FROM S GROUP BY G ORDER BY G";
    let physical = |s: &Session| s.explain(sql, &Params::new()).unwrap().split("-- physical:").nth(1).unwrap_or_default().to_owned();
    let held = physical(&s);
    assert!(held.contains("Aggregate") && !held.contains("Aggregate streamed"), "{held}");
    let rows = crate::common::rows(&s, sql).unwrap();
    exec(&s, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    let spilled = physical(&s);
    assert!(spilled.contains("Aggregate streamed") && spilled.contains("Sort"), "{spilled}");
    assert_eq!(crate::common::rows(&s, sql).unwrap(), rows);
    assert_eq!(rows.len(), 500);
}
