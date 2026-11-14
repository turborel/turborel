//! Window functions [ISO/IEC 9075-2:2023 §6.10, §7.15, §9.23, Features T611,
//! T612 in part, T620]: ROW_NUMBER, RANK, DENSE_RANK, PERCENT_RANK and
//! CUME_DIST, with peers, nulls placed first and last, and partitions;
//! set functions over the frames of no framing clause, and of ROWS, RANGE
//! of numbers, dates, timestamps and intervals, and GROUPS, each bound and
//! each exclusion; offsets given by parameters; named windows; grouped
//! queries; DISTINCT and ORDER BY over their values. The Syntax Rules,
//! views, partitions past the work area and plans are `window_rules.rs`'s.

use turborel::db::{Database, Session};
use turborel::error::SqlState;
use turborel::value::{Decimal, Value};

use crate::common::{rows, sorted};

/// Schema A: S, of sales in regions and days, a null region and a null
/// amount among them; F, of rows numbered I, of keys K with peers and
/// nulls, and values V, one null; T, of dates; H, of timestamps and
/// intervals.
pub fn sales() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE S (R VARCHAR(5), D INTEGER, AMOUNT INTEGER)
           CREATE TABLE F (I INTEGER, K INTEGER, V INTEGER)
           CREATE TABLE T (K DATE, V INTEGER)
           CREATE TABLE H (TS TIMESTAMP, LONG INTERVAL HOUR, V INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO S VALUES ('n', 1, 10), ('n', 2, 20), ('n', 2, 5), ('n', 4, NULL), ('s', 1, 5), ('s', 3, 7), (NULL, 2, 1)").unwrap();
    a.run("INSERT INTO F VALUES (1, 1, 10), (2, 1, 20), (3, 2, NULL), (4, 4, 40), (5, 5, 50), (6, 5, 60), (7, NULL, 70), (8, NULL, 80)")
        .unwrap();
    a.run(
        "INSERT INTO T VALUES (DATE '2024-01-31', 1), (DATE '2024-02-29', 2), (DATE '2024-03-30', 4), (DATE '2024-03-31', 8), (NULL, 16)",
    )
    .unwrap();
    a.run(
        "INSERT INTO H VALUES (TIMESTAMP '2024-01-01 00:00:00', INTERVAL '1' HOUR, 1), \
         (TIMESTAMP '2024-01-01 12:00:00', INTERVAL '2' HOUR, 2), \
         (TIMESTAMP '2024-01-02 06:00:00', INTERVAL '5' HOUR, 4), (TIMESTAMP '2024-01-03 00:00:00', INTERVAL '7' HOUR, 8)",
    )
    .unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The rows of `sql`, in its order.
pub fn ordered(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|_| panic!("{sql}: {:?}", crate::common::fetched(s, sql)))
}

/// The second column of each row of the prepared `sql` run with `values`,
/// in order; else the SQLSTATE of its failure.
pub fn prepared(a: &Session, sql: &str, values: &[Value]) -> Result<Vec<String>, Option<SqlState>> {
    let p = a.prepare(sql).unwrap();
    let mut cursor = a.open_prepared(&p, values).map_err(|e| e.state)?;
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().map_err(|e| e.state)? {
        out.push(row[1].to_string());
    }
    Ok(out)
}

