//! Eager aggregation (RULES.md D660-D663, `physical/eager.rs`): a
//! grouping over joins planned with a table grouped first gives the rows,
//! the warning and the error of the grouping planned as written, with the
//! rule on and off, by batches and row by row, in parallel and not; and
//! the groupings it must leave alone are left alone.

use turborel::db::{Database, Session};
use turborel::error::SqlState;
use turborel::optimizer::EAGER_AGGREGATION;
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{Rng, exec, rows};

/// The rows of F, the facts.
const FACTS: usize = 600;

/// What EXPLAIN shows of a grouping with a table grouped first.
const EAGER: &str = "(eager:";

/// Tables F and FC, facts of the row store and the column store alike,
/// E empty, DM a dimension of 2 rows of most of F's groups and nulls in
/// its key and its values, and T3, one of F's A, of schema A.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    let fact = "(K INTEGER NOT NULL, G INTEGER NOT NULL, A INTEGER NOT NULL, D DECIMAL(12, 2) NOT NULL, N INTEGER, X DOUBLE PRECISION, S VARCHAR(5))";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE F {fact}
           CREATE TABLE FC {fact} USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE E {fact}
           CREATE TABLE DM (G INTEGER, NAME VARCHAR(10), V INTEGER, W DECIMAL(9, 2))
           CREATE TABLE T3 (A INTEGER, C INTEGER)
           CREATE TABLE L (G INTEGER NOT NULL, N INTEGER)
           CREATE TABLE R (G INTEGER)"
    ))
    .unwrap();
    let a = db.session("A");
    let mut rng = Rng::new(660);
    let facts: Vec<String> = (0..FACTS)
        .map(|k| {
            let n = if rng.chance(20) { "NULL".to_owned() } else { rng.below(7).to_string() };
            let x = if rng.chance(10) { "NULL".to_owned() } else { format!("{}.{}", rng.below(9), rng.below(9)) };
            let s = rng.pick(&["a", "a ", "A", "b"]);
            format!("({k}, {}, {}, {}.{:02}, {n}, {x}, '{s}')", rng.below(12), rng.below(9), rng.below(500), rng.below(100))
        })
        .collect();
    for table in ["F", "FC"] {
        a.run(&format!("INSERT INTO {table} VALUES {}", facts.join(", "))).unwrap();
    }
    let dims: Vec<String> = (0..24)
        .map(|n| {
            let g = if n % 11 == 10 { "NULL".to_owned() } else { (n % 10).to_string() };
            let v = if n % 7 == 3 { "NULL".to_owned() } else { (n * 3 % 17).to_string() };
            let name = if n % 9 == 4 { "NULL".to_owned() } else { format!("'n{}'", n % 5) };
            format!("({g}, {name}, {v}, {}.{:02})", n % 13, n * 7 % 100)
        })
        .collect();
    a.run(&format!("INSERT INTO DM VALUES {}", dims.join(", "))).unwrap();
    let thirds: Vec<String> = (0..12).map(|n| format!("({}, {})", n % 9, n % 4)).collect();
    a.run(&format!("INSERT INTO T3 VALUES {}", thirds.join(", "))).unwrap();
    // L's nulls are all of rows no row of R joins.
    a.run("INSERT INTO L VALUES (1, 5), (1, 6), (1, 5), (1, 7), (2, NULL), (2, NULL), (2, 3), (2, NULL)").unwrap();
    a.run("INSERT INTO R VALUES (1), (1), (3)").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The rows of `sql`, sorted, as groups may come in another order, and
/// its warning; or its error.
fn outcome(s: &Session, sql: &str) -> String {
    match rows(s, sql) {
        Ok(mut found) => {
            found.sort();
            format!("{found:?} {:?}", s.warning())
        }
        Err(code) => format!("{code:?}"),
    }
}

/// Turns the rule on or off for `s`.
fn rule(s: &Session, on: bool) {
    exec(s, &format!("SET OPTIMIZER.{} = '{}'", EAGER_AGGREGATION.to_uppercase(), if on { "ON" } else { "OFF" })).unwrap();
}

/// Whether `sql`'s plan groups a table first.
fn eager(s: &Session, sql: &str) -> bool {
    s.explain(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e:?}")).contains(EAGER)
}

