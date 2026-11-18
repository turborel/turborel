//! The statistical set functions of Feature T621 [ISO/IEC 9075-2:2023
//! §10.9]: VAR_POP, VAR_SAMP, STDDEV_POP and STDDEV_SAMP, and the binary
//! set functions, against values worked by hand; pairs with a null
//! dropped and warned of; one row and none; REGR_R2's and CORR's special
//! cases; DECFLOAT operands; FILTER, GROUP BY, ROLLUP and windows; each
//! Syntax Rule; and 22003 past the range of a double.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};
use turborel::value::Value;

use crate::common::{code, exec, fetched, rows};

/// How near a double result must be to the value worked by hand.
const NEAR: f64 = 1e-12;

/// Schema A: P, of pairs (Y, X) in groups G, and of the same as DECFLOAT,
/// D and E. Group 1 is the hand-worked pairs (2, 1), (4, 2), (5, 3),
/// (9, 4): N 4, SX 10, SY 20, SXS 30, SYS 126, SXY 61, so SXX 5, SYY 26,
/// SXY 11. Group 2 is group 1 with a pair of a null Y and one of a null
/// X. Group 3 is the one pair (7, 3); group 4 X alike, 2; group 5 Y
/// alike, 6; group 6 falling, Y = 4 − X.
fn pairs() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE P (G INTEGER, I INTEGER, Y DOUBLE PRECISION, X INTEGER, D DECFLOAT, E DECFLOAT)")
        .unwrap();
    let a = db.session("A");
    a.run(
        "INSERT INTO P VALUES (1, 1, 2, 1, 2, 1), (1, 2, 4, 2, 4, 2), (1, 3, 5, 3, 5, 3), (1, 4, 9, 4, 9, 4), \
         (2, 1, 2, 1, 2, 1), (2, 2, 4, 2, 4, 2), (2, 3, 5, 3, 5, 3), (2, 4, 9, 4, 9, 4), (2, 5, NULL, 5, NULL, 5), (2, 6, 8, NULL, 8, NULL), \
         (3, 1, 7, 3, 7, 3), (4, 1, 1, 2, 1, 2), (4, 2, 5, 2, 5, 2), (5, 1, 6, 1, 6, 1), (5, 2, 6, 3, 6, 3), \
         (6, 1, 3, 1, 3, 1), (6, 2, 2, 2, 2, 2), (6, 3, 1, 3, 1, 3)",
    )
    .unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The values of the one row `sql` gives.
fn row(a: &Session, sql: &str) -> Vec<Value> {
    let mut c = a.open_cursor(sql).unwrap();
    let got = c.fetch().unwrap().unwrap().clone();
    assert!(c.fetch().unwrap().is_none(), "{sql} gives one row");
    got
}

/// That `got` is, of each of `want`, null of none, else a double within
/// `NEAR` of it.
fn near(got: &[Value], want: &[Option<f64>], sql: &str) {
    assert_eq!(got.len(), want.len(), "{sql}");
    for (g, w) in got.iter().zip(want) {
        match (g, w) {
            (Value::Null, None) => {}
            (Value::Approx(g), Some(w)) => assert!((g - w).abs() <= NEAR * w.abs().max(1.0), "{sql}: {g} is not {w}"),
            (g, w) => panic!("{sql}: {g} is not {w:?}"),
        }
    }
}

