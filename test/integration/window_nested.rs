//! Nested window functions [ISO/IEC 9075-2:2023 §6.11, Feature T619]:
//! ROW_NUMBER of each row marker over frames of ROWS, RANGE and GROUPS
//! with exclusions, partitioned; VALUE_OF at each marker moved either way,
//! past the partition giving the default, of a null and a parameter
//! offset, of its type and cast, nested in another's value expression; an
//! argument of the frame's row alone against the same over plain columns;
//! a grouped query; an empty frame; DISTINCT; and partitions past the work
//! area against a definition by a subquery. The Syntax Rules are
//! `window_nested_rules.rs`'s.

use turborel::value::{Decimal, Value};

use crate::common::{Counted, both_ways, exec, sorted};
use crate::windows::{by_row, ordered, prepared, sales};

/// The exact number `n`.
fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.11 GR2, GR3] ROW_NUMBER
/// of a marker is its row's place in the partition from 1: BEGIN_PARTITION
/// 1 and END_PARTITION the partition's rows; CURRENT_ROW the row computed
/// for; FRAME_ROW the frame's row the set function takes; BEGIN_FRAME and
/// END_FRAME the frame's first and last rows as its exclusion leaves it;
/// over ROWS, RANGE and GROUPS.
#[test]
fn row_numbers_of_each_marker_6_11() {
    let (_db, a) = sales();
    let packed = "MIN(ROW_NUMBER(BEGIN_FRAME) * 100 + ROW_NUMBER(END_FRAME) * 10 + ROW_NUMBER(CURRENT_ROW))";
    let rows = format!("{packed} OVER (ORDER BY I ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING EXCLUDE CURRENT ROW)");
    assert_eq!(by_row(&a, &rows), ["221", "132", "243", "354", "465", "576", "687", "778"]);
    let ends = "MAX(ROW_NUMBER(BEGIN_PARTITION) * 10 + ROW_NUMBER(END_PARTITION)) OVER (PARTITION BY R ORDER BY D, AMOUNT)";
    let sql = format!("SELECT R, D, AMOUNT, {ends} FROM S ORDER BY R, D, AMOUNT");
    assert_eq!(ordered(&a, &sql), ["n,1,10,14", "n,2,5,14", "n,2,20,14", "n,4,NULL,14", "s,1,5,12", "s,3,7,12", "NULL,2,1,11"]);
    let groups = "SUM(ROW_NUMBER(FRAME_ROW)) OVER (ORDER BY K GROUPS BETWEEN 1 PRECEDING AND 1 FOLLOWING EXCLUDE GROUP)";
    assert_eq!(by_row(&a, groups), ["3", "3", "7", "14", "19", "19", "11", "11"]);
    let range = "SUM(ROW_NUMBER(FRAME_ROW) - ROW_NUMBER(CURRENT_ROW)) OVER (ORDER BY I RANGE BETWEEN 2 PRECEDING AND 1 FOLLOWING \
                 EXCLUDE CURRENT ROW)";
    assert_eq!(by_row(&a, range), ["1", "0", "-2", "-2", "-2", "-2", "-2", "-3"]);
    // Peers are in no order, so each row's END_FRAME is told by its K.
    let ties = "SELECT K, MAX(ROW_NUMBER(END_FRAME)) OVER (ORDER BY K RANGE BETWEEN CURRENT ROW AND 1 FOLLOWING EXCLUDE TIES) AS E \
                FROM F ORDER BY K, E";
    let want = ["1,3", "1,3", "2,3", "4,6", "5,5", "5,6", "NULL,7", "NULL,8"];
    assert_eq!(ordered(&a, ties), want, "a row and the rows after its peers, of nulls the null rows");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.11 GR4] VALUE_OF is its
