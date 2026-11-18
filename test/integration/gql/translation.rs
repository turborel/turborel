//! GQL's lowering checked against SQL written by hand [P2 Theorem 5.1]:
//! each GQL query gives the rows of the relational query its clauses
//! translate to, over graph BANK's tables.

use super::{bank, direct, gql};

/// That GQL `text` and SQL `sql` give the same rows, in any order.
fn same(s: &turborel::db::Session, text: &str, sql: &str) {
    let mut ours = gql(s, text);
    let mut theirs = direct(s, sql);
    ours.sort();
    theirs.sort();
    assert_eq!(ours, theirs, "{text}");
    assert!(!ours.is_empty(), "{text} gives no row");
}

/// Verifies: [EXTENSION] [P2 Theorem 5.1]: MATCH is a join of the binding
/// table with the pattern's rows, a fixed path a join of its element
/// tables on their endpoints.
#[test]
fn match_is_a_join_p2_5_1() {
    let (_db, s) = bank();
    same(
        &s,
        "MATCH (a:ACCOUNT)-[t:TRANSFER]->(b:ACCOUNT) WHERE t.amount > 30 RETURN a.owner AS a, b.owner AS b",
        "SELECT X.OWNER, Y.OWNER FROM A.ACCOUNT X JOIN A.TRANSFER T ON T.FROM_ACCT = X.ACCT_ID \
         JOIN A.ACCOUNT Y ON T.TO_ACCT = Y.ACCT_ID WHERE T.AMOUNT > 30",
    );
}

/// Verifies: [EXTENSION] [P2 Theorem 5.1]: OPTIONAL MATCH is a left outer
/// join, and LET and FILTER a projection and a selection.
#[test]
fn optional_let_and_filter_p2_5_1() {
    let (_db, s) = bank();
    same(
        &s,
        "MATCH (a:ACCOUNT) OPTIONAL MATCH (a)-[:IS_LOCATED_IN]->(c:CITY) LET n = a.owner FILTER n <> 'Bob' RETURN n, c.city_name AS c",
        "SELECT X.OWNER, C.CITY_NAME FROM A.ACCOUNT X LEFT JOIN (A.LOCATED_IN L JOIN A.CITY C ON L.CITY_ID = C.CITY_ID) \
         ON L.ACCT_ID = X.ACCT_ID WHERE X.OWNER <> 'Bob'",
    );
}

/// Verifies: [EXTENSION] [P2 Theorem 5.1]: two MATCH clauses sharing a
/// variable are joined on its identity, and RETURN with an aggregate
/// groups by the other items.
#[test]
fn shared_variables_and_grouping_p2_5_1() {
    let (_db, s) = bank();
    same(
        &s,
        "MATCH (a:ACCOUNT)-[:TRANSFER]->(b) MATCH (b)-[:TRANSFER]->(c) RETURN a.owner AS a, COUNT(*) AS n",
        "SELECT X.OWNER, COUNT(*) FROM A.ACCOUNT X JOIN A.TRANSFER T ON T.FROM_ACCT = X.ACCT_ID \
         JOIN A.TRANSFER U ON U.FROM_ACCT = T.TO_ACCT GROUP BY X.OWNER",
    );
}

/// Verifies: [EXTENSION] [P2 Theorem 5.1]: a quantified path is the
/// transitive closure a recursive query computes, TRAIL its walks of
/// distinct edges.
#[test]
fn quantified_paths_are_recursion_p2_5_1() {
    let (_db, s) = bank();
    same(
        &s,
        "MATCH TRAIL (a:ACCOUNT {owner: 'Ann'})-[:TRANSFER]->{1,2}(b) RETURN DISTINCT b.owner AS b",
        "WITH RECURSIVE R (ACCT, N) AS (SELECT TO_ACCT, 1 FROM A.TRANSFER WHERE FROM_ACCT = 1 \
         UNION ALL SELECT T.TO_ACCT, R.N + 1 FROM R JOIN A.TRANSFER T ON T.FROM_ACCT = R.ACCT WHERE R.N < 2) \
         SELECT DISTINCT X.OWNER FROM R JOIN A.ACCOUNT X ON X.ACCT_ID = R.ACCT",
    );
}

/// That GQL_TABLE of GQL `text` and the SQL GRAPH_TABLE `table` give the
/// same rows, in any order.
fn equivalent(s: &turborel::db::Session, text: &str, table: &str) {
    let mut ours = direct(s, &format!("SELECT * FROM GQL_TABLE(BANK, '{}') AS G", text.replace('\'', "''")));
    let mut theirs = direct(s, &format!("SELECT * FROM GRAPH_TABLE (BANK {table}) AS G"));
    ours.sort();
    theirs.sort();
    assert_eq!(ours, theirs, "{text}");
    assert!(!ours.is_empty(), "{text} gives no row");
}

/// Verifies: [EXTENSION] graph standard §9, [GS §7]: GQL_TABLE of a MATCH
/// gives the rows GRAPH_TABLE of its pattern does: of a fixed path, of a
/// quantified one with its identities and group, and of CHEAPEST.
#[test]
fn gql_table_is_graph_table_9() {
    let (_db, s) = bank();
    equivalent(
        &s,
        "MATCH (a:ACCOUNT)-[t:TRANSFER]->(b) WHERE t.amount > 30 RETURN a.owner AS a, t.amount AS m, b.owner AS b",
        "MATCH (a:ACCOUNT)-[t:TRANSFER]->(b) WHERE t.amount > 30 COLUMNS (a.owner AS a, t.amount AS m, b.owner AS b)",
    );
    equivalent(
        &s,
        "MATCH TRAIL (a:ACCOUNT)-[t:TRANSFER]->{1,3}(b) RETURN ELEMENT_ID(a) AS a, ELEMENT_ID(b) AS b, COUNT(t) AS n",
        "MATCH TRAIL (a:ACCOUNT)-[t:TRANSFER]->{1,3}(b) COLUMNS (ELEMENT_ID(a) AS a, ELEMENT_ID(b) AS b, COUNT(t.*) AS n)",
    );
    equivalent(
        &s,
        "MATCH ANY CHEAPEST COST t.amount p = (a:ACCOUNT {owner: 'Bob'})-[t:TRANSFER]->+(b) RETURN b.owner AS b, PATH_LENGTH(p) AS n",
        "MATCH ANY CHEAPEST COST t.amount p = (a:ACCOUNT WHERE a.owner = 'Bob')-[t:TRANSFER]->+(b) COLUMNS (b.owner AS b, PATH_LENGTH(p) AS n)",
    );
}