/// The outcome of `sql`, which must be alike with the rule on and off,
/// by batches and row by row, at one degree and four.
fn compared(s: &Session, sql: &str) -> String {
    let on = outcome(s, sql);
    let mut seen = Vec::new();
    for set in ["EXECUTOR.BATCHES = 'OFF'", "EXECUTOR.BATCHES = 'ON'", "EXECUTOR.DEGREE = 4", "EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
        exec(s, &format!("SET {set}")).unwrap();
        seen.push((set, outcome(s, sql)));
    }
    rule(s, false);
    let off = outcome(s, sql);
    rule(s, true);
    for set in ["EXECUTOR.DEGREE = 1", "EXECUTOR.PARALLEL_MIN_ROWS TO DEFAULT"] {
        exec(s, &format!("SET {set}")).unwrap();
    }
    assert_eq!(on, off, "{sql}");
    for (set, got) in seen {
        assert_eq!(on, got, "{sql}, after {set}");
    }
    on
}

/// The queries a table is grouped first of, of facts `t`.
fn grouped_first(t: &str) -> Vec<String> {
    [
        "SELECT DM.NAME, SUM({T}.A), COUNT(*) FROM {T} JOIN DM ON {T}.G = DM.G GROUP BY DM.NAME",
        "SELECT DM.NAME, SUM({T}.D), AVG({T}.D), MIN({T}.A), MAX({T}.D), COUNT({T}.A) FROM {T}, DM WHERE {T}.G = DM.G GROUP BY DM.NAME",
        "SELECT {T}.G, SUM(DM.V), COUNT(DM.V), AVG(DM.W), COUNT(*), MIN(DM.NAME), MAX(DM.V) FROM {T} JOIN DM ON {T}.G = DM.G GROUP BY {T}.G",
        "SELECT COUNT(*), SUM(DM.V), SUM({T}.A), COUNT(DM.NAME) FROM {T} JOIN DM ON {T}.G = DM.G",
        "SELECT DM.NAME, SUM({T}.N), COUNT({T}.N), MAX({T}.N) FROM {T} JOIN DM ON {T}.G = DM.G GROUP BY DM.NAME",
        "SELECT T3.C, DM.NAME, SUM({T}.D), COUNT(*) FROM {T} JOIN DM ON {T}.G = DM.G JOIN T3 ON {T}.A = T3.A WHERE {T}.K > 40 AND DM.V < 15 GROUP BY T3.C, DM.NAME",
        "SELECT DM.NAME, SUM({T}.A) FILTER (WHERE {T}.K > 100), COUNT(*) FILTER (WHERE DM.V > 2), SUM(DM.W) FILTER (WHERE DM.V > 4) FROM {T} JOIN DM ON {T}.G = DM.G GROUP BY DM.NAME",
        "SELECT DM.NAME, SUM({T}.A) FROM {T} JOIN DM ON {T}.G = DM.G GROUP BY DM.NAME HAVING SUM({T}.A) > 100 ORDER BY 2 DESC",
        "SELECT DM.NAME, SUM({T}.A), COUNT(*) FROM {T} JOIN DM ON {T}.G = DM.G AND {T}.A > DM.V GROUP BY DM.NAME",
        "SELECT DM.G, SUM({T}.A), COUNT(DM.V) FROM {T} JOIN DM ON {T}.G IS NOT DISTINCT FROM DM.G GROUP BY DM.G",
        "SELECT DM.NAME, {T}.G, SUM({T}.D) FROM {T} JOIN DM ON {T}.G = DM.G GROUP BY DM.NAME, {T}.G",
        "SELECT DM.NAME, SUM({T}.A * 2 + {T}.G), SUM({T}.A / ({T}.G - 3)) FROM {T} JOIN DM ON {T}.G = DM.G AND DM.G <> 3 GROUP BY DM.NAME",
        "SELECT DM.NAME, COUNT(DISTINCT {T}.A), SUM({T}.A) FROM {T} JOIN DM ON {T}.G = DM.G GROUP BY DM.NAME",
    ]
    .iter()
    .map(|q| q.replace("{T}", t))
    .collect()
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7, §7.13] [DESIGN] RULES.md
/// D660-D662: groupings over joins of COUNT, SUM, AVG, MAX and MIN pushed
/// below the join in parts, computed above of the values passed up, and
/// counted and summed above multiplied by each early group's rows; with
/// duplicates on both sides, nulls in the join, grouping and argument
/// columns, filter clauses, HAVING, ORDER BY, a residual condition, a
/// join by IS NOT DISTINCT FROM, an expression that would raise of rows
/// the join drops, and DISTINCT: each grouped first when a session takes
/// every such plan, and alike with the rule on and off, of the row store,
/// the column store and an empty table, by batches and row by row, in
/// parallel and not.
#[test]
fn groupings_grouped_first_are_the_groupings_10_9() {
    let (_db, a) = tables();
    exec(&a, "SET PLANNER.EAGER_GAIN = 1000000").unwrap();
    for table in ["F", "FC", "E"] {
        for sql in grouped_first(table) {
            // DISTINCT is computed by two groupings first (RULES.md D340).
            if table != "E" && !sql.contains("COUNT(DISTINCT") {
                assert!(eager(&a, &sql), "not grouped first: {sql}");
            }
            compared(&a, &sql);
        }
    }
}

