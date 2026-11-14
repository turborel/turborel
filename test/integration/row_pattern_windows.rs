//! Row pattern recognition in a window [ISO/IEC 9075-2:2023 §6.10 GR1e,
//! §7.15 SR13, SR14, GR6, §9.23 SR9, Feature R020], and in FROM over more
//! rows than the work area holds.

use turborel::db::Database;
use turborel::error::Code;

use crate::common::{code, exec, expected, prices, recognized, rows};

/// A window W over T's rows of ticker A, by D, of the measures
/// `measures`, the skip `skip` and INITIAL or SEEK `seek`, the V pattern.
fn windowed(select: &str, measures: &str, skip: &str, seek: &str) -> String {
    format!(
        "SELECT D, {select} FROM T WHERE TICKER = 'A' WINDOW W AS (ORDER BY D MEASURES {measures} ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING \
         AFTER MATCH {skip} {seek} PATTERN (STRT DOWN+ UP+) DEFINE DOWN AS PRICE < PREV(PRICE), UP AS PRICE > PREV(PRICE)) ORDER BY D"
    )
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR6d iii, §6.10 GR1e,
/// §9.23 SR9, §7.9 SR3b ii, Feature R020] each row's frame is narrowed to
/// the match starting at it, INITIAL implicit; a row a match skips past, or
/// of no match, has an empty frame and null measures; window functions over
/// the window see the narrowed frame.
#[test]
fn a_frame_is_narrowed_to_its_match_7_15() {
    let (_db, a) = prices();
    let q = windowed("E OVER W, COUNT(*) OVER W, SUM(PRICE) OVER W", "LAST(UP.D) AS E", "SKIP PAST LAST ROW", "");
    let want = [
        "1,6,6,57",
        "2,NULL,0,NULL",
        "3,NULL,0,NULL",
        "4,NULL,0,NULL",
        "5,NULL,0,NULL",
        "6,NULL,0,NULL",
        "7,9,3,31",
        "8,NULL,0,NULL",
        "9,NULL,0,NULL",
    ];
    assert_eq!(rows(&a, &q), expected(&want));
    assert_eq!(
        rows(&a, &windowed("E OVER W", "LAST(UP.D) AS E", "SKIP PAST LAST ROW", "INITIAL")),
        rows(&a, &windowed("E OVER W", "LAST(UP.D) AS E", "SKIP PAST LAST ROW", ""))
    );
    let next = windowed("S OVER W, E OVER W", "FIRST(STRT.D) AS S, LAST(UP.D) AS E", "SKIP TO NEXT ROW", "INITIAL");
    assert_eq!(
        rows(&a, &next),
        expected(&["1,1,6", "2,2,6", "3,NULL,NULL", "4,NULL,NULL", "5,NULL,NULL", "6,6,9", "7,7,9", "8,NULL,NULL", "9,NULL,NULL"])
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR6d iii 2 B II, NOTE
/// 298] SEEK takes the first match starting in each row's frame, from its
/// row on.
#[test]
fn seek_takes_the_first_match_of_the_frame_7_15() {
    let (_db, a) = prices();
    let q = windowed("S OVER W, COUNT(*) OVER W", "FIRST(STRT.D) AS S", "SKIP TO NEXT ROW", "SEEK");
    assert_eq!(rows(&a, &q), expected(&["1,1,6", "2,2,5", "3,6,4", "4,6,4", "5,6,4", "6,6,4", "7,7,3", "8,NULL,0", "9,NULL,0"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 SR13, SR14b to SR14e,
/// §9.23 SR9, §6.31 SR22a] a window's row pattern recognition is of a frame
/// of ROWS BETWEEN CURRENT ROW AND a bound, EXCLUDE NO OTHERS, of no `^` or
/// `$` and no MATCH_NUMBER; a measure named OVER it is one of its measures;
/// each SQLCODE -103. MC/DC of `!CurrentRow || end.is_none()`: from
/// UNBOUNDED PRECEDING to a bound refuses; CURRENT ROW of no end refuses;
/// from CURRENT ROW to a bound accepts.
#[test]
fn a_windows_row_pattern_rules_7_15() {
    let (_db, a) = prices();
    let base = windowed("E OVER W", "LAST(UP.D) AS E", "SKIP PAST LAST ROW", "");
    let refused = |from: &str, to: &str, why: &str| {
        let sql = base.replace(from, to);
        assert_ne!(sql, base, "{why}");
        assert_eq!(code(&a, &sql), Code::SyntaxRule, "{why}: {sql}");
    };
    refused("ROWS BETWEEN", "RANGE BETWEEN", "SR14b");
    refused("ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING", "ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW", "SR14c");
    refused("ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING", "ROWS CURRENT ROW", "SR14c");
    refused("UNBOUNDED FOLLOWING", "UNBOUNDED FOLLOWING EXCLUDE TIES", "SR14d");
    refused("(STRT DOWN+ UP+)", "(^ STRT DOWN+ UP+)", "SR14e");
    refused("E OVER W", "Q OVER W", "§9.23 SR9");
    refused("LAST(UP.D) AS E", "MATCH_NUMBER() AS E", "§6.31 SR22a");
    assert!(rows(&a, &base.replace("UNBOUNDED FOLLOWING", "UNBOUNDED FOLLOWING EXCLUDE NO OTHERS")).is_ok(), "SR14d");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7, RULES.md D38]
/// partitions of more rows than the work area holds, spilling, give the
/// matches they give in it, ONE ROW and ALL ROWS PER MATCH, by batches and
/// row by row.
#[test]
fn partitions_past_the_work_area_7_7() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (TICKER CHARACTER(1) NOT NULL, D INTEGER NOT NULL, PRICE INTEGER, PADDING VARCHAR(60))").unwrap();
    let a = db.session("A");
    let values: Vec<String> = (1..=4000_i64)
        .map(|d| format!("('{}', {d}, {}, 'padding padding padding padding padding')", if d % 2 == 0 { 'A' } else { 'B' }, (d * 37) % 23))
        .collect();
    a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    let one = format!(
        "{} ORDER BY TICKER, S",
        recognized("STRT.D AS S, LAST(UP.D) AS E, SUM(PRICE) AS P", "ONE ROW PER MATCH", "SKIP PAST LAST ROW", "STRT DOWN+ UP+")
    );
    let all = format!(
        "{} ORDER BY TICKER, D",
        recognized("RUNNING COUNT(*) AS N, FINAL LAST(D) AS E", "ALL ROWS PER MATCH", "SKIP TO NEXT ROW", "STRT DOWN+ UP+")
    );
    let held = (rows(&a, &one).unwrap(), rows(&a, &all).unwrap());
    assert!(held.0.len() > 100, "{}", held.0.len());
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    assert_eq!((rows(&a, &one).unwrap(), rows(&a, &all).unwrap()), held, "spilled");
    exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    assert_eq!((rows(&a, &one).unwrap(), rows(&a, &all).unwrap()), held, "row by row");
}
