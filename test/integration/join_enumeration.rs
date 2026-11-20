//! Join enumeration (RULES.md D300-D309): the joins of a query planned in
//! the order, shape and methods found cheapest, outer joins among them,
//! give the rows the greedy order gives [X3.135-1986 §5.20 GR2b; ISO/IEC
//! 9075:1992 §7.5 GR5, GR6], and EXPLAIN tells the order and the
//! algorithm. These are `[DESIGN]` requirements on the plan; the standard
//! governs only the rows.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::value::Value;

use crate::common::{Rng, both_ways, exec, sorted};

/// Schema Q: A and B of 600 rows, keyed by indexes on K, with repeated
/// and null keys; C of 40 and D of 8, unindexed.
fn tables() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION Q
           CREATE TABLE A (K INTEGER, V INTEGER)
           CREATE TABLE B (K INTEGER, W INTEGER)
           CREATE TABLE C (K INTEGER, X INTEGER)
           CREATE TABLE D (K INTEGER, Y INTEGER)",
    )
    .unwrap();
    let s = db.session("Q");
    let values = |n: i32, of: &dyn Fn(i32) -> String| (0..n).map(of).collect::<Vec<_>>().join(", ");
    let key = |k: i32, m: i32| if k % 97 == 13 { "NULL".to_owned() } else { (k % m).to_string() };
    s.run(&format!("INSERT INTO A VALUES {}", values(600, &|k| format!("({}, {})", key(k, 300), k % 7)))).unwrap();
    s.run(&format!("INSERT INTO B VALUES {}", values(600, &|k| format!("({}, {})", key(k * 7, 400), k % 40)))).unwrap();
    s.run(&format!("INSERT INTO C VALUES {}", values(40, &|k| format!("({}, {})", key(k, 40), k % 5)))).unwrap();
    s.run(&format!("INSERT INTO D VALUES {}", values(8, &|k| format!("({}, {})", k, k % 3)))).unwrap();
    s.commit().unwrap();
    db.create_index("A_K", "Q.A", &["K"], "btree").unwrap();
    db.create_index("B_K", "Q.B", &["K"], "btree").unwrap();
    db.maintain().unwrap();
    db
}

/// Sets PLANNER.JOIN_ENUMERATION to `word`.
fn enumeration(s: &Session, word: &str) {
    exec(s, &format!("SET PLANNER.JOIN_ENUMERATION = '{word}'")).unwrap();
}

/// The rows of `sql` planned adaptively, by batch and row by row, and
/// greedily, which must agree.
fn agree(s: &Session, sql: &str) -> Vec<String> {
    enumeration(s, "ADAPTIVE");
    let (batched, rows) = both_ways(s, sql);
    assert_eq!(batched, rows, "{sql}: by batch and by row");
    enumeration(s, "GREEDY");
    assert_eq!(sorted(s, sql), rows, "{sql}: greedy and adaptive");
    enumeration(s, "ADAPTIVE");
    rows
}

/// Verifies: [DESIGN] RULES.md D300, D306: a chain of three joins is
/// planned by DPccp, EXPLAIN telling the order on the top join, and gives
/// the greedy order's rows; GREEDY plans it without enumeration.
#[test]
fn explain_tells_the_order_and_the_algorithm() {
    let db = tables();
    let s = db.session("Q");
    let sql = "SELECT A.V, B.W, C.X, D.Y FROM A, B, C, D WHERE A.K = B.K AND B.W = C.K AND C.X = D.K";
    let plan = s.explain(sql, &Params::new()).unwrap();
    assert!(plan.contains("(join order by DPccp: "), "{plan}");
    assert_ne!(agree(&s, sql), Vec::<String>::new());
    enumeration(&s, "GREEDY");
    assert!(!s.explain(sql, &Params::new()).unwrap().contains("join order by"));
}

