//! [GS §10.2, R5.1, R5.2, R16.8] What Oracle's GRAPH_TABLE writes: the
//! identity functions VERTEX_ID and EDGE_ID and the predicates
//! VERTEX_EQUAL and EDGE_EQUAL, the dot notation of a JSON property, and
//! hints in a comment after GRAPH_TABLE's parenthesis; and the identity
//! as `$node_id` and `$edge_id` spell it.

use super::{people, refused, rows};

/// Verifies: GS §10.2, R5.1: VERTEX_ID and EDGE_ID give an element's
/// identity as a JSON value, ELEMENT_ID's text, one per element.
#[test]
fn identity_functions_10_2() {
    let (_db, s) = people();
    let q = "SELECT COUNT(DISTINCT VID), COUNT(DISTINCT EID), COUNT(*) FROM GRAPH_TABLE (CHAIN MATCH (a)-[e]->(b) \
             COLUMNS (JSON_SERIALIZE(VERTEX_ID(a)) AS VID, JSON_SERIALIZE(EDGE_ID(e)) AS EID))";
    assert_eq!(rows(&s, q), ["4,5,5"]);
    let q = "SELECT COUNT(*) FROM GRAPH_TABLE (CHAIN MATCH (a)-[e]->(b) \
             COLUMNS (JSON_SERIALIZE(VERTEX_ID(a)) AS VID, ELEMENT_ID(a) AS ID)) WHERE VID = ID";
    assert_eq!(rows(&s, q), ["5"]);
}

/// Verifies: GS §10.2, R5.2: VERTEX_EQUAL and EDGE_EQUAL are SAME of two
/// vertices and of two edges.
#[test]
fn identity_predicates_10_2() {
    let (_db, s) = people();
    let q = "SELECT a, c FROM GRAPH_TABLE (CHAIN MATCH (x)-[e]->(y)<-[f]-(z) WHERE NOT VERTEX_EQUAL(x, z) \
             COLUMNS (x.name AS a, z.name AS c))";
    assert_eq!(rows(&s, q), ["Carol,Erin", "Erin,Carol"]);
    let q = "SELECT COUNT(*) FROM GRAPH_TABLE (CHAIN MATCH (x)-[e]->(y)<-[f]-(z) WHERE EDGE_EQUAL(e, f) COLUMNS (1 AS one))";
    assert_eq!(rows(&s, q), ["5"]);
}

/// Verifies: GS §10.2: a JSON property is read by the dot notation of
/// the JSON type's simplified accessor.
#[test]
fn json_property_dot_notation_10_2() {
    let (_db, s) = people();
    s.run("CREATE TABLE PLACE (ID INTEGER PRIMARY KEY, INFO JSON)").unwrap();
    s.run(r#"INSERT INTO PLACE VALUES (1, JSON('{"city":"Rome","tags":["old","big"]}')), (2, JSON('{"city":"Oslo"}'))"#).unwrap();
    s.run("CREATE PROPERTY GRAPH ATLAS VERTEX TABLES (PLACE PROPERTIES (ID, INFO))").unwrap();
    let q = r#"SELECT * FROM GRAPH_TABLE (ATLAS MATCH (p) WHERE p.info."city".string() = 'Rome'
               COLUMNS (p.id AS id, p.info."city" AS city, p.info."tags"[1] AS tag))"#;
    assert_eq!(rows(&s, q), [r#"1,"Rome","big""#]);
}

/// Verifies: GS §10.2, R16.8: hints in a comment after GRAPH_TABLE's
/// parenthesis are read, an unknown one ignored, and change no row; a
/// hint naming no node variable is refused.
#[test]
fn hints_change_no_row_10_2() {
    let (_db, s) = people();
    let plain = "SELECT a, b FROM GRAPH_TABLE (CHAIN MATCH ANY SHORTEST (x)-[e]->+(y) COLUMNS (x.name AS a, y.name AS b))";
    let hinted = "SELECT a, b FROM GRAPH_TABLE (/*+ GRAPH_START(x) GRAPH_DIRECTION(e, REVERSE) NO_WCOJ USE_ADJACENCY(FOLLOWS) \
                  PATH_ALGO(DFS) PARALLEL(4) */ CHAIN MATCH ANY SHORTEST (x)-[e]->+(y) COLUMNS (x.name AS a, y.name AS b))";
    assert_eq!(rows(&s, hinted), rows(&s, plain));
    for algo in ["BIBFS", "DIJKSTRA"] {
        let q = format!(
            "SELECT a, b FROM GRAPH_TABLE (/*+ PATH_ALGO({algo}) */ CHAIN MATCH ANY SHORTEST (x)-[e]->+(y) COLUMNS (x.name AS a, y.name AS b))"
        );
        assert_eq!(rows(&s, &q), rows(&s, plain));
    }
    let q = "SELECT * FROM GRAPH_TABLE (/*+ GRAPH_START(z) */ CHAIN MATCH (x)-[e]->(y) COLUMNS (x.id))";
    assert_eq!(refused(&s, q, "no variable"), -103);
    let q = "SELECT * FROM GRAPH_TABLE (/*+ PATH_ALGO(SIDEWAYS) */ CHAIN MATCH (x)-[e]->(y) COLUMNS (x.id))";
    assert_eq!(refused(&s, q, "PATH_ALGO"), -102);
}

/// Verifies: GS R5.1: `$node_id` and `$edge_id` of a variable inside
/// GRAPH_TABLE are its identity; of a node or edge table's element, its
/// table's pseudo-column.
#[test]
fn identity_spellings_5_1() {
    let (_db, s) = people();
    let q = "SELECT COUNT(*) FROM GRAPH_TABLE (SYS_GRAPH MATCH (a)-[e]->(b) COLUMNS (a.$node_id AS n, e.$edge_id AS i)) g, \
             person p, friend_of f WHERE g.n = p.$node_id AND g.i = f.$edge_id";
    assert_eq!(rows(&s, q), ["5"]);
    let q = "SELECT COUNT(*) FROM GRAPH_TABLE (CHAIN MATCH (a)-[e]->(b) COLUMNS (a.$node_id AS n, ELEMENT_ID(a) AS i, \
             e.$EDGE_ID AS en, ELEMENT_ID(e) AS ei)) WHERE n = i AND en = ei";
    assert_eq!(rows(&s, q), ["5"]);
    let q = "SELECT * FROM GRAPH_TABLE (CHAIN MATCH (a)-[e]->(b) COLUMNS (e.$node_id AS n))";
    assert_eq!(refused(&s, q, "$NODE_ID"), -103);
}
