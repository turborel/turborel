//! GRAPH_TABLE over SYS_GRAPH, the implicit graph of a schema's node and
//! edge tables (RULES.md D143): its vertices the nodes, keyed by
//! `$NODE_ID`, its edges joined by `$FROM_ID` and `$TO_ID`.

use turborel::db::{Database, Session};

use super::rows;

/// People who know each other and the cities they live in, of schema A:
/// ANN knows BOB, BOB knows CY, and BOB lives in ROME.
fn social() -> Session {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A \
         CREATE TABLE PERSON (NAME VARCHAR(20)) AS NODE \
         CREATE TABLE CITY (CITY_NAME VARCHAR(20)) AS NODE \
         CREATE TABLE KNOWS (SINCE INTEGER) AS EDGE \
         CREATE TABLE LIVES_IN (CONSTRAINT LIVES CONNECTION (PERSON TO CITY)) AS EDGE",
    )
    .unwrap();
    let s = db.session("A");
    s.run("INSERT INTO PERSON VALUES ('ANN'), ('BOB'), ('CY')").unwrap();
    s.run("INSERT INTO CITY VALUES ('ROME')").unwrap();
    let knows = "INSERT INTO KNOWS ($FROM_ID, $TO_ID, SINCE) SELECT F.$NODE_ID, T.$NODE_ID, 2020 FROM PERSON F, PERSON T WHERE ";
    s.run(&format!("{knows} F.NAME = 'ANN' AND T.NAME = 'BOB'")).unwrap();
    s.run(&format!("{knows} F.NAME = 'BOB' AND T.NAME = 'CY'")).unwrap();
    s.run("INSERT INTO LIVES_IN ($FROM_ID, $TO_ID) SELECT P.$NODE_ID, C.$NODE_ID FROM PERSON P, CITY C WHERE P.NAME = 'BOB'").unwrap();
    s
}

/// Verifies: [EXTENSION] [GS §8] (§4.2) GRAPH_TABLE matches SYS_GRAPH's
/// nodes and edges by their tables' labels, through `$FROM_ID` and
/// `$TO_ID`, its quantified paths too.
#[test]
fn sys_graph_is_matched_4_2() {
    let s = social();
    let q = "SELECT A, B FROM GRAPH_TABLE (SYS_GRAPH MATCH (a:Person)-[:Knows]->(b:Person) COLUMNS (a.name AS A, b.name AS B))";
    assert_eq!(rows(&s, q), ["ANN,BOB", "BOB,CY"]);
    let far = "SELECT N FROM GRAPH_TABLE (SYS_GRAPH MATCH (a:Person WHERE a.name = 'ANN')-[:Knows]->{2}(b) COLUMNS (b.name AS N))";
    assert_eq!(rows(&s, far), ["CY"]);
    let lives = "SELECT N, C FROM GRAPH_TABLE (SYS_GRAPH MATCH (p)-[:Lives_In]->(c:City) COLUMNS (p.name AS N, c.city_name AS C))";
    assert_eq!(rows(&s, lives), ["BOB,ROME"]);
    let ids = "SELECT I FROM GRAPH_TABLE (SYS_GRAPH MATCH (c:City) COLUMNS (ELEMENT_ID(c) AS I))";
    assert_eq!(rows(&s, ids).len(), 1);
}
