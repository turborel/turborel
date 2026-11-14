//! NTILE, LEAD, LAG, FIRST_VALUE, LAST_VALUE and NTH_VALUE [ISO/IEC
//! 9075-2:2023 §6.10, §9.23 SR5, SR6, Features T614, T615, T616, T617,
//! T618]: tiles even and uneven, more than the rows, partitioned, of a
//! parameter; rows before and after, offset 0, defaults constant, of
//! columns and cast, RESPECT and IGNORE NULLS, of a grouped query; the
//! first, last and nth of implied frames, of ROWS, RANGE and GROUPS, with
//! exclusions, from the last, empty and short; a window function in the
//! argument of another; and partitions past the work area. The Syntax
//! Rules are `window_value_rules.rs`'s.

use turborel::db::Session;
use turborel::error::SqlState;
use turborel::params::Params;
use turborel::value::{Decimal, Value};

use crate::common::{Counted, both_ways, exec, fetched, sorted};
use crate::windows::{by_row, ordered, prepared, sales};

/// The exact number `n`.
fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The SQLSTATE of the failure of the query `sql`.
fn state_of(a: &Session, sql: &str) -> Option<SqlState> {
    fetched(a, sql).expect_err(sql).state
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 GR1a] NTILE cuts each
/// partition's rows, in its ordering, into the number of tiles given, the
/// first MOD(rows, tiles) one row longer, and gives a row its tile's
/// number; of more tiles than rows, each row is a tile of its own.
#[test]
fn ntile_cuts_partitions_into_tiles_6_10() {
    let (_db, a) = sales();
    assert_eq!(by_row(&a, "NTILE(3) OVER (ORDER BY I)"), ["1", "1", "1", "2", "2", "2", "3", "3"]);
    assert_eq!(by_row(&a, "NTILE(4) OVER (ORDER BY I)"), ["1", "1", "2", "2", "3", "3", "4", "4"]);
    assert_eq!(by_row(&a, "NTILE(10) OVER (ORDER BY I)"), ["1", "2", "3", "4", "5", "6", "7", "8"]);
    assert_eq!(by_row(&a, "NTILE(1) OVER (ORDER BY I DESC)"), ["1"; 8]);
    let sql = "SELECT R, D, AMOUNT, NTILE(2) OVER (PARTITION BY R ORDER BY D, AMOUNT) FROM S ORDER BY R, D, AMOUNT";
    assert_eq!(ordered(&a, sql), ["n,1,10,1", "n,2,5,1", "n,2,20,2", "n,4,NULL,2", "s,1,5,1", "s,3,7,2", "NULL,2,1,1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 GR1a i, ii, §9.23 SR5i
/// i] NTILE's number of tiles is a literal or a parameter, evaluated
/// once; null, each row's tile is null; not positive, SQLSTATE 22014,
/// raised as the first partition is read, so of no rows it is not.
#[test]
fn ntile_of_null_or_no_tiles_6_10() {
    let (_db, a) = sales();
    let sql = "SELECT I, NTILE(?) OVER (ORDER BY I) FROM F WHERE I < ? ORDER BY I";
    let tiles = |strs: &[&str]| Ok(strs.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
    assert_eq!(prepared(&a, sql, &[int(3), int(6)]), tiles(&["1", "1", "2", "2", "3"]));
    assert_eq!(prepared(&a, sql, &[Value::Null, int(3)]), tiles(&["NULL", "NULL"]));
    let refused = Err(Some(SqlState::INVALID_ARGUMENT_FOR_NTILE));
    assert_eq!(prepared(&a, sql, &[int(0), int(3)]), refused);
    assert_eq!(prepared(&a, sql, &[int(-2), int(3)]), refused);
    assert_eq!(prepared(&a, sql, &[int(0), int(0)]), Ok(Vec::new()), "of no rows, no tiles are counted");
    assert_eq!(state_of(&a, "SELECT NTILE(0) OVER (ORDER BY I) FROM F"), Some(SqlState::INVALID_ARGUMENT_FOR_NTILE));
    assert_eq!(state_of(&a, "SELECT NTILE(-1) OVER (ORDER BY I) FROM F"), Some(SqlState::INVALID_ARGUMENT_FOR_NTILE));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 GR1b, §9.23 SR5j] LAG
/// and LEAD take the extent of the row the offset before or after the
/// current one, 1 if none is written; of offset 0, the row's own; past
/// the partition, the default evaluated for the current row, null if
/// none is written; RESPECT NULLS as if none were written.
#[test]
fn lead_and_lag_take_rows_before_and_after_6_10() {
    let (_db, a) = sales();
    let lagged = ["NULL", "10", "20", "NULL", "40", "50", "60", "70"];
    assert_eq!(by_row(&a, "LAG(V) OVER (ORDER BY I)"), lagged);
    assert_eq!(by_row(&a, "LAG(V, 1) RESPECT NULLS OVER (ORDER BY I)"), lagged);
    assert_eq!(by_row(&a, "LEAD(V, 2, -1) OVER (ORDER BY I)"), ["NULL", "40", "50", "60", "70", "80", "-1", "-1"]);
    assert_eq!(by_row(&a, "LEAD(V, 0) OVER (ORDER BY I)"), ["10", "20", "NULL", "40", "50", "60", "70", "80"]);
    assert_eq!(by_row(&a, "LAG(V, 1, I * 100) OVER (ORDER BY I)"), ["100", "10", "20", "NULL", "40", "50", "60", "70"]);
    assert_eq!(by_row(&a, "LEAD(I, 9, I) OVER (ORDER BY I)"), ["1", "2", "3", "4", "5", "6", "7", "8"]);
    let sql = "SELECT R, D, AMOUNT, LEAD(AMOUNT) OVER (PARTITION BY R ORDER BY D, AMOUNT) FROM S ORDER BY R, D, AMOUNT";
    assert_eq!(ordered(&a, sql), ["n,1,10,5", "n,2,5,20", "n,2,20,NULL", "n,4,NULL,NULL", "s,1,5,7", "s,3,7,NULL", "NULL,2,1,NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 GR1b iv 1 B, v 1 B]
/// under IGNORE NULLS, LAG and LEAD count only the rows whose extent is
/// not null; offset 0 is still the row's own, null or not.
#[test]
fn lead_and_lag_ignore_nulls_6_10() {
    let (_db, a) = sales();
    assert_eq!(by_row(&a, "LAG(V) IGNORE NULLS OVER (ORDER BY I)"), ["NULL", "10", "20", "20", "40", "50", "60", "70"]);
    assert_eq!(by_row(&a, "LAG(V, 2) IGNORE NULLS OVER (ORDER BY I)"), ["NULL", "NULL", "10", "10", "20", "40", "50", "60"]);
    assert_eq!(by_row(&a, "LEAD(V) IGNORE NULLS OVER (ORDER BY I)"), ["20", "40", "40", "50", "60", "70", "80", "NULL"]);
    assert_eq!(by_row(&a, "LEAD(V, 0, 0) IGNORE NULLS OVER (ORDER BY I)"), ["10", "20", "NULL", "40", "50", "60", "70", "80"]);
    assert_eq!(by_row(&a, "LEAD(V, 3, 0) IGNORE NULLS OVER (ORDER BY I)"), ["50", "60", "60", "70", "80", "0", "0", "0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.23 SR5j iii, v, §6.13]
/// LAG and LEAD are of their extent's type, the default cast to it, so a
/// string too long for it is cut with the warning 01004; over a grouped
/// query, of its groups, a set function their extent.
#[test]
fn lead_and_lag_defaults_take_the_extents_type_9_23() {
    let (_db, a) = sales();
    let sql = "SELECT D, LAG(SUM(AMOUNT)) OVER (ORDER BY D) FROM S GROUP BY D ORDER BY D";
    assert_eq!(ordered(&a, sql), ["1,NULL", "2,15", "3,26", "4,7"]);
    let fits = "SELECT D, LAG(R, 1, 'zz') OVER (ORDER BY D, AMOUNT) FROM S WHERE R = 's' ORDER BY D";
    assert_eq!(ordered(&a, fits), ["1,zz", "3,s"]);
    let long = "SELECT D, LAG(R, 1, 'abcdefgh') OVER (ORDER BY D, AMOUNT) FROM S WHERE R = 's' ORDER BY D";
    assert_eq!(ordered(&a, long), ["1,abcde", "3,s"]);
    assert_eq!(a.warning().map(SqlState::as_str), Some("01004"), "a string cut by CAST");
    assert_eq!(by_row(&a, "LAG(V, 1, 2.0) OVER (ORDER BY I)")[0], "2");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 GR1c, §7.15 GR5]
/// FIRST_VALUE and LAST_VALUE take the first and the last argument of the
/// frame; of the implied frame, LAST_VALUE takes the last peer; under
/// IGNORE NULLS, the first and last not null.
#[test]
fn first_and_last_values_of_frames_6_10() {
    let (_db, a) = sales();
    assert_eq!(by_row(&a, "FIRST_VALUE(V) OVER (ORDER BY K)"), ["10"; 8]);
    assert_eq!(by_row(&a, "LAST_VALUE(V) OVER (ORDER BY K)"), ["20", "20", "NULL", "40", "60", "60", "80", "80"]);
    assert_eq!(by_row(&a, "LAST_VALUE(V) IGNORE NULLS OVER (ORDER BY K)"), ["20", "20", "20", "40", "60", "60", "80", "80"]);
    assert_eq!(by_row(&a, "FIRST_VALUE(V) OVER ()"), ["10"; 8]);
    let rows = "OVER (ORDER BY I ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING)";
    assert_eq!(by_row(&a, &format!("FIRST_VALUE(V) {rows}")), ["10", "10", "20", "NULL", "40", "50", "60", "70"]);
    assert_eq!(by_row(&a, &format!("FIRST_VALUE(V) IGNORE NULLS {rows}")), ["10", "10", "20", "40", "40", "50", "60", "70"]);
    assert_eq!(by_row(&a, &format!("LAST_VALUE(V) {rows}")), ["20", "NULL", "40", "50", "60", "70", "80", "80"]);
    assert_eq!(by_row(&a, &format!("LAST_VALUE(V) IGNORE NULLS {rows}")), ["20", "20", "40", "50", "60", "70", "80", "80"]);
    let range = "LAST_VALUE(V) IGNORE NULLS OVER (ORDER BY K RANGE BETWEEN 1 PRECEDING AND 1 FOLLOWING)";
    assert_eq!(by_row(&a, range), ["20", "20", "20", "60", "60", "60", "80", "80"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 GR1c, GR1d, §7.15 GR5]
/// a frame's exclusion leaves its rows out of what the value functions
/// count, the rows on either side of the current one counted on; an
/// empty frame gives null.
#[test]
fn value_functions_skip_excluded_rows_7_15() {
    let (_db, a) = sales();
    let current = "FIRST_VALUE(V) OVER (ORDER BY I ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING EXCLUDE CURRENT ROW)";
    assert_eq!(by_row(&a, current), ["20", "10", "20", "NULL", "40", "50", "60", "70"]);
    let nth = "NTH_VALUE(V, 2) OVER (ORDER BY I ROWS BETWEEN 2 PRECEDING AND 2 FOLLOWING EXCLUDE CURRENT ROW)";
    assert_eq!(by_row(&a, nth), ["NULL", "NULL", "20", "NULL", "40", "50", "60", "70"]);
    let group = "FIRST_VALUE(V) OVER (ORDER BY K GROUPS BETWEEN 1 PRECEDING AND CURRENT ROW EXCLUDE GROUP)";
    assert_eq!(by_row(&a, group), ["NULL", "NULL", "10", "NULL", "40", "40", "50", "50"]);
    let ties = "LAST_VALUE(V) OVER (ORDER BY K ROWS BETWEEN CURRENT ROW AND 1 FOLLOWING EXCLUDE TIES)";
    assert_eq!(by_row(&a, ties), ["10", "NULL", "40", "50", "50", "70", "70", "80"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 GR1d] NTH_VALUE takes
/// the nth argument of the frame from its first row, or from its last
/// under FROM LAST, nulls left out under IGNORE NULLS; null of a frame of
/// fewer rows or a null nth row; not positive, SQLSTATE 22016.
#[test]
fn nth_value_counts_from_either_end_6_10() {
    let (_db, a) = sales();
    assert_eq!(by_row(&a, "NTH_VALUE(V, 2) OVER (ORDER BY K)"), ["20"; 8]);
    assert_eq!(by_row(&a, "NTH_VALUE(V, 2) FROM FIRST OVER (ORDER BY K)"), ["20"; 8]);
    assert_eq!(by_row(&a, "NTH_VALUE(V, 3) OVER (ORDER BY K)"), ["NULL"; 8]);
    assert_eq!(by_row(&a, "NTH_VALUE(V, 3) IGNORE NULLS OVER (ORDER BY K)"), ["NULL", "NULL", "NULL", "40", "40", "40", "40", "40"]);
    assert_eq!(by_row(&a, "NTH_VALUE(V, 2) FROM LAST OVER (ORDER BY K)"), ["10", "10", "20", "NULL", "50", "50", "70", "70"]);
    assert_eq!(by_row(&a, "NTH_VALUE(V, 2) FROM LAST IGNORE NULLS OVER (ORDER BY K)"), ["10", "10", "10", "20", "50", "50", "70", "70"]);
    let whole = "OVER (ORDER BY I ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING)";
    assert_eq!(by_row(&a, &format!("NTH_VALUE(V, 8) {whole}")), ["80"; 8]);
    assert_eq!(by_row(&a, &format!("NTH_VALUE(V, 9) {whole}")), ["NULL"; 8]);
    let sql = "SELECT I, NTH_VALUE(V, ?) OVER (ORDER BY I) FROM F WHERE I < ? ORDER BY I";
    let values = |strs: &[&str]| Ok(strs.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
    assert_eq!(prepared(&a, sql, &[int(2), int(4)]), values(&["NULL", "20", "20"]));
    assert_eq!(prepared(&a, sql, &[Value::Null, int(3)]), values(&["NULL", "NULL"]));
    assert_eq!(prepared(&a, sql, &[int(0), int(3)]), Err(Some(SqlState::INVALID_ARGUMENT_FOR_NTH_VALUE)));
    assert_eq!(prepared(&a, sql, &[int(-1), int(0)]), Ok(Vec::new()), "of no rows, no nth row is counted");
    assert_eq!(state_of(&a, "SELECT NTH_VALUE(V, 0) OVER (ORDER BY I) FROM F"), Some(SqlState::INVALID_ARGUMENT_FOR_NTH_VALUE));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10, §7.15 GR3, GR4] a
/// window function in the argument of LAG, LEAD or a value function is
/// computed first, by its own window or one alike; a window alike adds no
/// sort, and a window of another ordering sorts again. EXPLAIN shows each.
#[test]
fn window_functions_nest_in_value_functions_6_10() {
    let (_db, a) = sales();
    let lag = "SELECT LAG(ROW_NUMBER() OVER W) OVER W FROM F WINDOW W AS (ORDER BY I)";
    assert_eq!(by_row(&a, "LAG(ROW_NUMBER() OVER (ORDER BY I)) OVER (ORDER BY I)"), ["NULL", "1", "2", "3", "4", "5", "6", "7"]);
    let running = "FIRST_VALUE(SUM(V) OVER (ORDER BY I ROWS UNBOUNDED PRECEDING)) OVER (ORDER BY I DESC)";
    assert_eq!(by_row(&a, running), ["330"; 8]);
    let nested = "LEAD(LAG(V) OVER (ORDER BY I), 2) OVER (ORDER BY I)";
    assert_eq!(by_row(&a, nested), ["20", "NULL", "40", "50", "60", "70", "NULL", "NULL"]);
    let physical = |sql: &str| {
        let plan = a.explain(sql, &Params::new()).unwrap();
        plan.split_once("-- physical:").map(|(_, p)| p.to_owned()).unwrap_or(plan)
    };
    let plan = physical(lag);
    assert_eq!(plan.matches("Sort").count(), 1, "{plan}");
    assert!(plan.contains("LAG(") && plan.contains("ROW_NUMBER()"), "{plan}");
    let twice = physical("SELECT FIRST_VALUE(SUM(V) OVER (ORDER BY I ROWS UNBOUNDED PRECEDING)) OVER (ORDER BY I DESC) FROM F");
    assert_eq!(twice.matches("Sort").count(), 2, "{twice}");
}

/// The windows of the spill test, each over N's three partitions: every
/// function, IGNORE NULLS and FROM LAST among them, over frames of each
/// unit and exclusion.
const SPILLED: [&str; 4] = [
    "SELECT I, NTILE(7) OVER (PARTITION BY P ORDER BY I), LEAD(V, 3, -5) OVER (PARTITION BY P ORDER BY I), \
     LAG(V, 2) IGNORE NULLS OVER (PARTITION BY P ORDER BY I) FROM N",
    "SELECT I, LEAD(V, 4) IGNORE NULLS OVER (PARTITION BY P ORDER BY K, I), LAG(V, 0) OVER (PARTITION BY P ORDER BY K, I) FROM N",
    "SELECT I, FIRST_VALUE(V) IGNORE NULLS OVER (PARTITION BY P ORDER BY K, I ROWS BETWEEN 5 PRECEDING AND 5 FOLLOWING), \
     LAST_VALUE(V) OVER (PARTITION BY P ORDER BY K RANGE BETWEEN 2 PRECEDING AND 1 FOLLOWING EXCLUDE TIES) FROM N",
    "SELECT I, NTH_VALUE(V, 3) FROM LAST IGNORE NULLS OVER (PARTITION BY P ORDER BY K GROUPS BETWEEN 1 PRECEDING AND 2 FOLLOWING \
     EXCLUDE GROUP), NTH_VALUE(V, 2) OVER (PARTITION BY P ORDER BY I ROWS BETWEEN 10 PRECEDING AND CURRENT ROW EXCLUDE CURRENT ROW) FROM N",
];

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 GR1] [DESIGN] RULES.md
/// D38: over partitions past the work area, NTILE, LEAD, LAG and the value
/// functions give what they give in memory, the rows and the arguments
/// not null spilled; row by row and by batches alike; and LAG is what its
/// definition by a subquery gives.
#[test]
fn value_functions_past_the_work_area_spill_6_10() {
    let (db, a) = sales();
    a.run("CREATE TABLE N (I INTEGER, P INTEGER, K INTEGER, V INTEGER)").unwrap();
    for chunk in 0_i64..12 {
        let values: Vec<String> = (0_i64..100)
            .map(|n| {
                let i = chunk.checked_mul(100).and_then(|c| c.checked_add(n)).unwrap();
                let v = if i % 7 == 0 || i % 11 == 0 { "NULL".to_owned() } else { (i % 31).to_string() };
                format!("({i}, {}, {}, {v})", i % 3, i % 40)
            })
            .collect();
        a.run(&format!("INSERT INTO N VALUES {}", values.join(", "))).unwrap();
    }
    let lag = "SELECT I, LAG(V, 2) OVER (PARTITION BY P ORDER BY I) FROM N";
    let defined = "SELECT I, (SELECT V FROM N AS M WHERE M.P = N.P AND M.I = N.I - 6) FROM N";
    assert_eq!(sorted(&a, lag), sorted(&a, defined), "LAG of 2 rows, I 3 apart in a partition");
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    exec(&a, "SET MEMORY.WORK_BYTES = 67108864").unwrap();
    let in_memory: Vec<Vec<String>> = SPILLED.iter().map(|sql| sorted(&a, sql)).collect();
    assert_eq!(counted.taken(), 0, "64 MiB of work area holds them");
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    for (sql, expected) in SPILLED.iter().zip(&in_memory) {
        let (batched, by_rows) = both_ways(&a, sql);
        assert_eq!(&batched, expected, "{sql}, spilled");
        assert_eq!(&by_rows, expected, "{sql}, spilled, row by row");
    }
    assert!(counted.taken() > 0, "they spilled");
    a.rollback().unwrap();
}
