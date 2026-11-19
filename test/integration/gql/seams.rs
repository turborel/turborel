//! What a GQL MATCH shares with GRAPH_TABLE (RULES.md D144): CHEAPEST
//! and the path functions, restrictors over whole paths, group
//! variables' aggregates, a graph's definer's rights, and the identities
//! a data-modifying statement reads and draws.

use super::{bank, direct, failed, gql};
use turborel::db::Session;

/// The rows of GQL `text` on SYS_GRAPH.
fn sys(s: &Session, text: &str) -> Vec<String> {
    direct(s, &format!("EXECUTE GQL ON GRAPH SYS_GRAPH '{}'", text.replace('\'', "''")))
}

/// Verifies: [EXTENSION] graph standard §6.9, [GS §7.2]: ANY CHEAPEST
/// keeps the path of least COST, and each path function reads the path
/// a MATCH binds.
#[test]
fn cheapest_and_path_functions_6() {
    let (_db, s) = bank();
    let got = gql(
        &s,
        "MATCH ANY CHEAPEST COST t.amount p = (a:ACCOUNT {owner: 'Ann'})-[t:TRANSFER]->+(b:ACCOUNT {owner: 'Dee'}) \
         RETURN PATH_LENGTH(p) AS n, IS_ACYCLIC(p) AS a, IS_TRAIL(p) AS t, CARDINALITY(VERTICES(p)) AS v",
    );
    assert_eq!(got, ["3,TRUE,TRUE,4"]);
    let cyclic = gql(&s, "MATCH TRAIL p = (a:ACCOUNT {owner: 'Ann'})-[:TRANSFER]->{3}(a) RETURN IS_ACYCLIC(p) AS a, IS_SIMPLE(p) AS s");
    assert_eq!(cyclic, ["FALSE,TRUE"]);
}

/// Verifies: [EXTENSION] graph standard §6, [GS §6.4]: a restrictor holds
/// of the whole path, its fixed edges among the quantified ones'.
#[test]
fn restrictors_hold_of_the_whole_path_6() {
    let (_db, s) = bank();
    let count = |r: &str| gql(&s, &format!("MATCH {r} (a {{owner: 'Ann'}})-[x:TRANSFER]->(m)-[:TRANSFER]->+(b) RETURN COUNT(*) AS n"));
    assert_eq!(count("TRAIL"), ["3"]);
    assert_eq!(count("ACYCLIC"), ["2"]);
}

/// Verifies: [EXTENSION] graph standard §6, [GS §6.5]: an aggregate of a
/// group variable's property reads that row's group, and COUNT its
/// elements.
#[test]
fn group_variables_aggregate_6() {
    let (_db, s) = bank();
    let got = gql(
        &s,
        "MATCH (a:ACCOUNT {owner: 'Ann'})-[t:TRANSFER]->{2}(b) RETURN b.owner AS o, SUM(t.amount) AS s, COUNT(t) AS n, MAX(t.amount) AS m",
    );
    assert_eq!(got, ["Cy,150.00,2,100.00"]);
}

/// Verifies: [EXTENSION] graph standard §20, [GS §5]: a graph of definer's
/// rights is read by GQL_TABLE with SELECT on the graph alone.
#[test]
fn definer_rights_reach_gql_20() {
    let (db, s) = bank();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    s.run("GRANT SELECT ON PROPERTY GRAPH BANK TO B").unwrap();
    s.commit().unwrap();
    let b = db.session("B");
    let q = "SELECT G.O FROM GQL_TABLE(A.BANK, 'MATCH (a:ACCOUNT) WHERE a.is_blocked RETURN a.owner AS o') AS G";
    assert_eq!(direct(&b, q), ["Cy"]);
    let (code, _) = failed(&b, "SELECT OWNER FROM A.ACCOUNT");
    assert!(code < 0);
}

/// Verifies: [EXTENSION] graph standard §4.2: an INSERT of two nodes of
/// equal values takes each one's identity from its own row, so the edge
/// between them joins two nodes.
#[test]
fn equal_nodes_keep_their_own_identities_4_2() {
    let (_db, s) = super::dml::native();
    sys(&s, "INSERT (:PERSON {name: 'Twin', age: 1})-[:KNOWS {since: 1}]->(:PERSON {name: 'Twin', age: 1})");
    let got = sys(&s, "MATCH (a:PERSON)-[:KNOWS]->(b:PERSON) RETURN ELEMENT_ID(a) = ELEMENT_ID(b) AS same");
    assert_eq!(got, ["FALSE"]);
    sys(&s, "INSERT (x:PERSON {name: 'Twin', age: 1}) INSERT (x)-[:KNOWS {since: 2}]->(y:PERSON {name: 'Twin', age: 1})");
    assert_eq!(sys(&s, "MATCH (a)-[k:KNOWS]->(b) WHERE ELEMENT_ID(a) = ELEMENT_ID(b) RETURN COUNT(*) AS n"), ["0"]);
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM PERSON"), ["4"]);
}

/// Verifies: [EXTENSION] graph standard §12, [P2 §5.1]: an inline CALL
/// whose procedure modifies data runs once for each row of the binding
/// table, rows of equal values each their own.
#[test]
fn a_modifying_call_runs_per_row_12() {
    let (_db, s) = super::dml::native();
    sys(&s, "INSERT (r:CITY {city_name: 'Rome'}) INSERT (:PERSON {name: 'Ann'})-[:LIVES_IN]->(r)<-[:LIVES_IN]-(:PERSON {name: 'Bob'})");
    let got = sys(&s, "MATCH (:PERSON)-[:LIVES_IN]->(c:CITY) CALL (c) { INSERT (:CITY {city_name: 'copy'}) } RETURN COUNT(*) AS n");
    assert_eq!(got, ["2"]);
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM CITY WHERE CITY_NAME = 'copy'"), ["2"]);
    let returned = sys(
        &s,
        "MATCH (p:PERSON)-[:LIVES_IN]->(c:CITY) CALL (c) { INSERT (:CITY {city_name: 'more'}) RETURN 1 AS k } RETURN p.name AS p, k ORDER BY p",
    );
    assert_eq!(returned, ["Ann,1", "Bob,1"]);
}
