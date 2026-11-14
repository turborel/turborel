//! Framed COUNT DISTINCT [ISO/IEC 9075-2:2023 §9.23 SR8d, §10.9 GR4b,
//! Feature T627]: COUNT(DISTINCT x) over a window of an ordering or a
//! frame, each row's value against a correlated subquery of its frame's
//! rows written out, of ROWS, RANGE and GROUPS frames, frames that grow,
//! move and shrink, each exclusion, a filter, a collation, nested window
//! functions, nulls, partitions, and past the work area; the other
//! DISTINCT set functions still refused.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};

use crate::common::{Counted, exec, fetched, sorted};

/// The rows of W: I from 1 to 35, G 1 to I 20 and 2 after, K a third of
/// I, so of peers, V of five values, null of each sixth row, and S of
/// strings alike but in case.
fn counted_rows() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE W (I INTEGER, G INTEGER, K INTEGER, V INTEGER, S VARCHAR(3))").unwrap();
    let a = db.session("A");
    let strings = ["'a'", "'A'", "'b'", "'B'", "'c'"];
    let rows: Vec<String> = (1_u32..=35)
        .map(|i| {
            let g = if i <= 20 { 1 } else { 2 };
            let v =
                if i.checked_rem(6) == Some(0) { "NULL".to_owned() } else { i.checked_mul(7).unwrap().checked_rem(5).unwrap().to_string() };
            let s = strings[usize::try_from(i.checked_rem(5).unwrap()).unwrap()];
            format!("({i}, {g}, {}, {v}, {s})", i.checked_div(3).unwrap())
        })
        .collect();
    a.run(&format!("INSERT INTO W VALUES {}", rows.join(", "))).unwrap();
    a.commit().unwrap();
    (db, a)
}

