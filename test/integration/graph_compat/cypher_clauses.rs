//! [GS §10.3] openCypher's abbreviated relationships, and a query of
//! several MATCH and OPTIONAL MATCH clauses, a GRAPH_TABLE each joined
//! on the variables they share.

use super::{people, refused, rows};

/// Verifies: GS §10.3: `-->`, `<--`, `--` and `<-->` are relationships
/// of no detail, written between two nodes after MATCH; elsewhere `--`
/// still begins a comment.
#[test]
fn cypher_abbreviated_relationships_10_3() {
    let (_db, s) = people();
    assert_eq!(rows(&s, "USE chain MATCH (a {name: 'Alice'})-->(b) RETURN b.name"), ["Bob", "Erin"]);
    assert_eq!(rows(&s, "USE chain MATCH (a {name: 'Dave'})<--(b) RETURN b.name"), ["Carol", "Erin"]);
    assert_eq!(rows(&s, "MATCH (a:PERSON {name: 'Bob'})--(b) RETURN b.name"), ["Alice", "Carol"]);
    assert_eq!(rows(&s, "MATCH (a:PERSON {name: 'Bob'})<-->(b) RETURN b.name"), ["Alice", "Carol"]);
    assert_eq!(rows(&s, "USE chain MATCH (a)<-[r]->(b) WHERE a.id = 3 RETURN b.name"), ["Bob", "Dave"]);
    assert_eq!(rows(&s, "USE chain MATCH (a)-->()-->(c) WHERE a.id = 1 RETURN c.name"), ["Carol", "Dave"]);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM member WHERE (id)--(2)\n> 0"), ["5"]);
    assert_eq!(rows(&s, "USE chain MATCH (a) -- every member\nRETURN count(a)"), ["5"]);
}

/// Verifies: GS §10.3: MATCH clauses joined on the nodes and
/// relationships they share, each matching its relationships apart from
/// the others'; a clause sharing none is joined to each row before it.
#[test]
fn cypher_match_clauses_join_10_3() {
    let (_db, s) = people();
    let q = "USE chain MATCH (a)-->(b) MATCH (b)-->(c) RETURN a.name, b.name, c.name";
    assert_eq!(rows(&s, q), ["Alice,Bob,Carol", "Alice,Erin,Dave", "Bob,Carol,Dave"]);
    let q = "USE chain MATCH (a)-->(b) MATCH (b)-->(c) RETURN a.name, c.name, count(*)";
    assert_eq!(rows(&s, q), ["Alice,Carol,1", "Alice,Dave,1", "Bob,Dave,1"], "grouped by the names of both clauses");
    let q = "USE chain MATCH (a)-[r]->(b) MATCH (c)-[r]->(d) RETURN count(*)";
    assert_eq!(rows(&s, q), ["5"]);
    let q = "USE chain MATCH (a)-[r]->(b) MATCH (a)-[q]->(c) RETURN count(*)";
    assert_eq!(rows(&s, q), ["7"], "a relationship of one clause may be another's");
    let q = "USE chain MATCH (a {name: 'Bob'}) MATCH (d {name: 'Dave'}) RETURN a.name, d.name";
    assert_eq!(rows(&s, q), ["Bob,Dave"]);
}

/// Verifies: GS §10.3: OPTIONAL MATCH keeps each row it matches nowhere,
/// its variables null; its WHERE filters what it matches, reading an
/// earlier clause's variable as a condition of its join; a first
/// OPTIONAL MATCH gives one row of nulls if nothing matches.
#[test]
fn cypher_optional_match_10_3() {
    let (_db, s) = people();
    let q = "USE chain MATCH (a) OPTIONAL MATCH (a)-->(b) WHERE b.id > 3 RETURN a.name, b.name";
    assert_eq!(rows(&s, q), ["Alice,Erin", "Bob,NULL", "Carol,Dave", "Dave,NULL", "Erin,Dave"]);
    let q = "USE chain MATCH (a {id: 1}), (z {id: 5}) OPTIONAL MATCH (a)-->(b) WHERE b.id < z.id RETURN b.name";
    assert_eq!(rows(&s, q), ["Bob"]);
    let q = "USE chain MATCH (a)-->(b) OPTIONAL MATCH (b)-->(c) RETURN a.name, count(c)";
    assert_eq!(rows(&s, q), ["Alice,2", "Bob,1", "Carol,0", "Erin,0"]);
    let q = "USE chain OPTIONAL MATCH (a {name: 'Nobody'}) RETURN a.name";
    assert_eq!(rows(&s, q), ["NULL"]);
    let q = "USE chain OPTIONAL MATCH (a {id: 4}) OPTIONAL MATCH (a)-->(b) MATCH (a)<--(c) RETURN a.name, b.name, c.name";
    assert_eq!(rows(&s, q), ["Dave,NULL,Carol", "Dave,NULL,Erin"]);
}

/// Verifies: GS §10.3: a path, or a relationship of variable length, is
/// bound by one clause only.
#[test]
fn cypher_clause_refusals_10_3() {
    let (_db, s) = people();
    assert_eq!(refused(&s, "USE chain MATCH p = (a)-->(b) MATCH p = (b)-->(c) RETURN a.name", "bound by an earlier MATCH"), -103);
    assert_eq!(refused(&s, "USE chain MATCH (a)-[r*]->(b) MATCH (c)-[r*]->(d) RETURN a.name", "bound by an earlier MATCH"), -103);
    assert_eq!(refused(&s, "USE chain MATCH (a)-->(b) OPTIONAL (b)-->(c) RETURN a.name", ""), -102);
}
