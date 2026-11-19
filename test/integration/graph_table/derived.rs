//! Graphs made from query results [GS §11.3]: LINE_GRAPH in a
//! GRAPH_TABLE, and CREATE PROPERTY GRAPH ... AS of queries' rows.

use super::{bank, refused, rows};

/// Verifies: [EXTENSION] [GS §11.3] LINE_GRAPH ( g , L ) has a vertex of
/// each edge of label L, its properties L's, and an edge FOLLOWS from each
/// to each that leaves where it ends; a condition between consecutive
/// edges is one between adjacent vertices, the PREV formulation's (T21).
#[test]
fn line_graph_11_3() {
    let (_db, s) = bank();
    let count = "SELECT COUNT(*) FROM GRAPH_TABLE (LINE_GRAPH(BANK, TRANSFER) MATCH (x:TRANSFER) COLUMNS (x.id AS X))";
    assert_eq!(rows(&s, count), ["8"]);
    let line = "SELECT X, Y FROM GRAPH_TABLE (LINE_GRAPH(BANK, TRANSFER) MATCH (x)-[f:FOLLOWS]->(y) \
                WHERE x.when_at < y.when_at COLUMNS (x.id AS X, y.id AS Y))";
    let edges = "SELECT X, Y FROM GRAPH_TABLE (BANK MATCH ()-[t:Transfer]->()-[u:Transfer]->() \
                 WHERE t.when_at < u.when_at COLUMNS (t.id AS X, u.id AS Y))";
    assert_eq!(rows(&s, line), rows(&s, edges));
    assert_eq!(rows(&s, line), ["t1,t2", "t1,t7", "t2,t3", "t3,t4", "t4,t5", "t4,t6", "t5,t7", "t6,t8", "t7,t8"]);
    let prev = "SELECT A, B FROM GRAPH_TABLE (BANK MATCH (a) [-[t:Transfer]-> WHERE PREV(t) IS NULL OR t.when_at > PREV(t).when_at]{3} (b) \
                COLUMNS (a.id AS A, b.id AS B))";
    let three = "SELECT A, B FROM GRAPH_TABLE (LINE_GRAPH(BANK, TRANSFER) MATCH (x)-[f:FOLLOWS]->(y)-[g:FOLLOWS]->(z) \
                 WHERE x.when_at < y.when_at AND y.when_at < z.when_at COLUMNS (x.id AS A, z.id AS B))";
    assert_eq!(rows(&s, prev).len(), rows(&s, three).len());
    let (code, msg) = refused(&s, "SELECT 1 FROM GRAPH_TABLE (LINE_GRAPH(BANK, OWNS) MATCH (x) COLUMNS (x.id AS X))");
    assert!(code == -103 && msg.contains("GRF-0012"), "{code} {msg}");
}

/// The graph of large transfers, of queries' rows.
const LARGE: &str = "CREATE PROPERTY GRAPH LARGE AS \
     VERTICES FROM (SELECT ID, OWNER FROM ACCOUNT) KEY (ID) LABEL HOLDER PROPERTIES (ID, OWNER) \
     EDGES FROM (SELECT ID, SRC, DST, AMOUNT FROM TRANSFER WHERE AMOUNT >= 9) KEY (ID) \
       SOURCE KEY (SRC) REFERENCES HOLDER (ID) DESTINATION KEY (DST) REFERENCES HOLDER (ID) LABEL BIG PROPERTIES (AMOUNT)";

/// Verifies: [EXTENSION] [GS §11.3] CREATE PROPERTY GRAPH ... AS of
/// queries' rows: its element tables read as it is queried, of TRUSTED
/// MODE, its definition and information schema rows of its queries, its
/// tables' dependents, and dropped as any graph is.
#[test]
fn graphs_of_query_results_11_3() {
    let (_db, s) = bank();
    s.run(LARGE).unwrap();
    let q = "SELECT A, B, M FROM GRAPH_TABLE (LARGE MATCH (a)-[e:BIG]->(b) COLUMNS (a.id AS A, b.id AS B, e.amount AS M))";
    assert_eq!(rows(&s, q), ["a2,a4,10", "a3,a2,10", "a4,a6,10", "a5,a1,9", "a6,a3,10"]);
    s.run("INSERT INTO TRANSFER VALUES ('t9', 'a1', 'a2', DATE '2020-10-01', 20)").unwrap();
    assert!(rows(&s, q).contains(&"a1,a2,20".to_owned()));
    let info = "SELECT ELEMENT_TABLE_ALIAS, ELEMENT_TABLE_KIND, TABLE_NAME, ELEMENT_TABLE_QUERY FROM INFORMATION_SCHEMA.PG_ELEMENT_TABLES \
                WHERE PROPERTY_GRAPH_NAME = 'LARGE'";
    assert_eq!(
        rows(&s, info),
        ["BIG,EDGE,NULL,SELECT ID, SRC, DST, AMOUNT FROM TRANSFER WHERE AMOUNT >= 9", "HOLDER,VERTEX,NULL,SELECT ID, OWNER FROM ACCOUNT"]
    );
    let mode = "SELECT PROPERTY_GRAPH_MODE FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS WHERE PROPERTY_GRAPH_NAME = 'LARGE'";
    assert_eq!(rows(&s, mode), ["TRUSTED"]);
    s.run("ALTER PROPERTY GRAPH LARGE COMPILE").unwrap();
    assert_eq!(rows(&s, q).len(), 6);
    let e = s.run("DROP TABLE TRANSFER RESTRICT").unwrap_err();
    assert_eq!(e.sqlcode(), -103, "{}", e.msg);
    s.run("DROP PROPERTY GRAPH LARGE").unwrap();
    assert_eq!(rows(&s, info), Vec::<String>::new());
    let enforced = LARGE.replace("LABEL BIG PROPERTIES (AMOUNT)", "LABEL BIG PROPERTIES (AMOUNT) OPTIONS (ENFORCED MODE)");
    let e = s.run(&enforced).unwrap_err();
    assert!(e.sqlcode() == -103 && e.msg.contains("TRUSTED"), "{}", e.msg);
}
