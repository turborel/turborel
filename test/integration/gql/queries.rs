//! GQL's clauses over graph BANK (Mode A).

use super::{bank, direct, gql};

/// Verifies: [EXTENSION] graph standard §9 R9.1: MATCH of a fixed path
/// and RETURN of properties, ordered.
#[test]
fn match_and_return_9_1() {
    let (_db, s) = bank();
    let got = gql(&s, "MATCH (a:ACCOUNT)-[t:TRANSFER]->(b:ACCOUNT) RETURN a.owner AS src, b.owner AS dst, t.amount AS amt ORDER BY amt");
    assert_eq!(got, ["Cy,Ann,25.00", "Bob,Cy,50.00", "Cy,Dee,75.00", "Ann,Bob,100.00"]);
}

/// Verifies: [EXTENSION] graph standard §9: GQL_TABLE is a table of SQL.
#[test]
fn gql_table_is_a_table_9() {
    let (_db, s) = bank();
    let got = direct(&s, "SELECT G.O FROM GQL_TABLE(BANK, 'MATCH (a:ACCOUNT) WHERE a.is_blocked RETURN a.owner AS o') AS G");
    assert_eq!(got, ["Cy"]);
}

/// Verifies: [EXTENSION] graph standard §9 [P2 §5.1]: OPTIONAL MATCH
/// keeps each binding the pattern matches none of, its new variables
/// null.
#[test]
fn optional_match_keeps_the_unmatched_9() {
    let (_db, s) = bank();
    let got = gql(&s, "MATCH (a:ACCOUNT) OPTIONAL MATCH (a)-[:IS_LOCATED_IN]->(c:CITY) RETURN a.owner AS o, c.city_name AS n ORDER BY o");
    assert_eq!(got, ["Ann,Oslo", "Bob,Rome", "Cy,Oslo", "Dee,NULL"]);
}

/// Verifies: [EXTENSION] graph standard §9: LET adds a value, FILTER
/// keeps the rows its condition is true of, and a label predicate tests
/// an element's labels.
#[test]
fn let_and_filter_9() {
    let (_db, s) = bank();
    let got = gql(&s, "MATCH (a) FILTER a:ACCOUNT LET n = a.owner || '!' FILTER a.owner <> 'Ann' RETURN n ORDER BY n");
    assert_eq!(got, ["Bob!", "Cy!", "Dee!"]);
    let labeled = gql(&s, "MATCH (x) WHERE x IS LABELED PLACE RETURN COUNT(*) AS n");
    assert_eq!(labeled, ["2"]);
}

/// Verifies: [EXTENSION] graph standard §9: FOR binds each element of a
/// list, WITH ORDINALITY from 1 and WITH OFFSET from 0.
#[test]
fn for_each_element_9() {
    let (_db, s) = bank();
    assert_eq!(gql(&s, "FOR x IN [10, 20] WITH ORDINALITY i RETURN x, i"), ["10,1", "20,2"]);
    assert_eq!(gql(&s, "FOR x IN [10, 20] WITH OFFSET i RETURN x, i ORDER BY i DESC"), ["20,1", "10,0"]);
}

/// Verifies: [EXTENSION] graph standard §9: RETURN groups by its items
/// that call no aggregate, with GROUP BY or without, and DISTINCT, ORDER
/// BY, OFFSET and LIMIT page the result.
#[test]
fn return_groups_and_pages_9() {
    let (_db, s) = bank();
    let got =
        gql(&s, "MATCH (a:ACCOUNT)-[t:TRANSFER]->() RETURN a.owner AS o, COUNT(*) AS n, SUM(t.amount) AS total GROUP BY o ORDER BY o");
    assert_eq!(got, ["Ann,1,100.00", "Bob,1,50.00", "Cy,2,100.00"]);
    let distinct = gql(&s, "MATCH (a:ACCOUNT)-[:TRANSFER]->() RETURN DISTINCT a.owner AS o ORDER BY o OFFSET 1 LIMIT 1");
    assert_eq!(distinct, ["Bob"]);
    let paged = gql(&s, "MATCH (a:ACCOUNT) ORDER BY a.owner DESC LIMIT 2 RETURN a.owner AS o ORDER BY o");
    assert_eq!(paged, ["Cy", "Dee"]);
}

/// Verifies: [EXTENSION] graph standard §9, §6: a quantified edge, its
/// group variable counted, a path's length, and the restrictors TRAIL
/// and ACYCLIC.
#[test]
fn quantified_paths_and_restrictors_6() {
    let (_db, s) = bank();
    let walk = gql(&s, "MATCH (a:ACCOUNT WHERE a.owner = 'Ann')-[e:TRANSFER]->{1,3}(b) RETURN b.owner AS o, COUNT(e) AS n ORDER BY n, o");
    assert_eq!(walk, ["Bob,1", "Cy,2", "Ann,3", "Dee,3"]);
    let trail =
        gql(&s, "MATCH p = TRAIL (a:ACCOUNT {owner: 'Ann'})-[:TRANSFER]->+(b) RETURN b.owner AS o, PATH_LENGTH(p) AS n ORDER BY n, o");
    assert_eq!(trail, ["Bob,1", "Cy,2", "Ann,3", "Dee,3"]);
    let acyclic = gql(&s, "MATCH ACYCLIC (a:ACCOUNT {owner: 'Ann'})-[:TRANSFER]->+(b) RETURN b.owner AS o ORDER BY o");
    assert_eq!(acyclic, ["Bob", "Cy", "Dee"]);
}

