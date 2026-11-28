//! Grouping columns that are expressions of other grouping columns,
//! computed after the grouping (RULES.md D1305,
//! `optimizer/dependent_keys/computed.rs`).

use turborel::optimizer::{COMPUTE_KEYS_AFTER_GROUPING, REDUCE_GROUPING_KEYS};

use super::dependent_keys::with_keys;
use super::{alike, left, plan, rewritten};

/// The part of the plan EXPLAIN shows of `sql` from its heading `from`.
fn shown_from(s: &turborel::db::Session, sql: &str, from: &str) -> String {
    plan(s, sql).split(from).nth(1).unwrap_or_default().to_owned()
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.12, §4.26.1]; [DESIGN] RULES.md
/// D1305: a grouping by columns of a derived table, some of them
/// expressions of the others that cannot raise, gives the same rows
/// grouped by the others alone, of either store, by batches and row by
/// row: differences and products of one column, the expression written
/// first; a sum of two columns, one of them null in some rows, whose
/// group has the null the expression gives of it; a CASE; under ORDER BY
/// of the column computed with FETCH FIRST; under a HAVING condition
/// that reads it and a set function; with set functions that read it,
/// filtered, alone and beside a DISTINCT one; under a window over the groups; under a select
/// list of expressions of it; through a derived table of a derived
/// table; and with a column a key decides, which the rule of D1201 then
/// carries.
#[test]
fn a_grouping_column_computed_of_the_others_is_computed_after_the_grouping() {
    let (_db, s) = with_keys();
    for sql in [
        "SELECT V, V1, V2, COUNT(*) FROM (SELECT V, V - 1 AS V1, V * 2 AS V2 FROM {CH}) T GROUP BY V, V1, V2",
        "SELECT V1, V, COUNT(*) FROM (SELECT V - 1 AS V1, V FROM {CH}) T GROUP BY V1, V",
        "SELECT V, P, VP, COUNT(*) FROM (SELECT V, P, V + P AS VP FROM {CH}) T GROUP BY V, P, VP",
        "SELECT P, B, MAX(V) FROM (SELECT P, V, CASE WHEN P > 6 THEN 'high' WHEN P IS NULL THEN 'none' ELSE 'low' END AS B FROM {CH}) T \
         GROUP BY P, B",
        "SELECT P, P1, COUNT(*) AS C FROM (SELECT P, P + 1 AS P1 FROM {CH}) T GROUP BY P, P1 ORDER BY P1 DESC, C FETCH FIRST 4 ROWS ONLY",
        "SELECT P, P1, COUNT(*) FROM (SELECT P, P + 1 AS P1 FROM {CH}) T GROUP BY P, P1 HAVING COUNT(*) > 3 AND P1 < 9",
        "SELECT V, V1, SUM(V1), COUNT(DISTINCT S), COUNT(*) FILTER (WHERE V1 > 3) FROM (SELECT V, V + 1 AS V1, S FROM {CH}) T GROUP BY V, V1",
        "SELECT V, V1, MAX(V1), COUNT(*) FILTER (WHERE V1 > 3) FROM (SELECT V, V + 1 AS V1, S FROM {CH}) T GROUP BY V, V1",
        "SELECT V, V1, COUNT(*), RANK() OVER (ORDER BY V1 DESC) FROM (SELECT V, V + 1 AS V1 FROM {CH}) T GROUP BY V, V1 ORDER BY V",
        "SELECT V1 * 2, COUNT(*) + V1, V1 FROM (SELECT V, V + 1 AS V1 FROM {CH}) T GROUP BY V, V1",
        "SELECT V, V1, COUNT(*) FROM (SELECT V, V1 FROM (SELECT S, V, V + 1 AS V1 FROM {CH}) U) T GROUP BY V, V1",
        "SELECT K, N, K1, COUNT(*) FROM (SELECT K, N, K + 1 AS K1 FROM {PA}) T GROUP BY K, N, K1",
    ] {
        alike(&s, COMPUTE_KEYS_AFTER_GROUPING, sql);
    }
    assert!(rewritten(&s, REDUCE_GROUPING_KEYS, "SELECT K, N, K1, COUNT(*) FROM (SELECT K, N, K + 1 AS K1 FROM PA) T GROUP BY K, N, K1"));
}

