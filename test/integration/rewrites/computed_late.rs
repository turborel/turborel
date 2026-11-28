//! A select list's expressions computed after FETCH FIRST (RULES.md
//! D1306, `optimizer/late_fetch/projected.rs`).

use turborel::db::Session;
use turborel::optimizer::{COMPUTE_AFTER_FETCH_FIRST, COMPUTE_KEYS_AFTER_GROUPING};

use super::dependent_keys::with_keys;
use super::{alike, left, plan, rewritten};

/// The lines of the part `part` of the plan EXPLAIN shows of `sql`, from
/// its FETCH FIRST down, without their common indentation.
fn from_fetch(s: &Session, sql: &str, part: &str) -> Vec<String> {
    let shown = plan(s, sql);
    let section = shown.split(part).nth(1).and_then(|rest| rest.split("-- ").next()).unwrap_or_default();
    let lines: Vec<&str> = section.lines().skip_while(|l| !l.trim_start().starts_with("Limit")).collect();
    let indent = lines.iter().map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    lines.iter().map(|l| l.get(indent..).unwrap_or(l).to_owned()).collect()
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR6, GR7]; [DESIGN] RULES.md
/// D1306, D15: a query that sorts by columns of its select list and
/// keeps the first rows gives the same rows in the same order with the
/// expressions of its select list that cannot raise computed after the
/// FETCH FIRST, of either store, by batches and row by row: of a
/// grouping and of a table's rows; a constant; an expression written
/// before the sort key, and ones that read the sort key, two columns and
/// columns not selected; with OFFSET, WITH TIES and a percentage; beside
/// an expression that may raise, which stays below and raises of a row
/// the FETCH FIRST drops with the rule on and off; and of a grouping by
/// expressions of a grouping column, both rules rewriting it.
#[test]
fn expressions_of_the_select_list_are_computed_of_the_rows_kept() {
    let (_db, s) = with_keys();
    for sql in [
        "SELECT V, V - 1, V * 2, COUNT(*) AS C FROM {CH} GROUP BY V ORDER BY C DESC, V FETCH FIRST 3 ROWS ONLY",
        "SELECT 1, S, COUNT(*) AS C FROM {CH} GROUP BY S ORDER BY C DESC, S FETCH FIRST 2 ROWS ONLY",
        "SELECT V + 1, K, P FROM {CH} ORDER BY K DESC FETCH FIRST 5 ROWS ONLY",
        "SELECT K, K + 1, V + P FROM {CH} ORDER BY K FETCH FIRST 9 ROWS ONLY",
        "SELECT K, V + 1 FROM {CH} ORDER BY K OFFSET 3 ROWS FETCH FIRST 4 ROWS ONLY",
        "SELECT V, V + 1 FROM {CH} ORDER BY V FETCH FIRST 3 ROWS WITH TIES",
        "SELECT K, V + 1 FROM {CH} ORDER BY K FETCH FIRST 10 PERCENT ROWS ONLY",
        "SELECT K, V + 1, 100 / (V + 1) FROM {CH} ORDER BY K FETCH FIRST 3 ROWS ONLY",
        "SELECT K, V + 1, 10 / V FROM {CH} ORDER BY K DESC FETCH FIRST 1 ROW ONLY",
        "SELECT V, V1, V2, COUNT(*) AS C FROM (SELECT V, V - 1 AS V1, V * 2 AS V2 FROM {CH}) T GROUP BY V, V1, V2 \
         ORDER BY C DESC, V FETCH FIRST 3 ROWS ONLY",
    ] {
        alike(&s, COMPUTE_AFTER_FETCH_FIRST, sql);
    }
}

/// Verifies: [DESIGN] RULES.md D1306, D1305: below its FETCH FIRST, the
/// plan of a grouping by a column and expressions of it is the plan of
/// the query that counts by the column alone, optimized and physical, so
/// the grouping's first groups are kept as that query keeps them.
#[test]
fn the_plan_below_the_fetch_first_is_the_one_of_the_columns_alone() {
    let (_db, s) = with_keys();
    let derived = "SELECT V, V1, V2, COUNT(*) AS C FROM (SELECT V, V - 1 AS V1, V * 2 AS V2 FROM C_CH) T GROUP BY V, V1, V2 \
                   ORDER BY C DESC, V FETCH FIRST 3 ROWS ONLY";
    let alone = "SELECT V, COUNT(*) AS C FROM C_CH GROUP BY V ORDER BY C DESC, V FETCH FIRST 3 ROWS ONLY";
    assert!(rewritten(&s, COMPUTE_KEYS_AFTER_GROUPING, derived) && rewritten(&s, COMPUTE_AFTER_FETCH_FIRST, derived));
    for part in ["-- optimized:", "-- physical:"] {
        let below = from_fetch(&s, derived, part);
        assert!(below.len() > 2, "{}", plan(&s, derived));
        assert_eq!(below, from_fetch(&s, alone, part), "{}", plan(&s, derived));
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17]; [DESIGN] RULES.md D1306, D15:
/// the rule leaves a select list sorted by one of its expressions; one
/// whose only expression may raise; one of column references alone; one
/// under FETCH FIRST with no sort; and one sorted with no FETCH FIRST.
#[test]
fn a_select_list_that_may_not_be_computed_late_is_left() {
    let (_db, s) = with_keys();
    for sql in [
        "SELECT K, V + 1 AS W, P + 1 FROM {CH} ORDER BY W, K FETCH FIRST 3 ROWS ONLY",
        "SELECT K, 10 / (V + 1) FROM {CH} ORDER BY K FETCH FIRST 3 ROWS ONLY",
        "SELECT K, V FROM {CH} ORDER BY K FETCH FIRST 3 ROWS ONLY",
        "SELECT K, V + 1 FROM {CH} FETCH FIRST 100 ROWS ONLY",
        "SELECT K, V + 1 FROM {CH} ORDER BY K",
    ] {
        left(&s, COMPUTE_AFTER_FETCH_FIRST, sql);
    }
}
