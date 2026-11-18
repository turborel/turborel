//! [GS §10.3] PGQL's reachability, of edges or of a path macro: whether
//! two nodes are joined, each pair once.

use super::{people, refused, rows};

/// Verifies: GS §10.3: `-/:label*/->`, `+`, `{n}`, `<-/ /-` and `-/ /-`
/// match each pair of nodes once, however many paths join them; a path
/// holding a reachability keeps the bindings of its other parts.
#[test]
fn pgql_reachability_10_3() {
    let (_db, s) = people();
    let q = "SELECT b.name FROM MATCH (a)-/:Follows+/->(b) ON chain WHERE a.id = 1";
    assert_eq!(rows(&s, q), ["Bob", "Carol", "Dave", "Erin"]);
    let q = "SELECT b.name FROM MATCH (a)-/:Follows*/->(b) ON chain WHERE a.id = 1";
    assert_eq!(rows(&s, q), ["Alice", "Bob", "Carol", "Dave", "Erin"]);
    let q = "SELECT b.name FROM MATCH (a)<-/:Follows{2}/-(b) ON chain WHERE a.id = 4";
    assert_eq!(rows(&s, q), ["Alice", "Bob"]);
    let q = "SELECT b.name FROM MATCH (a)-/:Follows/-(b) ON chain WHERE a.id = 4";
    assert_eq!(rows(&s, q), ["Carol", "Erin"]);
    let q = "SELECT a.name, e.id, c.name FROM MATCH (a)-[e]->()-/:Follows+/->(c) ON chain WHERE a.id = 1";
    assert_eq!(rows(&s, q), ["Alice,12,Carol", "Alice,12,Dave", "Alice,15,Dave"]);
    let q = "SELECT COUNT(*) FROM MATCH (a)-/:Follows*/->(b)-/:Follows*/->(c) ON chain WHERE a.id = 1";
    assert_eq!(rows(&s, q), ["13"]);
    let q = "SELECT COUNT(*) FROM MATCH ()-/:FRIEND_OF/->(a), MATCH ()-/:FRIEND_OF/->(b) WHERE a.id = 4 AND b.id = 2";
    assert_eq!(rows(&s, q), ["2"], "each anonymous node is its own");
}

/// Verifies: GS §10.3: a path macro repeated by a reachability, its
/// WHERE a condition of each repetition, its variables its own at each
/// use.
#[test]
fn pgql_path_macros_10_3() {
    let (_db, s) = people();
    let q = "PATH two AS (x)-[:Follows]->()-[:Follows]->(y) SELECT b.name FROM MATCH (a)-/:two+/->(b) ON chain WHERE a.id = 1";
    assert_eq!(rows(&s, q), ["Carol", "Dave"]);
    let q = "PATH late AS (x)-[e:FRIEND_OF]->(y) WHERE e.since > 2020 \
             SELECT x.name, b.name FROM MATCH (x:PERSON)-/:late*/->(b) WHERE x.id = 2";
    assert_eq!(rows(&s, q), ["Bob,Bob", "Bob,Carol", "Bob,Dave"]);
    let q = "PATH f AS (x)-[:Follows]->(y) SELECT a.name, b.name FROM MATCH (a)-/:f/->(m) ON chain, MATCH (m)-/:f/->(b) ON chain \
             WHERE a.id = 1";
    assert_eq!(rows(&s, q), ["Alice,Carol", "Alice,Dave"]);
}

/// Verifies: GS §10.3: what a reachability and a path macro refuse.
#[test]
fn reachability_refusals_10_3() {
    let (_db, s) = people();
    assert_eq!(refused(&s, "SELECT a.name FROM MATCH (a)-/x:Follows*/->(b) ON chain", "binds no variable"), -102);
    assert_eq!(refused(&s, "SELECT a.name FROM MATCH (a)<-/:Follows*/->(b) ON chain", "one way or neither"), -102);
    assert_eq!(refused(&s, "SELECT a.name FROM MATCH (a)-/:Follows*/->-[e]->(b) ON chain", "between two node patterns"), -102);
    assert_eq!(refused(&s, "SELECT a.name FROM MATCH p = (a)-/:Follows*/->(b) ON chain", "begins with a node pattern"), -102);
    let q = "PATH f AS (x)->(y) PATH f AS (x)->(y) SELECT a.name FROM MATCH (a)-/:f/->(b) ON chain";
    assert_eq!(refused(&s, q, "defined twice"), -103);
    let q = "PATH f AS ANY (x)->(y) SELECT a.name FROM MATCH (a)-/:f/->(b) ON chain";
    assert_eq!(refused(&s, q, "no path variable or prefix"), -103);
}
