//! \[DESIGN\] A grouping over another whose grouping columns hold its
//! own, as DISTINCT set functions make them (RULES.md D340, D1113), run
//! as one part: each partition's thread makes the second grouping's
//! groups of its own, and the part gives the groups the plan run alone
//! gives, in its order.

use turborel::db::{Database, Session};
use turborel::functions::{Accumulator, AggregateFunction};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{DataType, Decimal, MAX_EXACT_PRECISION, Spelled, Value};

use crate::common::{exec, function_calls, rows};

/// Tables C, of the column store in groups of 2048, and R, of the row
/// store, alike: K, a key; G, one of 50, null for every 41st; H, one of
/// 7; N, one of 10 apart from G, null for every 13th: few groups of G and
/// N beside the rows, so a grouping by both runs in parallel.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER, N INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER, N INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    let null_or = |null: bool, v: i64| if null { "NULL".to_owned() } else { v.to_string() };
    for table in ["C", "R"] {
        let values: Vec<String> = (0_i64..7000)
            .map(|k| format!("({k}, {}, {}, {})", null_or(k % 41 == 0, k * 7 % 50), k % 7, null_or(k % 13 == 0, k.div_euclid(7) % 10)))
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The rows of a query, or the code it failed with.
type Rows = Result<Vec<String>, turborel::error::Code>;

/// The settings a query runs alone under, a row at a time on one thread.
const ALONE: [&str; 2] = ["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'OFF'"];

/// The settings a query runs in parallel under, on 4 threads in morsels
/// of 256 rows.
const TOGETHER: [&str; 4] =
    ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"];

/// The rows of `sql` run alone, its rows run in parallel, and its plan
/// then; the rows run in parallel by batch and by rows alike.
fn both(s: &Session, sql: &str) -> (Rows, Rows, String) {
    for set in ALONE {
        exec(s, set).unwrap();
    }
    let alone = rows(s, sql);
    for set in TOGETHER {
        exec(s, set).unwrap();
    }
    let shown = s.physical_plan(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let together = rows(s, sql);
    // Its morsels read a row at a time, its groups made of rows.
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    assert_eq!(rows(s, sql), together, "{sql}, its morsels read by rows");
    exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    (alone, together, shown)
}

/// Whether `shown`, a plan, has a part run in parallel whose top is two
/// groupings, one over the other, under a filter or not.
fn regrouped_part(shown: &str) -> bool {
    let lines: Vec<&str> = shown.lines().map(str::trim_start).filter(|l| !l.starts_with("Filter")).collect();
    lines.windows(3).any(|w| {
        let starts = |at: usize, with: &str| w.get(at).is_some_and(|l| l.starts_with(with));
        starts(0, "Parallel") && starts(1, "Aggregate") && starts(2, "Aggregate")
    })
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7b i] (RULES.md D1113) a
/// grouping with DISTINCT set functions, made by two groupings, runs as
/// one part, as EXPLAIN shows, and gives the groups it gives run alone,
/// in their order: of one DISTINCT function, of one beside functions
/// computed in parts, of two of different arguments, of a grouping by two
/// columns, under a HAVING condition, and of rows a WHERE clause leaves.
#[test]
fn distinct_set_functions_are_regrouped_by_their_part() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for shape in [
            "SELECT G, COUNT(DISTINCT N) FROM {t} GROUP BY G",
            "SELECT G, COUNT(DISTINCT N), SUM(N), COUNT(*), MAX(K) FROM {t} GROUP BY G",
            "SELECT H, COUNT(DISTINCT N), SUM(DISTINCT G) FROM {t} GROUP BY H",
            "SELECT H, N, COUNT(DISTINCT K / 1000) FROM {t} GROUP BY H, N",
            "SELECT G, COUNT(DISTINCT N) FROM {t} GROUP BY G HAVING COUNT(DISTINCT N) > 8",
            "SELECT H, AVG(DISTINCT N) FROM {t} WHERE K < 5000 GROUP BY H",
        ] {
            let sql = shape.replace("{t}", table);
            let (alone, together, shown) = both(&a, &sql);
            assert!(regrouped_part(&shown), "{sql}\n{shown}");
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
        }
    }
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1] (RULES.md D1113, D1063) a join
/// over such a part, whose filters are of the second grouping's columns,
/// tells the first's pipeline none, and gives the rows it gives alone.
#[test]
fn a_join_over_a_regrouped_part_gives_its_rows() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        let sql = format!(
            "SELECT COUNT(*), SUM(X.D), SUM({table}.N) FROM {table} JOIN (SELECT G, COUNT(DISTINCT N) AS D FROM {table} GROUP BY G) AS X ON X.G = {table}.K"
        );
        let (alone, together, _) = both(&a, &sql);
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
}

