//! Unit tests of the groups a comparison of a set function with a
//! constant keeps (RULES.md D1203): on a table of known statistics, each
//! set function's share, and the comparisons left the default.

use super::super::Estimates;
use super::normal_tail;
use crate::db::{Database, Session};

/// Schema H: L of 4,000 rows, O of 1,000 values, four rows each, Q of the
/// numbers 1 to 50 alike often; statistics gathered.
fn database() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION H CREATE TABLE L (O INTEGER NOT NULL, Q INTEGER NOT NULL)").unwrap();
    let s = db.session("H");
    let rows: Vec<String> = (0_i32..4_000_i32)
        .map(|k| format!("({}, {})", k.div_euclid(4_i32), k.wrapping_mul(7_919_i32).rem_euclid(50_i32).saturating_add(1_i32)))
        .collect();
    s.run(&format!("INSERT INTO L VALUES {}", rows.join(", "))).unwrap();
    s.commit().unwrap();
    db.analyze("H.L").unwrap();
    db.maintain().unwrap();
    (db, s)
}

/// The estimated rows of the groups of L by O that `having` keeps.
fn kept(s: &Session, having: &str) -> f64 {
    let (st, plan) = s.optimized(&format!("SELECT O FROM L GROUP BY O HAVING {having}")).unwrap();
    Estimates::new(&st).rows(&plan)
}

/// Whether `estimate` is within twice or half of `expected`.
fn near(estimate: f64, expected: f64) -> bool {
    estimate <= 2.0_f64 * expected && estimate >= expected / 2.0_f64
}

/// Verifies: [DESIGN] RULES.md D1203: the normal tail at its middle, at
/// the deviations that keep a fortieth, on both sides, and far out.
#[test]
fn the_normal_tail_is_the_complementary_error_function_s() {
    assert!((normal_tail(0.0_f64) - 0.5_f64).abs() < 1e-6_f64);
    assert!((normal_tail(1.959_964_f64) - 0.025_f64).abs() < 1e-6_f64);
    assert!((normal_tail(-1.959_964_f64) - 0.975_f64).abs() < 1e-6_f64);
    assert!(normal_tail(6.0_f64) < 1e-8_f64 && normal_tail(6.0_f64) >= 0.0_f64);
}

/// Verifies: [DESIGN] RULES.md D1203, R-TEST-7: a HAVING comparison of
/// SUM, AVG, COUNT, MAX or MIN of a column with a constant keeps the
/// share of the groups the column's statistics give, of groups of four
/// values of 1 to 50: a sum of mean 102 past 300 a group or so, past its
/// mean half, below 300 nearly all, past 200 the tail that the number of
/// rows varying widens; a mean past 40 a fiftieth; more than ten rows
/// none, one at least all; a greatest value within the lower half one in
/// sixteen, and a least value above it as many. A regression test: each
/// kept a third of the groups, so the 624 orders of 15 million a sum
/// kept were taken for 5 million, and the table joined to them built.
#[test]
fn a_set_function_compared_keeps_what_its_column_tells() {
    let (_db, s) = database();
    assert!(kept(&s, "SUM(Q) > 300") < 5.0_f64, "far past the mean: {}", kept(&s, "SUM(Q) > 300"));
    assert!(near(kept(&s, "SUM(Q) > 102"), 500.0_f64), "past the mean: {}", kept(&s, "SUM(Q) > 102"));
    assert!(near(kept(&s, "300 > SUM(Q)"), 1_000.0_f64), "below: {}", kept(&s, "300 > SUM(Q)"));
    assert!(near(kept(&s, "SUM(Q) > 200"), 31.0_f64), "the tail of rows that vary in number: {}", kept(&s, "SUM(Q) > 200"));
    assert!(near(kept(&s, "AVG(Q) >= 40"), 22.0_f64), "a mean: {}", kept(&s, "AVG(Q) >= 40"));
    assert!(kept(&s, "COUNT(*) > 10") < 5.0_f64 && near(kept(&s, "COUNT(*) >= 1"), 1_000.0_f64), "counts of rows");
    assert!(kept(&s, "COUNT(Q) > 12") < 5.0_f64, "counts of values: {}", kept(&s, "COUNT(Q) > 12"));
    assert!(near(kept(&s, "MAX(Q) <= 25"), 62.5_f64), "the greatest within: {}", kept(&s, "MAX(Q) <= 25"));
    assert!(near(kept(&s, "MAX(Q) > 25"), 937.5_f64), "the greatest past: {}", kept(&s, "MAX(Q) > 25"));
    assert!(near(kept(&s, "MIN(Q) > 25"), 62.5_f64), "the least past: {}", kept(&s, "MIN(Q) > 25"));
    assert!(near(kept(&s, "MIN(Q) <= 10"), 590.0_f64), "the least within: {}", kept(&s, "MIN(Q) <= 10"));
}

/// Verifies: [DESIGN] RULES.md D1203: a comparison the statistics do not
/// tell keeps the default, a third of the groups: of a filtered set
/// function, of one of an expression, and of two set functions; and an equality one group in a hundred.
#[test]
fn a_comparison_the_statistics_do_not_tell_keeps_the_default() {
    let (_db, s) = database();
    for having in ["SUM(Q) FILTER (WHERE Q > 3) > 300", "SUM(Q + 1) > 300", "SUM(Q) > MAX(Q) * 6"] {
        assert!(near(kept(&s, having), 333.0_f64), "{having}: {}", kept(&s, having));
    }
    assert!(near(kept(&s, "SUM(Q) = 100"), 10.0_f64), "an equality: {}", kept(&s, "SUM(Q) = 100"));
}
