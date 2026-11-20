//! GQL's SET, REMOVE and DELETE (graph standard §12, RULES.md D144) of
//! every item form: over an open graph's tables and over a graph over
//! tables, and their refusals.

use super::{bank, direct, failed, gql};
use turborel::db::{Database, Session};

/// A session of schema A with the open graph FREE: an A of X 1 and Y 2
/// and an edge R to a B, and a C alone.
fn free() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    s.run("CREATE GRAPH FREE ANY").unwrap();
    on(&s, "INSERT (:A {x: 1, y: 2})-[:R {w: 5}]->(:B {z: 3}), (:C)");
    (db, s)
}

/// The rows of GQL `text` on FREE.
fn on(s: &Session, text: &str) -> Vec<String> {
    direct(s, &format!("EXECUTE GQL ON GRAPH FREE '{}'", text.replace('\'', "''")))
}

/// Verifies: [EXTENSION] graph standard §12: SET of all an open graph's
/// element's properties replaces its map, REMOVE of a property takes it
/// out, and REMOVE of a label takes it away.
#[test]
fn set_all_and_remove_over_an_open_graph_12() {
    let (_db, s) = free();
    on(&s, "MATCH (a:A) SET a = {x: 7}");
    assert_eq!(on(&s, "MATCH (a:A) RETURN a.x AS x, a.y AS y"), ["7,NULL"]);
    on(&s, "MATCH (b:B) SET b:D REMOVE b.z, b:B");
    assert_eq!(on(&s, "MATCH (b:D) RETURN b.z AS z, LABELS(b) AS l"), ["NULL,|D|"]);
}

/// Verifies: [EXTENSION] graph standard §12 R12.1: DELETE of an open
/// graph's vertex with edges is refused (GRF-0052, SQLCODE -201) and
/// deletes nothing; DELETE of an edge, of a vertex alone and DETACH DELETE
/// of a vertex and its edges delete them; DELETE of a variable not in
/// scope is refused (SQLCODE -103). An empty element pattern of an open
/// graph is matched. It is a regression test, since `()` lost its closing
/// parenthesis when given its own variable, and was refused (SQLCODE
/// -102).
#[test]
fn delete_over_an_open_graph_12_1() {
    let (_db, s) = free();
    let (code, msg) = failed(&s, "EXECUTE GQL ON GRAPH FREE 'MATCH (a:A) DELETE a'");
    assert_eq!(code, -201, "{msg}");
    assert!(msg.contains("GRF-0052"), "{msg}");
    assert_eq!(on(&s, "MATCH (n) RETURN COUNT(*) AS n"), ["3"]);
    assert_eq!(on(&s, "MATCH ()-[]->(b) RETURN LABELS(b) AS l"), ["|B|"]);
    on(&s, "MATCH (c:C) DELETE c");
    on(&s, "MATCH ()-[r:R]->() DELETE r");
    assert_eq!(on(&s, "MATCH ()-[r]->() RETURN COUNT(*) AS n"), ["0"]);
    on(&s, "INSERT (:A)-[:R]->(:B)");
    on(&s, "MATCH (a:A) DETACH DELETE a");
    assert_eq!(on(&s, "MATCH (n) RETURN LABELS(n) AS l ORDER BY l"), ["|B|", "|B|"]);
    let (code, msg) = failed(&s, "EXECUTE GQL ON GRAPH FREE 'MATCH (a:B) DELETE q'");
    assert_eq!(code, -103, "{msg}");
}

/// Verifies: [EXTENSION] graph standard §12: over a graph over tables, SET
/// of all an element's properties makes each one it does not name null,
/// REMOVE of a property makes it null, SET of a label the element's table
/// has changes nothing, and SET of a label it lacks or REMOVE of one it
/// has is refused (SQLCODE -103), its labels being its table's; SET of an
/// unmatched optional binding writes nothing.
#[test]
fn set_and_remove_over_tables_12() {
    let (_db, s) = bank();
    gql(&s, "MATCH (a:ACCOUNT {owner: 'Dee'}) SET a = {acct_id: 4, owner: 'Di'}");
    assert_eq!(direct(&s, "SELECT OWNER, IS_BLOCKED FROM A.ACCOUNT WHERE ACCT_ID = 4"), ["Di,NULL"]);
    gql(&s, "MATCH (a:ACCOUNT {owner: 'Di'}) SET a:ACCOUNT REMOVE a.owner");
    assert_eq!(direct(&s, "SELECT COALESCE(OWNER, '-') FROM A.ACCOUNT WHERE ACCT_ID = 4"), ["-"]);
    gql(&s, "MATCH (c:CITY {city_name: 'Rome'}) OPTIONAL MATCH (c)<-[:IS_LOCATED_IN]-(a {owner: 'Zed'}) SET a.owner = 'Q'");
    assert_eq!(direct(&s, "SELECT COUNT(*) FROM A.ACCOUNT WHERE OWNER = 'Q'"), ["0"]);
    for text in ["MATCH (a:ACCOUNT) SET a:CITY", "MATCH (c:CITY) REMOVE c:PLACE"] {
        let (code, msg) = failed(&s, &format!("EXECUTE GQL ON GRAPH BANK '{text}'"));
        assert_eq!(code, -103, "{text}: {msg}");
        assert!(msg.contains("its labels being its table's"), "{msg}");
    }
}