/// Verifies: [X3.135-1986 §5.20 GR2b]; [ISO/IEC 9075:1992 §7.5 GR5, GR6];
/// [DESIGN] RULES.md D301: outer joins among inner ones, LEFT, RIGHT and
/// FULL, with conditions that reject nulls and that do not, give the
/// same rows reordered as written.
#[test]
fn outer_joins_keep_their_rows_reordered() {
    let db = tables();
    let s = db.session("Q");
    for sql in [
        "SELECT A.V, B.W, C.X FROM A JOIN B ON A.K = B.K LEFT JOIN C ON B.W = C.K",
        "SELECT A.V, B.W, C.X FROM (A LEFT JOIN B ON A.K = B.K) JOIN C ON A.V = C.K",
        "SELECT A.V, B.W, C.X FROM (A LEFT JOIN B ON A.K = B.K) JOIN C ON B.W = C.K",
        "SELECT A.V, B.W, C.X, D.Y FROM A LEFT JOIN (B JOIN C ON B.W = C.K) ON A.K = B.K LEFT JOIN D ON C.X = D.K",
        "SELECT A.V, C.X, D.Y FROM C RIGHT JOIN A ON A.V = C.K JOIN D ON A.V = D.K",
        "SELECT A.V, B.W, C.X FROM (A LEFT JOIN B ON A.K = B.K) LEFT JOIN C ON B.W = C.K OR C.K IS NULL",
        "SELECT C.X, D.Y, B.W FROM (C FULL JOIN D ON C.X = D.K) JOIN B ON B.W = C.K",
        "SELECT C.X, D.Y, A.V FROM (C FULL JOIN D ON C.X = D.K) LEFT JOIN A ON A.K = D.Y",
        "SELECT A.V, B.W, D.Y FROM A LEFT JOIN B ON A.K = B.K AND B.W < 5 JOIN D ON A.V = D.K WHERE A.V > 1",
        "SELECT A.V, B.W, C.X FROM (A LEFT JOIN B ON A.K = B.K) JOIN C ON A.V = C.K WHERE B.W IS NULL OR B.W < 3",
    ] {
        agree(&s, sql);
    }
}

/// Schema M: column tables L and R kept in the order of K
/// (COLUMNSTORE.ORDER), so a scan reads them in its order for no sort,
/// with runs of equal keys and nulls, and a small row table S.
fn ordered() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION M CREATE TABLE S (K INTEGER, Y INTEGER)").unwrap();
    let s = db.session("M");
    let values = |n: i32, of: &dyn Fn(i32) -> String| (0..n).map(of).collect::<Vec<_>>().join(", ");
    let key = |k: i32, m: i32| if k % 89 == 7 { "NULL".to_owned() } else { (k % m).to_string() };
    for (table, m) in [("L", 500), ("R", 700)] {
        s.run(&format!("CREATE TABLE {table} (K INTEGER, V INTEGER) USING columnstore WITH (COLUMNSTORE.ORDER = 'K')")).unwrap();
        s.run(&format!("INSERT INTO {table} VALUES {}", values(3000, &|k| format!("({}, {})", key(k * 3, m), k % 11)))).unwrap();
    }
    s.run(&format!("INSERT INTO S VALUES {}", values(6, &|k| format!("({k}, {})", k % 2)))).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D303: when scans give both operands in