/// Verifies: [DESIGN] RULES.md D1305: the plan of a grouping by a column
/// and expressions of it is the plan of the query that groups by the
/// column and writes the expressions in its select list, with HAVING and
/// without: one grouping column, no projection below the grouping, the
/// HAVING condition next to it, and one projection above.
#[test]
fn the_plan_is_the_one_of_the_query_that_groups_by_the_columns_kept() {
    let (_db, s) = with_keys();
    for (derived, written) in [
        (
            "SELECT V, V1, V2, COUNT(*) FROM (SELECT V, V - 1 AS V1, V * 2 AS V2 FROM CH) T GROUP BY V, V1, V2",
            "SELECT V, V - 1, V * 2, COUNT(*) FROM CH GROUP BY V",
        ),
        (
            "SELECT V, V1, COUNT(*) FROM (SELECT V, V - 1 AS V1 FROM C_CH) T GROUP BY V, V1 HAVING COUNT(*) > 8 \
             ORDER BY 3 DESC, 1 FETCH FIRST 2 ROWS ONLY",
            "SELECT V, V - 1, COUNT(*) FROM C_CH GROUP BY V HAVING COUNT(*) > 8 ORDER BY 3 DESC, 1 FETCH FIRST 2 ROWS ONLY",
        ),
    ] {
        for part in ["-- optimized:", "-- physical:"] {
            assert_eq!(shown_from(&s, derived, part), shown_from(&s, written, part), "{derived}");
        }
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.12, §7.13 GR2a]; [DESIGN] RULES.md
/// D1305: a grouping column that is a constant is computed after the
/// grouping, beside a column of any type, which nothing reads of it, and
/// beside one grouped by a collation, which is still grouped by it; of
/// constants alone the first is still grouped by, so a table of no rows
/// gives no group; and an expression found through a join, a semi join
/// and the preserved operand of an outer join is computed after the
/// grouping, the groups of rows extended with nulls among them.
#[test]
fn constants_and_expressions_found_through_joins_are_computed_after_the_grouping() {
    let (_db, s) = with_keys();
    for sql in [
        "SELECT ONE, S, COUNT(*) FROM (SELECT 1 AS ONE, S FROM {CH}) T GROUP BY ONE, S",
        "SELECT MIN(S), ONE, COUNT(*) FROM (SELECT S, 'x' AS ONE FROM {SK}) T GROUP BY S COLLATE CASE_INSENSITIVE, ONE",
        "SELECT ONE, TWO, COUNT(*) FROM (SELECT 1 AS ONE, 2 AS TWO FROM {CH}) T GROUP BY ONE, TWO",
        "SELECT ONE, TWO, COUNT(*) FROM (SELECT 1 AS ONE, 2 AS TWO FROM {CH} WHERE V > 100) T GROUP BY ONE, TWO",
        "SELECT T.V, T.V1, P.N, COUNT(*) FROM (SELECT P, V, V + 1 AS V1 FROM {CH}) T, {PA} P WHERE T.P = P.K GROUP BY T.V, T.V1, P.N",
        "SELECT V, V1, COUNT(*) FROM (SELECT P, V, V + 1 AS V1 FROM {CH}) T WHERE EXISTS (SELECT * FROM {PA} P WHERE P.K = T.P) \
         GROUP BY V, V1",
        "SELECT T.V, T.V1, P.A, COUNT(*) FROM (SELECT P, V, V + 1 AS V1 FROM {CH}) T LEFT JOIN {PA} P ON T.P = P.K GROUP BY T.V, T.V1, P.A",
        "SELECT T.V, T.V1, P.A, COUNT(*) FROM {PA} P RIGHT JOIN (SELECT P, V, V + 1 AS V1 FROM {CH}) T ON T.P = P.K GROUP BY T.V, T.V1, P.A",
    ] {
        alike(&s, COMPUTE_KEYS_AFTER_GROUPING, sql);
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.12, §4.26.1, §7.13 GR2a]; [DESIGN]
/// RULES.md D1305, D15: the rule leaves a grouping column that is an
/// expression which may raise, a quotient by a column that raises of a
/// zero row with the rule on and off, and a CAST that narrows; one of a
/// character string, of which two of one group may differ; one of an
/// approximate number; one that reads a column not grouped by, or the
/// column of that number of another table, which is grouped by; one of a
/// column that is itself an expression; the only grouping column; a
/// select list and a condition over a select list of a grouping that
/// may raise, a quotient by a zero group, which stay where they are;
/// one of the operand an outer join extends with nulls, of either side
/// and of a FULL join, whose rows extended have a null where the
/// expression of nulls has none; and the columns of ROLLUP, CUBE and
/// GROUPING SETS, with GROUPING of them.
#[test]
fn a_grouping_column_that_may_not_be_computed_of_the_others_is_left() {
    let (_db, s) = with_keys();
    let nulled = "(SELECT P, V, CASE WHEN V IS NULL THEN 0 ELSE 1 END AS V1 FROM {CH}) T";
    let on = "ON T.P = P.K AND T.V > 5";
    for sql in [
        "SELECT V, Q, COUNT(*) FROM (SELECT V, V / P AS Q FROM {CH}) T GROUP BY V, Q".to_owned(),
        "SELECT V, Q, COUNT(*) FROM (SELECT V, V / P AS Q FROM {CH} WHERE P > 0) T GROUP BY V, Q".to_owned(),
        "SELECT V, W, COUNT(*) FROM (SELECT V, CAST(V AS SMALLINT) AS W FROM {CH}) T GROUP BY V, W".to_owned(),
        "SELECT K, L, COUNT(*), MIN(S) FROM (SELECT K, S, CASE WHEN S = 'a' THEN 1 ELSE 0 END AS L FROM {WS}) T GROUP BY S, K, L"
            .to_owned(),
        "SELECT Y, B, COUNT(*) FROM (SELECT X, Y, CASE WHEN X > 1 THEN 1 ELSE 0 END AS B FROM {FL}) T GROUP BY X, Y, B".to_owned(),
        "SELECT V, VP, COUNT(*) FROM (SELECT V, V + P AS VP FROM {CH}) T GROUP BY V, VP".to_owned(),
        "SELECT V1, V2, COUNT(*) FROM (SELECT V1, V1 + 1 AS V2 FROM (SELECT V + 1 AS V1 FROM {CH}) U) T GROUP BY V1, V2".to_owned(),
        "SELECT V1, COUNT(*) FROM (SELECT V, V + 1 AS V1 FROM {CH}) T GROUP BY V1".to_owned(),
        "SELECT T.V1, U.A, COUNT(*) FROM (SELECT P, V + 1 AS V1 FROM {CH}) T, (SELECT K, N, A FROM {PA}) U WHERE T.P = U.K GROUP BY T.V1, U.A"
            .to_owned(),
        "SELECT V FROM (SELECT V, 10 / V AS Q, COUNT(*) AS C FROM {CH} GROUP BY V) T".to_owned(),
        "SELECT V, Q FROM (SELECT V, 10 / V AS Q, COUNT(*) AS C FROM {CH} GROUP BY V) T WHERE C > 100".to_owned(),
        format!("SELECT T.V, T.V1, COUNT(*) FROM {{PA}} P LEFT JOIN {nulled} {on} GROUP BY T.V, T.V1"),
        format!("SELECT T.V, T.V1, COUNT(*) FROM {nulled} RIGHT JOIN {{PA}} P {on} GROUP BY T.V, T.V1"),
        format!("SELECT T.V, T.V1, COUNT(*) FROM {nulled} FULL JOIN {{PA}} P {on} GROUP BY T.V, T.V1"),
        "SELECT V, V1, GROUPING(V1), COUNT(*) FROM (SELECT V, V + 1 AS V1 FROM {CH}) T GROUP BY ROLLUP (V, V1)".to_owned(),
        "SELECT V, V1, GROUPING(V, V1), COUNT(*) FROM (SELECT V, V + 1 AS V1 FROM {CH}) T GROUP BY CUBE (V, V1)".to_owned(),
        "SELECT V, V1, GROUPING(V1), COUNT(*) FROM (SELECT V, V + 1 AS V1 FROM {CH}) T GROUP BY GROUPING SETS ((V, V1), (V))".to_owned(),
    ] {
        left(&s, COMPUTE_KEYS_AFTER_GROUPING, &sql);
    }
}
