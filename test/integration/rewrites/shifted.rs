//! SUM of a value and a constant from the value's sum and count (RULES.md
//! D1216, `optimizer/shifted.rs`).

use turborel::optimizer::DISTINCT_AGGREGATES;
use turborel::optimizer::shifted::SHARE_SHIFTED_SUMS;

use turborel::db::Session;

use super::{COLUMNS, alike, left, plan, rule, tables};

/// Schema Q's tables, and of each store W: K, a key, null in some rows;
/// V, a SMALLINT; D, a DECIMAL(9, 2), null in some rows; B, a DECIMAL(30, 0); F,
/// approximate; and W0, of W's columns and no rows.
fn with_w() -> Session {
    let (db, s) = tables();
    for (prefix, store) in [("", ""), (COLUMNS, " USING columnstore")] {
        for table in ["W", "W0"] {
            s.run(&format!(
                "CREATE TABLE {prefix}{table} (K INTEGER, V SMALLINT, D DECIMAL(9, 2), B DECIMAL(30, 0), F DOUBLE PRECISION){store}"
            ))
            .unwrap();
        }
        s.run(&format!(
            "INSERT INTO {prefix}W SELECT K, CAST(V - 3 AS SMALLINT), CASE WHEN V = 2 THEN NULL ELSE CAST(V AS DECIMAL(9, 2)) / 4 END, \
             V * 1000000000000, V FROM A"
        ))
        .unwrap();
        s.commit().unwrap();
    }
    db.maintain().unwrap();
    s
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7a, GR7c, GR7d ii, GR7d vi]
/// [DESIGN] RULES.md D1216: SUM of an exact value and an exact literal,
/// added either way round or subtracted either way round, gives the sum
/// it gives with the rule off, of either store, by batches and row by
/// row: of many such sums of one value, which share one sum and count;
/// of a value with nulls, whose warning is raised still; of a scale of
/// the value's and of the literal's; under a filter clause; of each group
/// of a grouping, a group of nulls alone, whose sum is null, among them;
/// of no rows, null; beside the value's own SUM and COUNT, written
/// before or after; and of an expression of columns.
#[test]
fn sums_of_a_value_and_a_constant_share_its_sum_and_count() {
    let s = with_w();
    for sql in [
        "SELECT SUM(V + 1) FROM {W}",
        "SELECT SUM(V), SUM(V + 1), SUM(V + 2), SUM(3 + V), SUM(V - 4), SUM(5 - V), COUNT(V) FROM {W}",
        "SELECT SUM(D + 1), SUM(D - 0.125), SUM(100 - D) FROM {W}",
        "SELECT SUM(V + 1.5), SUM(K + 7) FROM {W}",
        "SELECT SUM(V + 1) FILTER (WHERE K > 100), SUM(V + 2) FILTER (WHERE K > 100), SUM(V + 2) FROM {W}",
        "SELECT V, SUM(D + 10), SUM(K - 1), COUNT(*) FROM {W} GROUP BY V",
        "SELECT V, SUM(D + 10) FROM {W} GROUP BY V HAVING SUM(D + 10) > 500 OR SUM(D + 10) IS NULL",
        "SELECT SUM(V + 1), SUM(D - 2) FROM {W0}",
        "SELECT K, SUM(V + 1) FROM {W0} GROUP BY K",
        "SELECT COUNT(V), SUM(V + 9), SUM(V) FROM {W} WHERE K < 50",
        "SELECT SUM((V * 2 - K) + 6) FROM {W}",
    ] {
        alike(&s, SHARE_SHIFTED_SUMS, sql);
    }
    let shown = plan(&s, &format!("SELECT SUM(V + 1), SUM(V + 2), SUM(7 - V) FROM {COLUMNS}W"));
    let physical = shown.split("-- physical:").nth(1).unwrap_or_default();
    assert!(physical.contains("computing [SUM(C_W.V), COUNT(C_W.V)]"), "one sum and one count: {shown}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7b i, GR7d iii, GR7d vi]
/// [DESIGN] RULES.md D1216: the rule leaves a SUM whose two forms could
/// differ: DISTINCT, made a grouping first or not; an approximate value or literal; a value of so many
/// digits that a sum of it could pass its type's range, as one of 30 digits;
/// AVG, whose quotient is truncated; a product; two constants; a call's
/// value; and grouping sets. Each gives what it gives with the rule off.
#[test]
fn sums_that_could_differ_are_left() {
    let s = with_w();
    for sql in [
        "SELECT SUM(DISTINCT V + 1) FROM {W}",
        "SELECT SUM(F + 1) FROM {W}",
        "SELECT SUM(V + 1.5E0) FROM {W}",
        "SELECT SUM(B + 1) FROM {W}",
        "SELECT AVG(V + 1) FROM {W}",
        "SELECT SUM(V * 2), MAX(V + 1), COUNT(V + 1) FROM {W}",
        "SELECT SUM(ABS(V) + 1) FROM {W}",
        "SELECT K, V, SUM(V + 1) FROM {W} GROUP BY GROUPING SETS ((K), (V))",
    ] {
        left(&s, SHARE_SHIFTED_SUMS, sql);
    }
    // Of a DISTINCT sum not made a grouping first.
    rule(&s, DISTINCT_AGGREGATES, false);
    left(&s, SHARE_SHIFTED_SUMS, "SELECT SUM(DISTINCT V + 1), SUM(V) FROM {W}");
    rule(&s, DISTINCT_AGGREGATES, true);
}

/// Verifies: [X3.135-1986 §5.8 SR9c] [DESIGN] RULES.md D1216: AVG of a
/// value and a constant is not AVG of the value and the constant where
/// the mean is below zero, as AVG truncates toward zero: of -1, -1 and 1
/// the mean of the values and 1 is 0.66, and the mean and 1 is 0.67. So
/// AVG is left, and gives the first.
#[test]
fn the_mean_of_a_value_and_a_constant_is_truncated_once() {
    let s = with_w();
    s.run("CREATE TABLE M (X INTEGER)").unwrap();
    s.run("INSERT INTO M VALUES (-1), (-1), (1)").unwrap();
    s.commit().unwrap();
    let mean = super::outcome(&s, "SELECT AVG(X + 1), AVG(X) + 1, SUM(X + 1) FROM M");
    assert!(mean.contains("0.66") && mean.contains("0.67"), "{mean}");
}