/// their key's order and the order is asked of the join, a merge join
/// pairs runs of equal keys, a null key matching none, of a LEFT join
/// each left row matching none kept with nulls, and no sort follows; the
/// rows are the greedy order's.
#[test]
fn merge_joins_pair_runs_of_equal_keys() {
    let db = ordered();
    let s = db.session("M");
    let inner = "SELECT L.K, L.V, R.V, S.Y FROM L, R, S WHERE L.K = R.K AND L.V = S.K ORDER BY L.K";
    let left = "SELECT L.K, L.V, R.V, S.Y FROM (L LEFT JOIN R ON L.K = R.K) JOIN S ON L.V = S.K ORDER BY L.K";
    for sql in [inner, left] {
        let explained = s.explain(sql, &Params::new()).unwrap();
        let plan = explained.split("-- physical:").last().unwrap();
        assert!(plan.contains("MergeJoin L.K = R.K") && !plan.contains("Sort L.K"), "{sql}: {plan}");
        let rows = agree(&s, sql);
        assert!(rows.iter().any(|r| r.contains("NULL")) == (sql == left), "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md D300-D304: random chains of up to five
/// joins, inner and outer, give the same rows planned adaptively and
/// greedily (R-TEST-5, from a fixed seed).
#[test]
fn random_chains_agree_with_the_greedy_order() {
    let db = tables();
    let s = db.session("Q");
    let mut rng = Rng::new(0x5EED_0300);
    let tables = ["A", "B", "C", "D"];
    let columns = [("A", "K"), ("A", "V"), ("B", "K"), ("B", "W"), ("C", "K"), ("C", "X"), ("D", "K"), ("D", "Y")];
    for _ in 0..40 {
        let n = 3 + rng.below(2);
        let mut from = String::from("A");
        for (at, t) in tables.iter().enumerate().take(n).skip(1) {
            let (pt, pc) = columns[rng.below(at * 2)];
            let column = if rng.chance(50) { "K" } else { ["V", "W", "X", "Y"][at] };
            let join = rng.pick(&["JOIN", "JOIN", "LEFT JOIN", "RIGHT JOIN", "FULL JOIN"]);
            from = format!("({from} {join} {t} ON {pt}.{pc} = {t}.{column})");
        }
        let filter = format!("{}.{} > {}", tables[rng.below(n)], "K", rng.below(20));
        let picked: Vec<String> = tables.iter().take(n).map(|t| format!("{t}.K")).collect();
        let sql = format!("SELECT {} FROM {from} WHERE {filter} OR {}.K IS NULL", picked.join(", "), tables[rng.below(n)]);
        agree(&s, &sql);
    }
}

/// Verifies: [DESIGN] RULES.md D305: the settings of the enumeration are
/// read per session; past a budget of one subgraph, the chain is planned
/// still, by DPccp below 14 relations.
#[test]
fn the_budget_and_limits_are_settings() {
    let db = tables();
    let s = db.session("Q");
    exec(&s, "SET PLANNER.DP_BUDGET = 1").unwrap();
    exec(&s, "SET PLANNER.LINEARIZED_RELATIONS = 2").unwrap();
    let sql = "SELECT A.V FROM A, B, C WHERE A.K = B.K AND B.W = C.K";
    assert!(s.explain(sql, &Params::new()).unwrap().contains("DPccp"));
    agree(&s, sql);
    assert!(db.set_setting("PLANNER.JOIN_ENUMERATION", &Value::Char("BUSHY".into())).is_err());
}

/// Verifies: [DESIGN] RULES.md D300, D370: a chain one of whose relations
/// is a grouping, a derived table of GROUP BY, is planned by DPccp, as
/// its rows are estimated as its groups, and gives the greedy order's
/// rows; the hash join over it builds the grouping's few groups, not the
/// table's 600 rows.
#[test]
fn a_grouping_among_the_relations_is_enumerated() {
    let db = tables();
    let s = db.session("Q");
    let sql = "SELECT A.V, G.N, D.Y FROM A, (SELECT X, COUNT(*) AS N FROM C GROUP BY X) AS G, D \
               WHERE A.V = G.X AND G.X = D.K";
    let plan = s.explain(sql, &Params::new()).unwrap();
    assert!(plan.contains("(join order by DPccp: "), "{plan}");
    assert_ne!(agree(&s, sql), Vec::<String>::new());
    let built =
        s.explain("SELECT A.V, G.N FROM A, (SELECT X, COUNT(*) AS N FROM C GROUP BY X) AS G WHERE A.V = G.X", &Params::new()).unwrap();
    let physical = built.split("-- physical:").last().unwrap_or_default();
    let (join, grouping) = (physical.find("HashJoin").unwrap_or(usize::MAX), physical.find("Aggregate").unwrap_or(0));
    let table = physical.find("SeqScan Q.A").unwrap_or(0);
    assert!(join < table && table < grouping, "the grouping is the hash join's built side: {physical}");
}