/// Verifies: [EXTENSION] graph standard §6: the selectors keep, of each
/// pair of endpoints, the shortest paths, and `*` matches the path of no
/// edge.
#[test]
fn selectors_keep_the_shortest_6() {
    let (_db, s) = bank();
    let got =
        gql(&s, "MATCH p = ANY SHORTEST (a:ACCOUNT {owner: 'Ann'})-[:TRANSFER]->+(b:ACCOUNT {owner: 'Dee'}) RETURN PATH_LENGTH(p) AS n");
    assert_eq!(got, ["3"]);
    let all = gql(&s, "MATCH ALL SHORTEST (a:ACCOUNT {owner: 'Bob'})-[:TRANSFER]->*(b:ACCOUNT) RETURN b.owner AS o ORDER BY o");
    assert_eq!(all, ["Ann", "Bob", "Cy", "Dee"]);
    let none = gql(&s, "MATCH ALL SHORTEST (a:ACCOUNT {owner: 'Dee'})-[:TRANSFER]->*(b:ACCOUNT) RETURN b.owner AS o");
    assert_eq!(none, ["Dee"], "a path of no edge");
}

/// Verifies: [EXTENSION] graph standard §9 [P2 §5.1]: the set operators
/// combine statements by column name, and OTHERWISE gives the right's
/// rows only if the left has none.
#[test]
fn set_operators_and_otherwise_9() {
    let (_db, s) = bank();
    let union = gql(&s, "MATCH (a:ACCOUNT) RETURN a.owner AS o UNION MATCH (c:CITY) RETURN c.city_name AS o");
    assert_eq!(union.len(), 6);
    let except = gql(&s, "MATCH (a:ACCOUNT) RETURN a.owner AS o EXCEPT MATCH (a:ACCOUNT)-[:TRANSFER]->() RETURN a.owner AS o");
    assert_eq!(except, ["Dee"]);
    let otherwise = gql(&s, "MATCH (a:ACCOUNT {owner: 'Zed'}) RETURN a.owner AS o OTHERWISE RETURN 'none' AS o");
    assert_eq!(otherwise, ["none"]);
    let kept = gql(&s, "MATCH (a:ACCOUNT {owner: 'Ann'}) RETURN a.owner AS o OTHERWISE RETURN 'none' AS o");
    assert_eq!(kept, ["Ann"]);
}

/// Verifies: [EXTENSION] graph standard §9 [P2 §5.1]: NEXT feeds one
/// statement's table to the next, and an inline CALL is joined with the
/// table on the variables its scope imports.
#[test]
fn next_and_call_9() {
    let (_db, s) = bank();
    let next = gql(&s, "MATCH (a:ACCOUNT) RETURN a.owner AS o NEXT FILTER o <> 'Ann' RETURN COUNT(*) AS n");
    assert_eq!(next, ["3"]);
    let call = gql(&s, "MATCH (a:ACCOUNT) CALL (a) { MATCH (a)-[t:TRANSFER]->() RETURN COUNT(*) AS n } RETURN a.owner AS o, n ORDER BY o");
    assert_eq!(call, ["Ann,1", "Bob,1", "Cy,2"]);
    let optional = gql(
        &s,
        "MATCH (a:ACCOUNT) OPTIONAL CALL (a) { MATCH (a)-[:IS_LOCATED_IN]->(c) RETURN c.city_name AS n } RETURN a.owner AS o, n ORDER BY o",
    );
    assert_eq!(optional, ["Ann,Oslo", "Bob,Rome", "Cy,Oslo", "Dee,NULL"]);
}

/// Verifies: [EXTENSION] graph standard §5, §22: an element's identity
/// and labels, and the conditions of an unknown label or property
/// (GRF-0012).
#[test]
fn identities_labels_and_unknowns_5() {
    let (_db, s) = bank();
    assert_eq!(
        gql(&s, "MATCH (c:CITY {city_name: 'Rome'}) RETURN ELEMENT_ID(c) AS i, LABELS(c) AS l"),
        [r#"{"table":"CITY","key":[2]},|CITY|PLACE|"#]
    );
    let (code, msg) = super::failed(&s, "EXECUTE GQL ON GRAPH BANK 'MATCH (x:NOPE) RETURN x'");
    assert_eq!(code, -103);
    assert!(msg.contains("GRF-0012"), "{msg}");
    let (code, msg) = super::failed(&s, "EXECUTE GQL ON GRAPH BANK 'MATCH (c:CITY) RETURN c.owner'");
    assert_eq!(code, -103);
    assert!(msg.contains("GRF-0012"), "{msg}");
}

/// Verifies: [EXTENSION] graph standard §9: a GQL session reads each
/// direct statement as GQL, of the graph SESSION SET GRAPH names, until
/// SET takes SQL back.
#[test]
fn a_gql_session_9() {
    let (_db, s) = bank();
    s.set_setting("SESSION.LANGUAGE", &turborel::value::Value::Char("GQL".into())).unwrap();
    direct(&s, "SESSION SET GRAPH BANK");
    assert_eq!(direct(&s, "MATCH (c:CITY) RETURN c.city_name AS n ORDER BY n"), ["Oslo", "Rome"]);
    assert!(s.direct("START TRANSACTION READ ONLY").is_err(), "a transaction is active");
    direct(&s, "COMMIT");
    direct(&s, "START TRANSACTION READ ONLY");
    assert_eq!(direct(&s, "MATCH (c:CITY {city_name: 'Oslo'}) RETURN c.city_name AS n"), ["Oslo"]);
    direct(&s, "COMMIT");
    direct(&s, "SESSION RESET GRAPH");
    let (code, msg) = super::failed(&s, "MATCH (c:CITY) RETURN c");
    assert_eq!(code, -103, "{msg}");
    direct(&s, "SET SESSION.LANGUAGE = 'SQL'");
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM CITY"), ["2"]);
}