/// value expression at the marker's row moved by the offset, plus or
/// minus; past either end of the partition, the default, evaluated at the
/// frame's row, or null if none is written; a null left out warns 01003.
#[test]
fn value_of_at_each_marker_6_11() {
    let (_db, a) = sales();
    let lead = "SUM(VALUE_OF(V AT CURRENT_ROW + 1, 0)) OVER (ORDER BY I ROWS CURRENT ROW)";
    assert_eq!(by_row(&a, lead), ["20", "NULL", "40", "50", "60", "70", "80", "0"]);
    assert_eq!(by_row(&a, "MAX(VALUE_OF(V AT BEGIN_PARTITION + 1)) OVER ()"), ["20"; 8]);
    assert_eq!(by_row(&a, "MAX(VALUE_OF(V AT END_PARTITION - 1)) OVER ()"), ["70"; 8]);
    assert_eq!(by_row(&a, "MAX(VALUE_OF(V AT END_PARTITION + 1)) OVER ()"), ["NULL"; 8]);
    let ahead = "MAX(VALUE_OF(V AT END_FRAME + 1, -1)) OVER (ORDER BY I ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)";
    assert_eq!(by_row(&a, ahead), ["20", "NULL", "40", "50", "60", "70", "80", "-1"]);
    let behind = "SUM(VALUE_OF(V AT BEGIN_FRAME - 1, I)) OVER (ORDER BY I ROWS BETWEEN 1 PRECEDING AND CURRENT ROW)";
    assert_eq!(by_row(&a, behind), ["1", "3", "20", "40", "NULL", "80", "100", "120"], "a default of the frame's row's I");
    assert_eq!(a.warning().map(turborel::error::SqlState::as_str), Some("01003"), "V of the third row left out of the fifth's frame");
    let frame_row = "SUM(VALUE_OF(V AT FRAME_ROW - 1, 1000)) OVER (ORDER BY I ROWS BETWEEN CURRENT ROW AND 1 FOLLOWING)";
    assert_eq!(by_row(&a, frame_row), ["1010", "30", "20", "40", "90", "110", "130", "70"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.11 GR4d, GR4e i, SR3, SR4,
/// §20.7 GR5a] a null offset gives the default; an offset may be a
/// parameter, of a NUMERIC of scale 0, as may a default, of the value
/// expression's type; VALUE_OF is of the type of its value expression and
/// default, each cast to it; a nested window function may be in
/// VALUE_OF's value expression, read of the rows it is.
#[test]
fn value_of_offsets_types_and_nesting_6_11() {
    let (_db, a) = sales();
    let sql = "SELECT I, SUM(VALUE_OF(V AT CURRENT_ROW + ?, -5)) OVER (ORDER BY I ROWS CURRENT ROW) FROM F ORDER BY I";
    let strings = |s: &[&str]| Ok(s.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
    assert_eq!(prepared(&a, sql, &[Value::Null]), strings(&["-5"; 8]));
    assert_eq!(prepared(&a, sql, &[int(2)]), strings(&["NULL", "40", "50", "60", "70", "80", "-5", "-5"]));
    assert_eq!(prepared(&a, sql, &[int(-7)]), strings(&["-5", "-5", "-5", "-5", "-5", "-5", "-5", "10"]));
    let defaulted = "SELECT I, MAX(VALUE_OF(V AT CURRENT_ROW - 1, ?)) OVER (ORDER BY I ROWS CURRENT ROW) FROM F ORDER BY I";
    assert_eq!(prepared(&a, defaulted, &[int(7)]), strings(&["7", "10", "20", "NULL", "40", "50", "60", "70"]));
    let cast = "MAX(VALUE_OF(V AT CURRENT_ROW + 7, 1.5)) OVER (ORDER BY I ROWS CURRENT ROW)";
    assert_eq!(by_row(&a, cast), ["80.0", "1.5", "1.5", "1.5", "1.5", "1.5", "1.5", "1.5"], "INTEGER and DECIMAL(2,1) give a DECIMAL");
    let nested = "SUM(VALUE_OF(V + ROW_NUMBER(CURRENT_ROW) AT FRAME_ROW + 1, 0)) OVER (ORDER BY I ROWS CURRENT ROW)";
    assert_eq!(by_row(&a, nested), ["21", "NULL", "43", "54", "65", "76", "87", "0"], "the next row's V plus the current row's number");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.11 GR1, §10.9 GR]; [DESIGN]
/// an argument of the frame's row alone, computed once for each row, and
/// one of the current row too, computed for each frame, give what the
/// same set function gives of the plain columns, over frames that grow
/// and that move.
#[test]
fn arguments_of_the_frame_row_are_the_plain_columns_6_11() {
    let (_db, a) = sales();
    for w in [
        "ORDER BY I",
        "ORDER BY I ROWS BETWEEN 2 PRECEDING AND 1 FOLLOWING",
        "ORDER BY K RANGE BETWEEN 1 PRECEDING AND CURRENT ROW EXCLUDE GROUP",
        "PARTITION BY K ORDER BY I GROUPS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING",
    ] {
        let plain = sorted(&a, &format!("SELECT I, SUM(V) OVER ({w}), AVG(V * I) OVER ({w}) FROM F"));
        let alone = format!("SELECT I, SUM(VALUE_OF(V AT FRAME_ROW)) OVER ({w}), AVG(VALUE_OF(V * I AT FRAME_ROW + 0)) OVER ({w}) FROM F");
        assert_eq!(sorted(&a, &alone), plain, "{w}");
        let current =
            format!("SELECT I, SUM(V + 0 * ROW_NUMBER(CURRENT_ROW)) OVER ({w}), AVG(V * I + 0 * ROW_NUMBER(END_FRAME)) OVER ({w}) FROM F");
        assert_eq!(sorted(&a, &current), plain, "{w}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.11, §9.20, NOTE 170] in a
/// grouped query, VALUE_OF reads a set function of the groups at another
/// group; an empty frame evaluates no argument, so one that would fail
/// gives the set function of no rows; COUNT(DISTINCT ...) over the whole
/// partition counts each value of the argument of the current row once.
#[test]
fn grouped_empty_and_distinct_6_11() {
    let (_db, a) = sales();
    let sql = "SELECT K, SUM(VALUE_OF(SUM(V) AT CURRENT_ROW - 1, 0)) OVER (ORDER BY K ROWS CURRENT ROW) FROM F GROUP BY K ORDER BY K";
    assert_eq!(ordered(&a, sql), ["1,0", "2,30", "4,NULL", "5,40", "NULL,110"]);
    let failing = "1 / (ROW_NUMBER(CURRENT_ROW) - ROW_NUMBER(CURRENT_ROW))";
    let empty = format!("SUM({failing}) OVER (ORDER BY I ROWS BETWEEN 1 PRECEDING AND 2 PRECEDING)");
    assert_eq!(by_row(&a, &empty), ["NULL"; 8]);
    assert_eq!(by_row(&a, &format!("COUNT({failing}) OVER (ORDER BY I ROWS BETWEEN 2 FOLLOWING AND 1 FOLLOWING)")), ["0"; 8]);
    let apart = "(ROW_NUMBER(FRAME_ROW) - ROW_NUMBER(CURRENT_ROW))";
    let distinct = format!("COUNT(DISTINCT {apart} * {apart}) OVER ()");
    assert_eq!(by_row(&a, &distinct), ["8", "7", "6", "5", "5", "6", "7", "8"]);
}

/// Queries of nested window functions over partitions past a 64 KiB work
/// area: an argument computed once for each row, by a tree of states;
/// one computed for each frame; VALUE_OF at the partition's end and the
/// frame's start.
const SPILLED: [&str; 3] = [
    "SELECT I, SUM(VALUE_OF(V AT FRAME_ROW - 1, 0) * ROW_NUMBER(FRAME_ROW)) OVER (PARTITION BY P ORDER BY I ROWS BETWEEN 3 PRECEDING \
     AND 2 FOLLOWING) FROM N",
    "SELECT I, SUM(V * (ROW_NUMBER(FRAME_ROW) - ROW_NUMBER(CURRENT_ROW))) OVER (PARTITION BY P ORDER BY I ROWS BETWEEN 5 PRECEDING AND \
     5 FOLLOWING EXCLUDE CURRENT ROW) FROM N",
    "SELECT I, MAX(VALUE_OF(V AT END_PARTITION - 3, -1) + VALUE_OF(K AT BEGIN_FRAME + 1)) OVER (PARTITION BY P ORDER BY K, I GROUPS \
     BETWEEN 1 PRECEDING AND 1 FOLLOWING) FROM N",
];

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.11 GR]; [DESIGN] RULES.md
/// D38: past the work area, nested window functions give what they give
/// in memory, the rows, their columns and a precomputed argument spilled;
/// row by row and by batches alike; and a frame weighted by its rows'
/// distance from the current row is what its definition by a subquery
/// gives.
#[test]
fn nested_functions_past_the_work_area_spill_6_11() {
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
    let weighted = "SELECT I, SUM(V * (ROW_NUMBER(FRAME_ROW) - ROW_NUMBER(CURRENT_ROW)) * 3) OVER (PARTITION BY P ORDER BY I ROWS \
                    BETWEEN 5 PRECEDING AND 5 FOLLOWING) FROM N";
    let defined =
        "SELECT I, (SELECT SUM(M.V * M.I) - N.I * SUM(M.V) FROM N AS M WHERE M.P = N.P AND M.I BETWEEN N.I - 15 AND N.I + 15) FROM N";
    assert_eq!(sorted(&a, weighted), sorted(&a, defined), "rows of a partition 3 apart in I");
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