/// The value of the window function `call` of each row of F, by I.
pub fn by_row(s: &Session, call: &str) -> Vec<String> {
    let found = ordered(s, &format!("SELECT I, {call} FROM F ORDER BY I"));
    found.into_iter().map(|r| r.split_once(',').map_or_else(String::new, |(_, v)| v.to_owned())).collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.23 SR5d, SR5e, SR5f, §7.15
/// GR3, GR4] ROW_NUMBER numbers each partition's rows from 1 in its
/// ordering; RANK is 1 plus the rows of the peer groups before a row's,
/// so peers share it and the next is past them; DENSE_RANK 1 plus the
/// peer groups before; each partition apart, the rows of a null region
/// one partition.
#[test]
fn ranks_count_rows_and_peer_groups_9_23() {
    let (_db, a) = sales();
    let sql = "SELECT R, D, AMOUNT, ROW_NUMBER() OVER (PARTITION BY R ORDER BY D, AMOUNT), RANK() OVER (PARTITION BY R ORDER BY D), \
               DENSE_RANK() OVER (PARTITION BY R ORDER BY D) FROM S ORDER BY R, D, AMOUNT";
    let want = ["n,1,10,1,1,1", "n,2,5,2,2,2", "n,2,20,3,2,2", "n,4,NULL,4,4,3", "s,1,5,1,1,1", "s,3,7,2,2,2", "NULL,2,1,1,1,1"];
    assert_eq!(ordered(&a, sql), want);
    assert_eq!(by_row(&a, "RANK() OVER (ORDER BY K)"), ["1", "1", "3", "4", "5", "5", "7", "7"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.10 GR1e, §7.15 GR4] in a
/// window ordering nulls are peers of each other, last ascending and first
/// descending as implied (ID133), else where NULLS FIRST or NULLS LAST
/// puts them, whatever the direction.
#[test]
fn nulls_are_placed_in_a_window_ordering_10_10() {
    let (_db, a) = sales();
    assert_eq!(by_row(&a, "DENSE_RANK() OVER (ORDER BY K)"), ["1", "1", "2", "3", "4", "4", "5", "5"]);
    assert_eq!(by_row(&a, "DENSE_RANK() OVER (ORDER BY K DESC)"), ["5", "5", "4", "3", "2", "2", "1", "1"]);
    assert_eq!(by_row(&a, "DENSE_RANK() OVER (ORDER BY K DESC NULLS LAST)"), ["4", "4", "3", "2", "1", "1", "5", "5"]);
    assert_eq!(by_row(&a, "RANK() OVER (ORDER BY K NULLS FIRST)"), ["3", "3", "5", "6", "7", "7", "1", "1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.23 SR5g, SR5h] PERCENT_RANK
/// is (RANK - 1) / (rows - 1), 0 of a partition of one row; CUME_DIST the
/// rows up to a row's last peer over the rows; each DOUBLE PRECISION
/// (IV082, IV083).
#[test]
fn relative_ranks_are_fractions_of_the_partition_9_23() {
    let (_db, a) = sales();
    let first = "SELECT I, PERCENT_RANK() OVER (ORDER BY K), CUME_DIST() OVER (ORDER BY K) FROM F WHERE I <= 5 ORDER BY I";
    assert_eq!(ordered(&a, first), ["1,0E0,4E-1", "2,0E0,4E-1", "3,5E-1,6E-1", "4,7.5E-1,8E-1", "5,1E0,1E0"]);
    let alone =
        "SELECT R, PERCENT_RANK() OVER (PARTITION BY R ORDER BY D), CUME_DIST() OVER (PARTITION BY R ORDER BY D) FROM S WHERE R IS NULL";
    assert_eq!(ordered(&a, alone), ["NULL,0E0,1E0"]);
    a.run(
        "CREATE VIEW FRACTIONS (P, C, N, M) AS SELECT PERCENT_RANK() OVER (ORDER BY K), CUME_DIST() OVER (ORDER BY K), \
         RANK() OVER (ORDER BY K), COUNT(*) OVER () FROM F",
    )
    .unwrap();
    let columns =
        "SELECT ORDINAL_POSITION, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'FRACTIONS' ORDER BY ORDINAL_POSITION";
    let types: Vec<String> =
        ordered(&a, columns).into_iter().map(|r| r.split_once(',').map(|(_, t)| t.to_owned()).unwrap_or_default()).collect();
    assert_eq!(types[..2], ["DOUBLE PRECISION", "DOUBLE PRECISION"]);
    assert_eq!(types[2], types[3], "RANK is of COUNT's type (IV166)");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR4, §9.23 SR5f; T612]
/// without a window ordering all of a partition's rows are peers, so
/// ROW_NUMBER numbers them 1 to its rows in an order of no consequence
/// (US034).
#[test]
fn row_number_without_an_ordering_numbers_each_row_9_23() {
    let (_db, a) = sales();
    let numbered = sorted(&a, "SELECT ROW_NUMBER() OVER () FROM S");
    assert_eq!(numbered, ["1", "2", "3", "4", "5", "6", "7"]);
    let parted = sorted(&a, "SELECT R, ROW_NUMBER() OVER (PARTITION BY R) FROM S");
    assert_eq!(parted, ["NULL,1", "n,1", "n,2", "n,3", "n,4", "s,1", "s,2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5a, §10.9 GR] a set
/// function over a window of no framing clause is over its partition, or,
/// of an ordering, the rows up to a row's last peer; a null left out warns
/// 01003; COUNT(DISTINCT X) over a partition counts each value once.
#[test]
fn set_functions_over_implied_frames_7_15() {
    let (_db, a) = sales();
    let sql = "SELECT R, D, AMOUNT, SUM(AMOUNT) OVER (PARTITION BY R ORDER BY D), COUNT(*) OVER (PARTITION BY R), \
               SUM(AMOUNT) OVER () FROM S ORDER BY R, D, AMOUNT";
    let want =
        ["n,1,10,10,4,48", "n,2,5,35,4,48", "n,2,20,35,4,48", "n,4,NULL,35,4,48", "s,1,5,5,2,48", "s,3,7,12,2,48", "NULL,2,1,1,1,48"];
    assert_eq!(ordered(&a, sql), want);
    assert_eq!(a.warning().map(SqlState::as_str), Some("01003"), "a null left out of n's frames");
    let each = "SELECT R, COUNT(DISTINCT D) OVER (PARTITION BY R), MIN(D) OVER (PARTITION BY R), MAX(AMOUNT) OVER (PARTITION BY R) FROM S";
    assert_eq!(sorted(&a, each), ["NULL,1,2,1", "n,3,1,20", "n,3,1,20", "n,3,1,20", "n,3,1,20", "s,2,1,7", "s,2,1,7"]);
    let more = "SELECT R, AVG(AMOUNT) OVER (PARTITION BY R), EVERY(AMOUNT > 4) OVER (PARTITION BY R), \
                CARDINALITY(ARRAY_AGG(D) OVER (PARTITION BY R)), ANY_VALUE(R) OVER (PARTITION BY R) FROM S WHERE R = 's'";
    assert_eq!(sorted(&a, more), ["s,6.00,TRUE,2,s", "s,6.00,TRUE,2,s"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5b i 2, SR11] a
/// frame of ROWS counts rows before and after the current row: both
/// bounds before it, both after it, to either end, and a start alone,
/// which ends at the current row; an empty frame gives what an empty
/// group gives, null of SUM and 0 of COUNT.
#[test]
fn rows_frames_count_rows_7_15() {
    let (_db, a) = sales();
    let before = "SUM(V) OVER (ORDER BY I ROWS BETWEEN 2 PRECEDING AND 1 PRECEDING)";
    assert_eq!(by_row(&a, before), ["NULL", "10", "30", "20", "40", "90", "110", "130"]);
    let after = "SUM(V) OVER (ORDER BY I ROWS BETWEEN 1 FOLLOWING AND 3 FOLLOWING)";
    assert_eq!(by_row(&a, after), ["60", "90", "150", "180", "210", "150", "80", "NULL"]);
    assert_eq!(by_row(&a, "COUNT(V) OVER (ORDER BY I ROWS 1 PRECEDING)"), ["1", "2", "1", "1", "2", "2", "2", "2"]);
    let rest = "MIN(V) OVER (ORDER BY I ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING)";
    assert_eq!(by_row(&a, rest), ["10", "20", "40", "40", "50", "60", "70", "80"]);
    let none = "COUNT(*) OVER (ORDER BY I ROWS BETWEEN 8 FOLLOWING AND 9 FOLLOWING)";
    assert_eq!(by_row(&a, none), ["0", "0", "0", "0", "0", "0", "0", "0"]);
    let whole = "COUNT(*) OVER (ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING)";
    assert_eq!(by_row(&a, whole), ["8", "8", "8", "8", "8", "8", "8", "8"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5b i 1 B, C] a frame
/// of RANGE takes the rows whose key is within the offset of the current
/// row's, before and after it in the ordering: ascending, the key minus
/// the offset to the key plus it; descending, the other way. A row whose
/// key is null has the rows whose key is null as its frame; a row whose
/// key is not, the rows whose key is not. Each partition's frames are of
/// its own rows.
#[test]
fn range_frames_compare_keys_7_15() {
    let (_db, a) = sales();
    let around = "SUM(V) OVER (ORDER BY K RANGE BETWEEN 1 PRECEDING AND 1 FOLLOWING)";
    assert_eq!(by_row(&a, around), ["30", "30", "30", "150", "150", "150", "150", "150"]);
    let down = "SUM(V) OVER (ORDER BY K DESC NULLS FIRST RANGE BETWEEN 1 PRECEDING AND CURRENT ROW)";
    assert_eq!(by_row(&a, down), ["30", "30", "NULL", "150", "110", "110", "150", "150"]);
    let peers = "COUNT(*) OVER (ORDER BY K RANGE BETWEEN CURRENT ROW AND CURRENT ROW)";
    assert_eq!(by_row(&a, peers), ["2", "2", "1", "1", "2", "2", "2", "2"]);
    let ahead = "COUNT(*) OVER (ORDER BY K NULLS FIRST RANGE BETWEEN 2 FOLLOWING AND UNBOUNDED FOLLOWING)";
    assert_eq!(by_row(&a, ahead), ["3", "3", "3", "0", "0", "0", "8", "8"]);
    // Each partition's bounds are found from its own first row.
    let parted = "SELECT R, D, SUM(AMOUNT) OVER (PARTITION BY R ORDER BY D RANGE BETWEEN 1 PRECEDING AND CURRENT ROW), \
                  COUNT(*) OVER (PARTITION BY R ORDER BY D GROUPS BETWEEN CURRENT ROW AND 1 FOLLOWING) FROM S";
    assert_eq!(sorted(&a, parted), ["NULL,2,1,1", "n,1,10,3", "n,2,35,3", "n,2,35,3", "n,4,NULL,1", "s,1,5,2", "s,3,7,1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5b i 1 A, SR15a iii]
/// a frame of RANGE over dates moves each by a year-month interval, a day
/// its month has not made the month's last, so 31 January and a month is
/// 29 February, where the same arithmetic in an expression is SQLSTATE
/// 22008; over timestamps by a day-time interval; over intervals by an
/// interval.
#[test]
fn range_frames_over_datetimes_and_intervals_7_15() {
    let (_db, a) = sales();
    let by_date = |call: &str| {
        ordered(&a, &format!("SELECT {call}, K FROM T ORDER BY K NULLS LAST"))
            .into_iter()
            .map(|r| r.split(',').next().unwrap_or_default().to_owned())
            .collect::<Vec<_>>()
    };
    let back = "SUM(V) OVER (ORDER BY K RANGE BETWEEN INTERVAL '1' MONTH PRECEDING AND CURRENT ROW)";
    assert_eq!(by_date(back), ["1", "3", "6", "14", "16"]);
    let on = "SUM(V) OVER (ORDER BY K RANGE BETWEEN CURRENT ROW AND INTERVAL '1' MONTH FOLLOWING)";
    assert_eq!(by_date(on), ["3", "2", "12", "8", "16"]);
    let raised = rows(&a, "SELECT K + INTERVAL '1' MONTH FROM T WHERE K = DATE '2024-01-31'");
    assert!(raised.is_err(), "an expression's date past its month's last day is 22008");
    let by_time = |call: &str| {
        ordered(&a, &format!("SELECT {call}, TS FROM H ORDER BY TS"))
            .into_iter()
            .map(|r| r.split(',').next().unwrap_or_default().to_owned())
            .collect::<Vec<_>>()
    };
    let day = "SUM(V) OVER (ORDER BY TS RANGE BETWEEN INTERVAL '1' DAY PRECEDING AND CURRENT ROW)";
    assert_eq!(by_time(day), ["1", "3", "6", "12"]);
    let hours = "COUNT(*) OVER (ORDER BY LONG RANGE BETWEEN CURRENT ROW AND INTERVAL '2' HOUR FOLLOWING)";
    assert_eq!(by_time(hours), ["2", "1", "2", "1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5b i 3; T620] a frame
/// of GROUPS counts peer groups before and after the current row's, a
/// row's peers all in or all out.
#[test]
fn groups_frames_count_peer_groups_7_15() {
    let (_db, a) = sales();
    let around = "COUNT(*) OVER (ORDER BY K GROUPS BETWEEN 1 PRECEDING AND 1 FOLLOWING)";
    assert_eq!(by_row(&a, around), ["3", "3", "4", "4", "5", "5", "4", "4"]);
    let before = "SUM(V) OVER (ORDER BY K GROUPS BETWEEN 2 PRECEDING AND 1 PRECEDING)";
    assert_eq!(by_row(&a, before), ["NULL", "NULL", "30", "30", "40", "40", "150", "150"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5b ii] EXCLUDE
/// CURRENT ROW leaves the current row out of its frame, EXCLUDE GROUP its
/// peers and itself, EXCLUDE TIES its peers but itself, EXCLUDE NO OTHERS
/// nothing.
#[test]
fn exclusions_leave_rows_out_7_15() {
    let (_db, a) = sales();
    let counted =
        |exclusion: &str| by_row(&a, &format!("COUNT(*) OVER (ORDER BY K GROUPS BETWEEN 1 PRECEDING AND 1 FOLLOWING {exclusion})"));
    assert_eq!(counted("EXCLUDE NO OTHERS"), ["3", "3", "4", "4", "5", "5", "4", "4"]);
    assert_eq!(counted("EXCLUDE CURRENT ROW"), ["2", "2", "3", "3", "4", "4", "3", "3"]);
    assert_eq!(counted("EXCLUDE GROUP"), ["1", "1", "3", "3", "3", "3", "2", "2"]);
    assert_eq!(counted("EXCLUDE TIES"), ["2", "2", "4", "4", "4", "4", "3", "3"]);
    let summed = "SUM(V) OVER (ORDER BY I ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING EXCLUDE CURRENT ROW)";
    assert_eq!(by_row(&a, summed), ["20", "10", "60", "50", "100", "120", "140", "70"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5b i 1 B I 1, §6.10]
/// an offset given by a parameter is its value; negative or null it is
/// SQLSTATE 22013, raised as a row's frame is found, so of no rows it is
/// not raised.
#[test]
fn offsets_negative_or_null_are_22013_7_15() {
    let (_db, a) = sales();
    let p = a.prepare("SELECT I, SUM(V) OVER (ORDER BY I ROWS BETWEEN ? PRECEDING AND CURRENT ROW) FROM F WHERE I < ? ORDER BY I").unwrap();
    let int = |n: i128| Value::Exact(Decimal::new(n, 0));
    let read = |values: &[Value]| -> Result<Vec<String>, Option<SqlState>> {
        let mut cursor = a.open_prepared(&p, values).map_err(|e| e.state)?;
        let mut out = Vec::new();
        while let Some(row) = cursor.fetch().map_err(|e| e.state)? {
            out.push(row[1].to_string());
        }
        Ok(out)
    };
    assert_eq!(read(&[int(1), int(4)]), Ok(vec!["10".to_owned(), "30".to_owned(), "20".to_owned()]));
    let refused = Err(Some(SqlState::INVALID_PRECEDING_OR_FOLLOWING_SIZE));
    assert_eq!(read(&[int(-1), int(4)]), refused);
    assert_eq!(read(&[Value::Null, int(4)]), refused);
    assert_eq!(read(&[int(-1), int(0)]), Ok(Vec::new()), "of no rows, no frame is found");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 SR12, SR16, §6.10] a
/// window named in the WINDOW clause is the window of each function over
/// it, or the existing window of a later definition or a function's own
/// specification, which adds an ordering it lacks, or a frame; windows of
/// one partitioning and ordering give their peers one order (US035).
#[test]
fn named_windows_are_reused_7_15() {
    let (_db, a) = sales();
    let sql = "SELECT R, D, AMOUNT, COUNT(*) OVER P, SUM(AMOUNT) OVER O, SUM(AMOUNT) OVER (O ROWS UNBOUNDED PRECEDING), \
               RANK() OVER (P ORDER BY D) FROM S WINDOW P AS (PARTITION BY R), O AS (P ORDER BY D) ORDER BY R, D, AMOUNT";
    let got = ordered(&a, sql);
    assert_eq!(got.len(), 7);
    let partition_n: Vec<&String> = got.iter().filter(|r| r.starts_with("n,")).collect();
    let fields = |r: &str| r.split(',').map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(fields(partition_n[0]), ["n", "1", "10", "4", "10", "10", "1"]);
    assert_eq!(fields(partition_n[3]), ["n", "4", "NULL", "4", "35", "35", "4"]);
    let peers = "SELECT ROW_NUMBER() OVER (ORDER BY K), SUM(V) OVER (ORDER BY K ROWS UNBOUNDED PRECEDING), V FROM F ORDER BY 1";
    let running: Vec<String> = ordered(&a, peers);
    let mut total = 0_i64;
    for row in &running {
        let f = fields(row);
        if f[2] != "NULL" {
            total = f[2].parse::<i64>().unwrap().checked_add(total).unwrap();
        }
        assert_eq!(f[1], total.to_string(), "the numbering and the running total see one order of peers: {running:?}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.20, §7.15 GR1b, §7.13] the
/// windows of a grouped query are over its groups, after HAVING: a set
/// function of the groups is a window's argument or key, as SUM(SUM(X))
/// and RANK() OVER (ORDER BY SUM(X)); over the sets of a ROLLUP, and the
/// groups GROUP BY DISTINCT leaves.
#[test]
fn windows_of_grouped_queries_are_over_groups_9_20() {
    let (_db, a) = sales();
    let totals = "SELECT R, SUM(AMOUNT), SUM(SUM(AMOUNT)) OVER (), RANK() OVER (ORDER BY SUM(AMOUNT) DESC) FROM S GROUP BY R";
    assert_eq!(sorted(&a, totals), ["NULL,1,48,3", "n,35,48,1", "s,12,48,2"]);
    let had = "SELECT R, SUM(SUM(AMOUNT)) OVER (), COUNT(*) OVER () FROM S GROUP BY R HAVING SUM(AMOUNT) > 5";
    assert_eq!(sorted(&a, had), ["n,47,2", "s,47,2"]);
    let rolled = "SELECT R, GROUPING(R), RANK() OVER (ORDER BY GROUPING(R), SUM(AMOUNT) DESC) FROM S GROUP BY ROLLUP (R)";
    assert_eq!(sorted(&a, rolled), ["NULL,0,3", "NULL,1,4", "n,0,1", "s,0,2"]);
    assert_eq!(sorted(&a, "SELECT COUNT(*) OVER () FROM S GROUP BY ROLLUP (R), R"), ["6", "6", "6", "6", "6", "6"]);
    assert_eq!(sorted(&a, "SELECT COUNT(*) OVER () FROM S GROUP BY DISTINCT ROLLUP (R), R"), ["3", "3", "3"]);
    let parted = "SELECT R, D, SUM(AMOUNT), SUM(SUM(AMOUNT)) OVER (PARTITION BY R) FROM S GROUP BY R, D";
    assert_eq!(sorted(&a, parted), ["NULL,2,1,1", "n,1,10,35", "n,2,25,35", "n,4,NULL,35", "s,1,5,12", "s,3,7,12"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.16 GR, §9.20 SR22, §10.10]
/// windows are computed before DISTINCT, which then compares their values
/// with the rest of a row, and before ORDER BY, which may sort by them.
/// Of a grouped query, DISTINCT is of the groups' columns the select list
/// reads, before the windows: four days of six groups.
#[test]
fn distinct_and_order_by_follow_windows_7_16() {
    let (_db, a) = sales();
    assert_eq!(sorted(&a, "SELECT DISTINCT R, COUNT(*) OVER (PARTITION BY R) FROM S"), ["NULL,1", "n,4", "s,2"]);
    assert_eq!(sorted(&a, "SELECT DISTINCT D, COUNT(*) OVER () FROM S GROUP BY R, D"), ["1,4", "2,4", "3,4", "4,4"]);
    let ranked = "SELECT D, AMOUNT, RANK() OVER (ORDER BY AMOUNT DESC) AS RK FROM S ORDER BY RK, D";
    assert_eq!(ordered(&a, ranked), ["4,NULL,1", "2,20,2", "1,10,3", "3,7,4", "1,5,5", "2,5,5", "2,1,7"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR3, SR5] a partition
/// is the rows not distinct in its columns, each by its collation, so
/// strings alike by it are of one partition; COLLATE names another.
#[test]
fn partitions_compare_by_collation_7_15() {
    let (_db, a) = sales();
    a.run("CREATE TABLE C (P VARCHAR(5) COLLATE CASE_INSENSITIVE, N INTEGER)").unwrap();
    a.run("INSERT INTO C VALUES ('a', 1), ('A', 2), ('b', 3)").unwrap();
    assert_eq!(sorted(&a, "SELECT N, COUNT(*) OVER (PARTITION BY P) FROM C"), ["1,2", "2,2", "3,1"]);
    assert_eq!(sorted(&a, "SELECT N, COUNT(*) OVER (PARTITION BY P COLLATE UCS_BASIC) FROM C"), ["1,1", "2,1", "3,1"]);
    a.rollback().unwrap();
}