/// `near` of the one row `sql` gives.
fn gives(a: &Session, sql: &str, want: &[Option<f64>]) {
    near(&row(a, sql), want, sql);
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7d ix, SR7h ii, SR7i, SR7j]
/// of X 1, 2, 3, 4: SX 10, SXS 30, so VAR_POP (30 − 100 / 4) / 4 = 1.25,
/// VAR_SAMP 5 / 3, STDDEV_POP √1.25, STDDEV_SAMP √(5 / 3); of Y 2, 4, 5,
/// 9, VAR_POP 26 / 4 and VAR_SAMP 26 / 3: each DOUBLE PRECISION, of an
/// INTEGER argument as of a DOUBLE one, a row at a time and in batches.
#[test]
fn variances_of_values_worked_by_hand_10_9() {
    let (_db, a) = pairs();
    for batches in ["OFF", "ON"] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        gives(
            &a,
            "SELECT VAR_POP(X), VAR_SAMP(X), STDDEV_POP(X), STDDEV_SAMP(X), VAR_POP(Y), VAR_SAMP(Y) FROM P WHERE G = 1",
            &[Some(1.25), Some(5.0 / 3.0), Some(1.25_f64.sqrt()), Some((5.0_f64 / 3.0).sqrt()), Some(6.5), Some(26.0 / 3.0)],
        );
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR8b, GR8c, SR9c] of the pairs
/// of group 1: REGR_COUNT 4, an exact number; REGR_SXX 5, REGR_SYY 26,
/// REGR_SXY 11, REGR_AVGX 2.5, REGR_AVGY 5, COVAR_POP 11 / 4, COVAR_SAMP
/// 11 / 3, REGR_SLOPE 11 / 5, REGR_INTERCEPT (20 × 30 − 10 × 61) / (4 ×
/// 30 − 100) = −0.5, REGR_R2 121 / 130 and CORR √(121 / 130).
#[test]
fn binary_set_functions_of_values_worked_by_hand_10_9() {
    let (_db, a) = pairs();
    assert_eq!(rows(&a, "SELECT REGR_COUNT(Y, X) FROM P WHERE G = 1").unwrap(), ["4"]);
    let r2 = 121.0_f64 / 130.0;
    gives(
        &a,
        "SELECT REGR_SXX(Y, X), REGR_SYY(Y, X), REGR_SXY(Y, X), REGR_AVGX(Y, X), REGR_AVGY(Y, X), COVAR_POP(Y, X), \
         COVAR_SAMP(Y, X), REGR_SLOPE(Y, X), REGR_INTERCEPT(Y, X), REGR_R2(Y, X), CORR(Y, X) FROM P WHERE G = 1",
        &[
            Some(5.0),
            Some(26.0),
            Some(11.0),
            Some(2.5),
            Some(5.0),
            Some(2.75),
            Some(11.0 / 3.0),
            Some(2.2),
            Some(-0.5),
            Some(r2),
            Some(r2.sqrt()),
        ],
    );
    // Y and X swapped: the fit of X on Y.
    gives(&a, "SELECT REGR_SLOPE(X, Y), REGR_AVGX(X, Y) FROM P WHERE G = 1", &[Some(11.0 / 26.0), Some(5.0)]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7a, GR8a] a pair with a null Y
/// or a null X is dropped, so group 2 gives group 1's results, and the
/// warning null value eliminated in set function, 01003, is raised; of
/// VAR_POP X's null alone is dropped.
#[test]
fn pairs_with_a_null_are_dropped_and_warned_10_9() {
    let (_db, a) = pairs();
    let sql = "SELECT REGR_COUNT(Y, X), REGR_SLOPE(Y, X), COVAR_SAMP(Y, X), CORR(Y, X) FROM P WHERE G = 2";
    let got = row(&a, sql);
    assert_eq!(a.warning(), Some(SqlState::NULL_VALUE_ELIMINATED));
    assert_eq!(got.first().map(ToString::to_string), Some("4".to_owned()));
    near(&got[1..], &[Some(2.2), Some(11.0 / 3.0), Some((121.0_f64 / 130.0).sqrt())], sql);
    gives(&a, "SELECT VAR_POP(X), REGR_AVGX(X, X) FROM P WHERE G = 2", &[Some(2.0), Some(3.0)]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7d ii, GR7d ix 2 A, GR8c i,
/// GR8c ii, GR8c ix 1] of no rows REGR_COUNT is 0 and every other null;
/// of one pair VAR_POP 0, VAR_SAMP, STDDEV_SAMP and COVAR_SAMP null,
/// REGR_SXX 0, REGR_AVGY its Y, and REGR_SLOPE null, its line not
/// determined.
#[test]
fn one_row_and_none_10_9() {
    let (_db, a) = pairs();
    let none = "WHERE G = 9";
    assert_eq!(rows(&a, &format!("SELECT REGR_COUNT(Y, X) FROM P {none}")).unwrap(), ["0"]);
    gives(&a, &format!("SELECT VAR_POP(X), STDDEV_SAMP(X), CORR(Y, X), REGR_AVGX(Y, X), REGR_SXY(Y, X) FROM P {none}"), &[None; 5]);
    gives(
        &a,
        "SELECT VAR_POP(X), VAR_SAMP(X), STDDEV_POP(Y), STDDEV_SAMP(Y), COVAR_POP(Y, X), COVAR_SAMP(Y, X), REGR_SXX(Y, X), \
         REGR_AVGY(Y, X), REGR_SLOPE(Y, X), CORR(Y, X) FROM P WHERE G = 3",
        &[Some(0.0), None, Some(0.0), None, Some(0.0), None, Some(0.0), Some(7.0), None, None],
    );
    // Of groups, an empty one is no group at all.
    assert_eq!(rows(&a, "SELECT G, REGR_COUNT(Y, X) FROM P WHERE G = 9 GROUP BY G").unwrap(), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR8c x, xi, xii, xiii] X all
/// alike, N × SXS = SX × SX: CORR, REGR_R2, REGR_SLOPE and REGR_INTERCEPT
/// null; Y all alike: CORR null, REGR_R2 1, REGR_SLOPE 0, REGR_INTERCEPT
/// Y's value.
#[test]
fn regr_r2_and_corr_of_values_alike_10_9() {
    let (_db, a) = pairs();
    let fits = "SELECT CORR(Y, X), REGR_R2(Y, X), REGR_SLOPE(Y, X), REGR_INTERCEPT(Y, X) FROM P WHERE G = ";
    gives(&a, &format!("{fits}4"), &[None, None, None, None]);
    gives(&a, &format!("{fits}5"), &[None, Some(1.0), Some(0.0), Some(6.0)]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR8c x 3] CORR is SQRT ( POWER (
/// N × SXY − SX × SY, 2 ) / ... ), not below 0, so of Y falling as X
/// rises it is 1, while REGR_SLOPE is −1 and COVAR_POP −2 / 3.
#[test]
fn corr_is_a_square_root_10_9() {
    let (_db, a) = pairs();
    gives(&a, "SELECT CORR(Y, X), REGR_SLOPE(Y, X), COVAR_POP(Y, X) FROM P WHERE G = 6", &[Some(1.0), Some(-1.0), Some(-2.0 / 3.0)]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 SR7h i, SR9c ii 1, GR7d ix,
/// GR8c] of DECFLOAT operands the result is DECFLOAT, found in decimal:
/// VAR_POP 1.25, VAR_SAMP of Y 26 / 3 to 34 digits, STDDEV_POP of 1 and
/// 3 exactly 1, REGR_SLOPE 2.2, REGR_INTERCEPT −0.5, REGR_R2 121 / 130;
/// of a DECFLOAT Y and an INTEGER X too; of X alike STDDEV_POP 0, CORR
/// null; of Y alike REGR_R2 1.
#[test]
fn decfloat_operands_are_found_in_decimal_10_9() {
    let (_db, a) = pairs();
    assert_eq!(
        rows(
            &a,
            "SELECT VAR_POP(E), VAR_SAMP(D), REGR_SLOPE(D, E), REGR_INTERCEPT(D, X), REGR_R2(D, E), REGR_COUNT(D, E) FROM P WHERE G = 1"
        )
        .unwrap(),
        ["1.25,8.666666666666666666666666666666667,2.2,-0.5,0.9307692307692307692307692307692308,4"]
    );
    assert_eq!(rows(&a, "SELECT STDDEV_POP(E), STDDEV_SAMP(E), CORR(D, E) FROM P WHERE G = 4").unwrap(), ["0,0,NULL"]);
    assert_eq!(rows(&a, "SELECT STDDEV_POP(D) FROM P WHERE G = 6 AND I <> 2").unwrap(), ["1"]);
    assert_eq!(rows(&a, "SELECT CORR(D, E), REGR_R2(D, E) FROM P WHERE G = 5").unwrap(), ["NULL,1"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR4, §7.13, §7.9] the set
/// functions take a FILTER clause, and are of each group of a GROUP BY
/// and of each grouping set of a ROLLUP.
#[test]
fn filtered_grouped_and_rolled_up_10_9() {
    let (_db, a) = pairs();
    gives(&a, "SELECT REGR_SLOPE(Y, X) FILTER (WHERE G = 1), VAR_POP(X) FILTER (WHERE G = 1) FROM P", &[Some(2.2), Some(1.25)]);
    assert_eq!(
        rows(&a, "SELECT G, REGR_COUNT(Y, X), CAST(VAR_SAMP(X) * 12 AS INTEGER) FROM P WHERE G IN (1, 2) GROUP BY G ORDER BY G").unwrap(),
        ["1,4,20", "2,4,30"]
    );
    // All 7 pairs: X 1, 2, 3, 4, 3, 2, 2: SX 17, SXS 47, so SXX 47 − 289 / 7, about 5.7.
    let rolled = "SELECT G, REGR_COUNT(Y, X), CAST(REGR_SXX(Y, X) * 10 AS INTEGER) FROM P WHERE G IN (1, 3, 4) \
                  GROUP BY ROLLUP (G) ORDER BY G NULLS LAST";
    assert_eq!(rows(&a, rolled).unwrap(), ["1,4,50", "3,1,0", "4,2,0", "NULL,7,57"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR1e, §7.15, §6.10] each is a
/// window function: over a frame of the row and the one before it,
/// VAR_POP of X and REGR_COUNT; over a partition, REGR_SLOPE of each row.
#[test]
fn window_functions_10_9() {
    let (_db, a) = pairs();
    let framed = "SELECT I, VAR_POP(X) OVER (ORDER BY I ROWS BETWEEN 1 PRECEDING AND CURRENT ROW), \
                  REGR_COUNT(Y, X) OVER (ORDER BY I ROWS BETWEEN 1 PRECEDING AND CURRENT ROW) FROM P WHERE G = 2 ORDER BY I";
    assert_eq!(rows(&a, framed).unwrap(), ["1,0E0,1", "2,2.5E-1,2", "3,2.5E-1,2", "4,2.5E-1,2", "5,2.5E-1,1", "6,0E0,0"]);
    assert_eq!(
        rows(&a, "SELECT G, CAST(REGR_SLOPE(Y, X) OVER (PARTITION BY G) * 10 AS INTEGER) FROM P WHERE G IN (1, 6) ORDER BY G, I").unwrap(),
        ["1,22", "1,22", "1,22", "1,22", "6,-10", "6,-10", "6,-10"]
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 SR2, SR7h, SR9] STDDEV_POP,
/// STDDEV_SAMP, VAR_POP and VAR_SAMP take no DISTINCT or ALL; the
/// argument of each is a number, as is Y and X of a binary set function,
/// else SQLCODE -103; and the names are still columns' where no "("
/// follows.
#[test]
fn syntax_rules_10_9() {
    let (_db, a) = pairs();
    for sql in [
        "SELECT VAR_POP(DISTINCT X) FROM P",
        "SELECT STDDEV_SAMP(ALL X) FROM P",
        "SELECT VAR_SAMP('A') FROM P",
        "SELECT REGR_SLOPE(Y, 'A') FROM P",
        "SELECT CORR(DATE '2026-01-01', X) FROM P",
    ] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(fetched(&a, "SELECT CORR FROM (SELECT X AS CORR FROM P) AS T").unwrap(), 18);
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR3e] a sum of squares past the
/// range of a double raises 22003, as does a DECFLOAT square past its
/// range.
#[test]
fn out_of_range_is_22003_10_9() {
    let (_db, a) = pairs();
    a.run("INSERT INTO P VALUES (7, 1, 1E300, 1, CAST('9E4000' AS DECFLOAT), 1), (7, 2, -1E300, 2, CAST('-9E4000' AS DECFLOAT), 2)")
        .unwrap();
    for sql in ["SELECT VAR_POP(Y) FROM P WHERE G = 7", "SELECT COVAR_POP(D, X) FROM P WHERE G = 7"] {
        assert_eq!(fetched(&a, sql).unwrap_err().state, Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE), "{sql}");
    }
}
