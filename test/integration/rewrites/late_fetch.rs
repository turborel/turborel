//! The columns a grouping carries of a table joined on its key, fetched
//! after FETCH FIRST (RULES.md D1202, `optimizer/late_fetch.rs`).

use turborel::optimizer::FETCH_DEPENDENTS_LATE;

use super::dependent_keys::with_keys;
use super::{alike, left, plan};

/// Verifies: [ISO/IEC 9075-2:2023 §4.26.1, §6.7 GR1e, §7.17 GR6, GR7];
/// [DESIGN] RULES.md D1202: a query that keeps the first rows of its
/// groups gives the same rows, in the same order, with the columns its
/// grouping carries of a table read again by its key for the rows kept,
/// of either store: columns the grouping columns' key determines, written
/// in GROUP BY or not; with no ORDER BY; of a table whose key is no
/// grouping column but equal to one; with one column a sort key reads,
/// which stays carried, and another fetched; with an expression of the
/// select list reading one; under OFFSET; under a HAVING condition of a
/// set function; of two tables, one reached through the other; and of
/// the grouped table itself.
#[test]
fn carried_columns_are_fetched_for_the_rows_kept() {
    let (_db, s) = with_keys();
    for sql in [
        "SELECT P.K, P.N, P.A, SUM(C.V) AS T FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY P.K, P.N, P.A \
         ORDER BY T DESC, P.K FETCH FIRST 3 ROWS ONLY",
        "SELECT P.K, P.N, COUNT(*) AS T FROM {PA} P JOIN {CH} C ON C.P = P.K GROUP BY P.K ORDER BY T DESC, P.K FETCH FIRST 2 ROWS ONLY",
        "SELECT P.K, P.N, COUNT(*) FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY P.K, P.N FETCH FIRST 100 ROWS ONLY",
        "SELECT C.P, P.N, COUNT(*) FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY C.P, P.N ORDER BY 3 DESC, 1 FETCH FIRST 4 ROWS ONLY",
        "SELECT P.K, P.N, P.A, COUNT(*) FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY P.K, P.N, P.A ORDER BY P.A, P.K \
         FETCH FIRST 5 ROWS ONLY",
        "SELECT P.K, P.A + 1, P.N FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY P.K, P.A, P.N ORDER BY 1 FETCH FIRST 3 ROWS ONLY",
        "SELECT P.K, P.N, MAX(C.V) FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY P.K, P.N ORDER BY 1 DESC \
         OFFSET 2 ROWS FETCH FIRST 3 ROWS ONLY",
        "SELECT P.K, P.N, COUNT(*) FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY P.K, P.N HAVING COUNT(*) > 3 ORDER BY 1 \
         FETCH FIRST 3 ROWS ONLY",
        "SELECT P.K, P.N, T.L, COUNT(*) FROM {PA} P, {CH} C, {TP} T WHERE C.P = P.K AND P.A = T.K GROUP BY P.K, P.N, T.L \
         ORDER BY 4 DESC, 1 FETCH FIRST 3 ROWS ONLY",
        "SELECT C.K, C.S, COUNT(*) FROM {CH} C GROUP BY C.K, C.S ORDER BY 1 FETCH FIRST 3 ROWS ONLY",
    ] {
        alike(&s, FETCH_DEPENDENTS_LATE, sql);
    }
    let shown = plan(
        &s,
        "SELECT P.K, P.N, SUM(C.V) AS T FROM PA P, CH C WHERE C.P = P.K GROUP BY P.K, P.N ORDER BY T DESC, P.K FETCH FIRST 3 ROWS ONLY",
    );
    let optimized = shown.split("-- optimized:").nth(1).and_then(|rest| rest.split("-- physical:").next()).unwrap_or_default();
    assert!(optimized.trim_start().starts_with("Sort"), "sorted again above the fetch: {shown}");
    assert_eq!(optimized.matches("Scan Q.PA").count(), 2, "joined below, and read again above: {shown}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR7]; [DESIGN] RULES.md D1202,
/// D15: the rule leaves a query whose carried columns every group needs,
/// or whose fetch could change a row: one of no FETCH FIRST; of WITH TIES
/// and of PERCENT, whose rows kept are not a constant count; whose every
/// carried column a sort key or a HAVING condition reads; of a table an
/// outer join extends with nulls, which has no row for some groups; of a
/// table whose key the grouping columns do not determine; and of as many
/// rows kept as the grouping groups.
#[test]
fn carried_columns_every_group_needs_stay_carried() {
    let (_db, s) = with_keys();
    for sql in [
        "SELECT P.K, P.N, COUNT(*) FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY P.K, P.N ORDER BY 1",
        "SELECT P.K, P.N, COUNT(*) FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY P.K, P.N ORDER BY 3 DESC FETCH FIRST 2 ROWS WITH TIES",
        "SELECT P.K, P.N, COUNT(*) FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY P.K, P.N ORDER BY 1 FETCH FIRST 50 PERCENT ROWS ONLY",
        "SELECT P.K, P.N, COUNT(*) FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY P.K, P.N ORDER BY P.N, P.K FETCH FIRST 3 ROWS ONLY",
        "SELECT P.K, P.N, COUNT(*) FROM {PA} P, {CH} C WHERE C.P = P.K GROUP BY P.K HAVING P.N <> 'n1' ORDER BY 1 \
         FETCH FIRST 3 ROWS ONLY",
        "SELECT C.K, P.N, COUNT(*) FROM {CH} C LEFT JOIN {PA} P ON C.P = P.K GROUP BY C.K, P.N ORDER BY 1 FETCH FIRST 5 ROWS ONLY",
        "SELECT T.K, P.A, COUNT(*) FROM {TP} T, {PA} P WHERE T.K = P.A GROUP BY T.K, P.A ORDER BY 1 FETCH FIRST 1 ROWS ONLY",
        "SELECT C.K, C.S, COUNT(*) FROM {CH} C GROUP BY C.K, C.S ORDER BY 1 FETCH FIRST 50 ROWS ONLY",
    ] {
        left(&s, FETCH_DEPENDENTS_LATE, sql);
    }
}