/// Verifies: [DESIGN] (RULES.md D1113) a pair the part does not hold
/// whole, of a grouping by no column, whose one group would be one
/// partition's, or with groups of empty grouping sets, keeps the second
/// grouping above the part and gives the rows it gives alone.
#[test]
fn a_pair_of_no_grouping_column_or_of_grouping_sets_stays_two() {
    let (_db, a) = tables();
    for sql in ["SELECT COUNT(DISTINCT N), SUM(N) FROM C", "SELECT H, COUNT(DISTINCT N) FROM C GROUP BY ROLLUP (H)"] {
        let (alone, together, shown) = both(&a, sql);
        assert!(!regrouped_part(&shown), "{sql}\n{shown}");
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
}

/// Verifies: [EXTENSION] (RULES.md D1113, D378) EXPLAIN ANALYZE of such
/// a part counts the groups of the first grouping its partitions made,
/// as many as the grouping by both columns makes.
#[test]
fn explain_analyze_counts_the_first_grouping_of_a_part() {
    let (_db, a) = tables();
    for set in TOGETHER {
        exec(&a, set).unwrap();
    }
    let sql = "SELECT G, COUNT(DISTINCT N) FROM C GROUP BY G";
    let shown = a.physical_plan(sql, &Params::new()).unwrap();
    assert!(regrouped_part(&shown), "{shown}");
    let analyzed = a.analyzed(sql, &Params::new()).unwrap();
    let firsts: Vec<u64> =
        analyzed.operators.iter().filter(|o| o.label.starts_with("Aggregate group by [C.G, C.N]")).map(|o| o.rows).collect();
    let expected = rows(&a, "SELECT COUNT(*) FROM (SELECT G, N FROM C GROUP BY G, N) AS X").unwrap();
    assert_eq!(
        firsts.iter().map(u64::to_string).collect::<Vec<_>>(),
        expected,
        "{:?}",
        analyzed.operators.iter().map(|o| &o.label).collect::<Vec<_>>()
    );
}

/// Table T, of the column store in groups of 2048: K, a key; B, a
/// CHARACTER(4) of 3 values of different lengths, null for every 37th; V,
/// a VARCHAR of 40 values; F, an approximate number; N, one of 9, null
/// for every 11th.
fn texts() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER PRIMARY KEY, B CHARACTER(4), V VARCHAR(10), F DOUBLE PRECISION, N INTEGER)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0_i64..12_000)
        .map(|k| {
            let b = if k % 37 == 0 {
                "NULL".to_owned()
            } else {
                format!("'{}'", ["a", "bc", "def"].get(usize::try_from(k % 3).unwrap()).unwrap())
            };
            let n = if k % 11 == 0 { "NULL".to_owned() } else { (k % 9).to_string() };
            format!("({k}, {b}, 'v{}', {}E{}, {n})", k * 13 % 40, k % 17 + 1, k % 31 - 15)
        })
        .collect();
    a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7b i] (RULES.md D1123) the