/// Each row's I and `call` over a window, against each row's I and the
/// count of the distinct `x` of the rows of W of its G that `frame`, a
/// condition of W2 and W, takes.
fn against(a: &Session, (call, x): (&str, &str), frame: &str) {
    let windowed = format!("SELECT I, {call} FROM W");
    let defined = format!("SELECT I, (SELECT COUNT(DISTINCT {x}) FROM W AS W2 WHERE W2.G = W.G AND {frame}) FROM W");
    assert_eq!(sorted(a, &windowed), sorted(a, &defined), "{call}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.23 SR8d ii, §7.15 GR5,
/// §10.9 GR4b, Feature T627] COUNT(DISTINCT x) over a frame counts each
/// value of its rows once: of ROWS frames that move, start at the current
/// row, or end there; of RANGE and GROUPS frames of peers; of frames that
/// grow; each in its partition.
#[test]
fn framed_count_distinct_counts_each_value_once_9_23() {
    let (_db, a) = counted_rows();
    let cases = [
        ("ROWS BETWEEN 2 PRECEDING AND 1 FOLLOWING", "W2.I BETWEEN W.I - 2 AND W.I + 1"),
        ("ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING", "W2.I >= W.I"),
        ("ROWS BETWEEN 3 FOLLOWING AND 5 FOLLOWING", "W2.I BETWEEN W.I + 3 AND W.I + 5"),
        ("ROWS UNBOUNDED PRECEDING", "W2.I <= W.I"),
    ];
    for (frame, condition) in cases {
        against(&a, (&format!("COUNT(DISTINCT V) OVER (PARTITION BY G ORDER BY I {frame})"), "W2.V"), condition);
    }
    let peers = [
        ("RANGE BETWEEN 1 PRECEDING AND 2 FOLLOWING", "W2.K BETWEEN W.K - 1 AND W.K + 2"),
        ("GROUPS BETWEEN 1 PRECEDING AND CURRENT ROW", "W2.K BETWEEN W.K - 1 AND W.K"),
        ("RANGE BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING", "W2.K >= W.K"),
    ];
    for (frame, condition) in peers {
        against(&a, (&format!("COUNT(DISTINCT V) OVER (PARTITION BY G ORDER BY K {frame})"), "W2.V"), condition);
    }
    against(&a, ("COUNT(DISTINCT V) OVER (PARTITION BY G ORDER BY K)", "W2.V"), "W2.K <= W.K");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5b ii, §9.23 SR8d
/// ii, Feature T627] what an exclusion leaves out of a frame is not
/// counted: the current row, its peers, or its peers but itself, of frames
/// that move.
#[test]
fn exclusions_leave_values_out_7_15() {
    let (_db, a) = counted_rows();
    let rows = "PARTITION BY G ORDER BY I ROWS BETWEEN 2 PRECEDING AND 2 FOLLOWING EXCLUDE CURRENT ROW";
    against(&a, (&format!("COUNT(DISTINCT V) OVER ({rows})"), "W2.V"), "W2.I BETWEEN W.I - 2 AND W.I + 2 AND W2.I <> W.I");
    let range = |exclusion: &str| {
        format!("COUNT(DISTINCT V) OVER (PARTITION BY G ORDER BY K RANGE BETWEEN 1 PRECEDING AND 1 FOLLOWING {exclusion})")
    };
    let near = "W2.K BETWEEN W.K - 1 AND W.K + 1";
    against(&a, (&range("EXCLUDE GROUP"), "W2.V"), &format!("{near} AND W2.K <> W.K"));
    against(&a, (&range("EXCLUDE TIES"), "W2.V"), &format!("{near} AND (W2.K <> W.K OR W2.I = W.I)"));
    against(&a, (&range("EXCLUDE CURRENT ROW"), "W2.V"), &format!("{near} AND W2.I <> W.I"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR4, GR4b, §9.15,
/// §6.11, Feature T627] of a frame that moves, a filter leaves rows out of
/// the count; values alike by a collation are counted once; a nested
/// window function's argument, computed once or for each frame, is
/// counted as the frame's values are; a null left out warns 01003, and
/// none does of no null, nor of a row the filter does not take.
#[test]
fn filters_collations_nested_and_nulls_10_9() {
    let (_db, a) = counted_rows();
    let moving = "PARTITION BY G ORDER BY I ROWS BETWEEN 3 PRECEDING AND CURRENT ROW";
    let near = "W2.I BETWEEN W.I - 3 AND W.I";
    against(&a, (&format!("COUNT(DISTINCT V) FILTER (WHERE V > 1) OVER ({moving})"), "CASE WHEN W2.V > 1 THEN W2.V END"), near);
    against(&a, (&format!("COUNT(DISTINCT S COLLATE CASE_INSENSITIVE) OVER ({moving})"), "W2.S COLLATE CASE_INSENSITIVE"), near);
    against(&a, (&format!("COUNT(DISTINCT S) OVER ({moving})"), "W2.S"), near);
    against(&a, (&format!("COUNT(DISTINCT VALUE_OF(V AT FRAME_ROW)) OVER ({moving})"), "W2.V"), near);
    against(&a, (&format!("COUNT(DISTINCT V + 0 * ROW_NUMBER(CURRENT_ROW)) OVER ({moving})"), "W2.V"), near);
    sorted(&a, &format!("SELECT COUNT(DISTINCT V) OVER ({moving}) FROM W"));
    assert_eq!(a.warning().map(SqlState::as_str), Some("01003"), "a null left out");
    sorted(&a, &format!("SELECT COUNT(DISTINCT S) OVER ({moving}) FROM W"));
    assert_eq!(a.warning(), None, "no null left out");
    sorted(&a, &format!("SELECT COUNT(DISTINCT S) FILTER (WHERE I > 3) OVER ({moving}) FROM W"));
    assert_eq!(a.warning(), None, "a row not taken is no null left out");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.23 SR8d ii, RULES.md D38]
/// a partition past the work area, its rows and the counts of its values
/// spilled, counts what it does in memory: of a frame whose values are
/// all distinct, its rows but the current one; of one whose values come
/// round again, as many as it holds of them.
#[test]
fn framed_count_distinct_past_the_work_area_9_23() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE M (I INTEGER, V INTEGER, R INTEGER)").unwrap();
    let a = db.session("A");
    for first in (0_u32..6000).step_by(500) {
        // V of 3,000 values and R of 1,000, each a row's I times 7, which
        // is coprime to both, so they come round every 3,000 and 1,000 rows.
        let values: Vec<String> = (first..first.checked_add(500).unwrap())
            .map(|i| {
                let seventh = i.checked_mul(7).unwrap();
                format!("({i}, {}, {})", seventh.checked_rem(3000).unwrap(), seventh.checked_rem(1000).unwrap())
            })
            .collect();
        a.run(&format!("INSERT INTO M VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    let counted = "SELECT I, COUNT(DISTINCT V) OVER (ORDER BY I ROWS BETWEEN 1000 PRECEDING AND 999 FOLLOWING EXCLUDE CURRENT ROW), \
                   COUNT(DISTINCT R) OVER (ORDER BY I ROWS BETWEEN 1000 PRECEDING AND 499 FOLLOWING) FROM M";
    let in_memory = sorted(&a, counted);
    let rows = |i: i64, after: i64| i.saturating_add(after).min(5999).saturating_sub(i.saturating_sub(1000).max(0));
    let mut frames: Vec<String> =
        (0_i64..6000).map(|i| format!("{i},{},{}", rows(i, 999), rows(i, 499).saturating_add(1).min(1000))).collect();
    frames.sort();
    assert_eq!(in_memory, frames);
    let spilled = Counted::default();
    db.set_spill(Box::new(spilled.clone())).unwrap();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    assert_eq!(sorted(&a, counted), in_memory);
    assert!(spilled.taken() > 0, "the counts spilled");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.23 SR8d ii] over a window
/// of an ordering or a frame, a DISTINCT set function but COUNT is
/// SQLCODE -103, citing its rule.
#[test]
fn other_distinct_set_functions_stay_refused_9_23() {
    let (_db, a) = counted_rows();
    for sql in ["SELECT SUM(DISTINCT V) OVER (ORDER BY I) FROM W", "SELECT MAX(DISTINCT V) OVER (ROWS 2 PRECEDING) FROM W"] {
        let e = fetched(&a, sql).expect_err(sql);
        assert_eq!((e.code, e.rule.as_deref().is_some_and(|r| r.contains("§9.23 SR8d ii"))), (Code::SyntaxRule, true), "{sql}");
    }
}