/// Verifies: [ISO/IEC 9075:1992 §6.5 GR1b] [DESIGN] RULES.md D660: the
/// warning of a null eliminated is raised of a null the join keeps, of a
/// value summed above it, and never of a null of a row the join drops,
/// which the early grouping takes.
#[test]
fn nulls_eliminated_warn_as_the_join_does_6_5() {
    let (_db, a) = tables();
    exec(&a, "SET PLANNER.EAGER_GAIN = 1000000").unwrap();
    let kept = "SELECT F.G, SUM(DM.V) FROM F JOIN DM ON F.G = DM.G GROUP BY F.G";
    assert!(eager(&a, kept));
    compared(&a, kept);
    rows(&a, kept).unwrap();
    assert_eq!(a.warning(), Some(SqlState::NULL_VALUE_ELIMINATED), "{kept}");
    let dropped = "SELECT R.G, SUM(L.N), COUNT(L.N), MAX(L.N) FROM L JOIN R ON L.G = R.G GROUP BY R.G";
    assert!(eager(&a, dropped));
    compared(&a, dropped);
    rows(&a, dropped).unwrap();
    assert_eq!(a.warning(), None, "{dropped}");
}

/// The groupings no table is grouped first of: a sum of approximate
/// numbers, whose total depends on the order they come in; a grouping by
/// a key; a join of character strings, whose equal values may differ; an
/// outer join; and empty grouping sets.
const LEFT_ALONE: [&str; 6] = [
    "SELECT DM.NAME, SUM(F.X), COUNT(*) FROM F JOIN DM ON F.G = DM.G GROUP BY DM.NAME",
    "SELECT DM.NAME, AVG(F.X) FROM F JOIN DM ON F.G = DM.G GROUP BY DM.NAME",
    "SELECT F.K, SUM(F.A), COUNT(*) FROM F JOIN DM ON F.G = DM.G GROUP BY F.K",
    "SELECT DM.NAME, SUM(F.A) FROM F JOIN DM ON F.S = DM.NAME GROUP BY DM.NAME",
    "SELECT DM.NAME, SUM(F.A) FROM F LEFT JOIN DM ON F.G = DM.G GROUP BY DM.NAME",
    "SELECT DM.NAME, SUM(F.A) FROM F JOIN DM ON F.G = DM.G GROUP BY GROUPING SETS ((DM.NAME), ())",
];

/// Verifies: [DESIGN] RULES.md D660: a sum or mean of approximate
/// numbers, a grouping by a key, a join of character strings, an outer
/// join and empty grouping sets are planned as written, however much a
/// session would take a plan grouped first, their results alike.
#[test]
fn groupings_eager_aggregation_leaves_alone() {
    let (_db, a) = tables();
    exec(&a, "SET PLANNER.EAGER_GAIN = 1000000").unwrap();
    for sql in LEFT_ALONE {
        assert!(!eager(&a, sql), "grouped first: {sql}");
        compared(&a, sql);
    }
}

/// Verifies: [DESIGN] RULES.md D663: PLANNER.EAGER_GAIN 0 never groups
/// first, and at its default a grouping of few groups of many facts over
/// a join is planned grouped first by the costs; OPTIMIZER.EAGER_AGGREGATION
/// OFF plans it as written.
#[test]
fn the_gain_and_the_switch_decide_6_5() {
    let (_db, a) = tables();
    let sql = "SELECT DM.NAME, SUM(F.A), COUNT(*) FROM F JOIN DM ON F.G = DM.G GROUP BY DM.NAME";
    exec(&a, "SET PLANNER.EAGER_GAIN = 0").unwrap();
    assert!(!eager(&a, sql));
    exec(&a, "SET PLANNER.EAGER_GAIN TO DEFAULT").unwrap();
    assert!(eager(&a, sql), "{}", a.explain(sql, &Params::new()).unwrap());
    rule(&a, false);
    assert!(!eager(&a, sql));
}