/// second grouping of a pair, made in the executor's table of the first's
/// groups given as a batch, gives the groups the plan run alone gives, in
/// their order: grouped by a padded text with nulls, by two texts, of
/// many groups of the first in one group of the second, of exact sums
/// and means, of a text's greatest value, which no column of states
/// holds, and beside a null a set function eliminates.
#[test]
fn a_second_grouping_made_in_a_table_gives_the_plans_groups() {
    let (_db, a) = texts();
    for sql in [
        "SELECT B, COUNT(DISTINCT N) FROM T GROUP BY B",
        "SELECT B, V, COUNT(DISTINCT N), COUNT(*) FROM T GROUP BY B, V",
        "SELECT B, COUNT(DISTINCT K / 2), SUM(N) FROM T GROUP BY B",
        "SELECT V, SUM(DISTINCT K / 3), AVG(DISTINCT N) FROM T GROUP BY V",
        "SELECT B, MAX(DISTINCT V), COUNT(DISTINCT V), MIN(F) FROM T GROUP BY B",
        "SELECT N, COUNT(DISTINCT B), SUM(DISTINCT K / 5) FROM T GROUP BY N HAVING COUNT(DISTINCT B) > 2",
    ] {
        let (alone, together, shown) = both(&a, sql);
        assert!(regrouped_part(&shown), "{sql}\n{shown}");
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR4] (RULES.md D1123) a second
/// grouping whose set function is filtered, which a grouping by batch
/// does not read, is made of the first's groups a row at a time, and
/// gives the groups the plan run alone gives.
#[test]
fn a_second_grouping_no_batch_reads_is_made_of_rows() {
    let (_db, a) = texts();
    let sql = "SELECT B, COUNT(DISTINCT N) FILTER (WHERE N > 3), COUNT(DISTINCT N) FROM T GROUP BY B";
    let (alone, together, _) = both(&a, sql);
    assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
    assert_eq!(alone, together, "{sql}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR6] (RULES.md D1124, D1281) the
/// first rows of a sort over a pair of groupings run as one part are not
/// taken from the first grouping's tables, whose groups are not the
/// pair's. They are taken of the second's groups, and are the rows the
/// plan run alone gives, in its order.
#[test]
fn the_first_rows_of_a_sort_over_a_pair_are_the_second_groupings() {
    let (_db, a) = tables();
    for sql in [
        "SELECT G, COUNT(DISTINCT N) FROM C GROUP BY G ORDER BY 2 DESC FETCH FIRST 5 ROWS ONLY",
        "SELECT H, COUNT(DISTINCT N), COUNT(*) FROM C GROUP BY H ORDER BY 3, 1 FETCH FIRST 3 ROWS ONLY",
    ] {
        let (alone, together, shown) = both(&a, sql);
        assert!(regrouped_part(&shown), "{sql}\n{shown}");
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
}

/// TOTAL, a plug-in's sum of whole numbers, nulls left out, whose
/// accumulator writes its state: a part of it is kept in no column of
/// states.
struct Total;

/// TOTAL's sum so far.
struct Totalled(i128);

impl AggregateFunction for Total {
    fn name(&self) -> &'static str {
        "TOTAL"
    }
    fn result_type(&self, _arg: Option<DataType>) -> turborel::error::Result<DataType> {
        Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric })
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(Totalled(0))
    }
}

impl Accumulator for Totalled {
    fn update(&mut self, v: &Value) -> turborel::error::Result<()> {
        if let Value::Exact(d) = v {
            self.0 += d.units;
        }
        Ok(())
    }
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        out.extend_from_slice(&self.0.to_le_bytes());
        true
    }
    fn merge(&mut self, state: &[u8]) -> turborel::error::Result<()> {
        self.0 += <[u8; 16]>::try_from(state).map_or(0, i128::from_le_bytes);
        Ok(())
    }
    fn finish(&self) -> turborel::error::Result<Value> {
        Ok(Value::Exact(Decimal::new(self.0, 0)))
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7b i] (RULES.md D1280, D341)
/// set functions without DISTINCT beside a DISTINCT one, whose parts the
/// second grouping of the pair merges from the first's columns of states,
/// give the groups the plan run alone gives, in their order: counts,
/// exact sums and means, least and greatest integers, of groups with
/// nulls and of the null group, under a HAVING condition of a merged sum
/// too. A part no column holds beside them is merged as before and gives
/// the plan's result: a plug-in's set function that writes its state, a
/// text's greatest value, a sum of approximate numbers, which writes no
/// state, and the parts of two DISTINCT arguments, grouped again of rows.
#[test]
fn parts_merged_from_their_columns_give_the_plans_groups() {
    let (db, a) = tables();
    db.register_aggregate_function(Box::new(Total)).unwrap();
    function_calls(&a);
    for sql in [
        "SELECT G, SUM(N), COUNT(*), AVG(N), MIN(N), MAX(N), COUNT(N), COUNT(DISTINCT H) FROM C GROUP BY G",
        "SELECT H, COUNT(DISTINCT G), SUM(K), AVG(K + N), MAX(K), COUNT(*) FROM C GROUP BY H",
        "SELECT G, SUM(N), COUNT(DISTINCT H) FROM C GROUP BY G HAVING SUM(N) > 500",
        "SELECT G, TOTAL(N), SUM(N), COUNT(DISTINCT H), COUNT(*) FROM C GROUP BY G",
    ] {
        let (alone, together, shown) = both(&a, sql);
        assert!(regrouped_part(&shown), "{sql}\n{shown}");
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
    let (_texts, t) = texts();
    for sql in [
        "SELECT B, MAX(V), SUM(N), COUNT(*), COUNT(DISTINCT K / 2) FROM T GROUP BY B",
        "SELECT B, SUM(F), COUNT(N), COUNT(DISTINCT N) FROM T GROUP BY B",
        "SELECT B, SUM(N), COUNT(DISTINCT N), COUNT(DISTINCT V) FROM T GROUP BY B",
    ] {
        let (alone, together, _) = both(&t, sql);
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
}
