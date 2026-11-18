//! [GS §10.3] PGQL's SELECT ... FROM MATCH ... and openCypher's MATCH
//! ... RETURN, each the query of one GRAPH_TABLE: of the graph they name,
//! or of SYS_GRAPH, the schema's node and edge tables.

use super::{people, refused, rows};

/// Verifies: GS §10.3: a PGQL query of a named graph, its WHERE the
/// postfilter, a property and a variable read through the match, and
/// GROUP BY, ORDER BY, LIMIT and OFFSET of its rows.
#[test]
fn pgql_queries_10_3() {
    let (_db, s) = people();
    let q = "SELECT b.name FROM MATCH (a:Member) -[e:Follows]-> (b) ON chain WHERE a.name = 'Alice'";
    assert_eq!(rows(&s, q), ["Bob", "Erin"]);
    let q = "SELECT b.name, COUNT(*) AS n FROM MATCH (a) -> (b) ON chain GROUP BY b.name HAVING COUNT(*) > 1";
    assert_eq!(rows(&s, q), ["Dave,2"]);
    let q = "SELECT a.name FROM MATCH (a) -> (b) ON chain ORDER BY a.name DESC LIMIT 2 OFFSET 1";
    let mut c = s.open_cursor(q).unwrap();
    let mut got = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        got.push(row.iter().map(|v| v.to_string().trim().to_owned()).collect::<Vec<_>>().join(","));
    }
    assert_eq!(got, ["Carol", "Bob"]);
    let q = "SELECT a.name, c.name FROM MATCH (a) -> (b) ON chain, MATCH (b) -> (c) ON chain WHERE a.id = 1";
    assert_eq!(rows(&s, q), ["Alice,Carol", "Alice,Dave"]);
}

/// Verifies: GS §10.3: PGQL of no ON reads SYS_GRAPH; a path's group
/// read by an aggregate is of each path; a PGQL query is a query
/// expression anywhere one is.
#[test]
fn pgql_paths_and_nesting_10_3() {
    let (_db, s) = people();
    let q = "SELECT a.name, LISTAGG(x.name, '->') AS route FROM MATCH ANY SHORTEST (a:person) (-[e]->(x))+ (b) WHERE a.id = 2";
    assert_eq!(rows(&s, q), ["Bob,Carol", "Bob,Carol->Dave"]);
    let q = "SELECT COUNT(*) FROM (SELECT b.name FROM MATCH (a) -> (b) ON chain) AS t";
    assert_eq!(rows(&s, q), ["5"]);
    let q = "SELECT name FROM member WHERE name IN (SELECT b.name FROM MATCH (a) -> (b) ON chain WHERE a.id = 1)";
    assert_eq!(rows(&s, q), ["Bob", "Erin"]);
}

/// Verifies: GS §10.3: an openCypher query, its patterns' labels,
/// directions, property maps and variable lengths, RETURN with implicit
/// grouping, ORDER BY, SKIP and LIMIT.
#[test]
fn cypher_queries_10_3() {
    let (_db, s) = people();
    let q = "USE chain MATCH (a:Member {name: 'Alice'})-[:Follows]->(b) RETURN b.name";
    assert_eq!(rows(&s, q), ["Bob", "Erin"]);
    let q = "USE chain MATCH (a)<-[r]-(b) RETURN a.name AS who, count(r) AS n";
    assert_eq!(rows(&s, q), ["Bob,1", "Carol,1", "Dave,2", "Erin,1"]);
    let q = "USE chain MATCH (a)-[*2..3]->(d) WHERE a.id = 1 RETURN DISTINCT d.name";
    assert_eq!(rows(&s, q), ["Carol", "Dave"]);
    let q = "USE chain MATCH (a)-[]-(b) WHERE a.name = 'Dave' RETURN b.name ORDER BY b.name SKIP 1 LIMIT 1";
    assert_eq!(rows(&s, q), ["Erin"]);
    let q = "USE chain MATCH p = shortestPath((a)-[*]->(d)) WHERE a.id = 1 AND d.id = 4 RETURN length(p), cardinality(nodes(p))";
    assert_eq!(rows(&s, q), ["2,3"]);
    let q = "USE chain MATCH (a)-[r*]->(d) WHERE a.id = 2 RETURN d.name, collect(r.id)";
    assert_eq!(rows(&s, q), ["Carol,ARRAY[23]", "Dave,ARRAY[23, 34]"]);
}

/// Verifies: GS §10.3: Cypher of no USE reads SYS_GRAPH, the node and
/// edge tables, a direct statement beginning with MATCH.
#[test]
fn cypher_reads_node_tables_10_3() {
    let (_db, s) = people();
    let q = "MATCH (a:PERSON)-[f:FRIEND_OF]->(b:PERSON) WHERE f.since > 2030 RETURN a.name, b.name";
    assert_eq!(rows(&s, q), ["Carol,Dave", "Erin,Dave"]);
    let q = "MATCH (a)-[f]->(b) RETURN count(*)";
    assert_eq!(rows(&s, q), ["5"]);
}

/// Verifies: GS §10.3, RULES.md D112: what the front ends refuse.
#[test]
fn front_end_refusals_10_3() {
    let (_db, s) = people();
    assert_eq!(refused(&s, "SELECT a.name FROM MATCH (a) ON chain, MATCH (b) ON sys_graph", "name one graph"), -103);
    assert_eq!(refused(&s, "USE chain MATCH (a)-[e*]->(b) RETURN e.id", "GRF-0022"), -103);
    assert_eq!(refused(&s, "USE chain MATCH (a)-[r*1..x]->(b) RETURN a.name", ""), -102);
    assert_eq!(refused(&s, "USE chain MATCH (a)-[r]->(b)", "RETURN"), -102);
}
