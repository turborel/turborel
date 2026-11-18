//! Node and edge tables kept across reopening (§4.2): their rows and
//! identities, the next identity drawn after them, their CONNECTION
//! constraints, adjacency indexes and SYS_GRAPH, made again from the log
//! or taken from a checkpoint's snapshot.

use turborel::db::Database;

use crate::common::Scratch;

use super::{SOCIAL, node, refused, rows};

/// Whether every person has an identity of its own: a generator's
/// values after reopening may skip some, as an identity column's do.
const UNIQUE: &str = "SELECT CASE WHEN COUNT(DISTINCT $NODE_ID) = COUNT(*) THEN 'YES' ELSE 'NO' END FROM PERSON";

/// What `db` shows of schema A's node and edge tables and SYS_GRAPH.
fn shown(db: &Database) -> Vec<Vec<String>> {
    let a = db.session("A");
    vec![
        rows(&a, "SELECT $NODE_ID, NAME FROM PERSON ORDER BY NAME"),
        rows(&a, "SELECT $EDGE_ID, $FROM_ID, $TO_ID FROM LIVES_IN"),
        rows(&a, "SELECT ELEMENT_TABLE_ALIAS FROM INFORMATION_SCHEMA.PG_ELEMENT_TABLES ORDER BY 1"),
        rows(&a, "SELECT FROM_TABLE_NAME, TO_TABLE_NAME, DELETE_RULE FROM INFORMATION_SCHEMA.CONNECTION_CONSTRAINTS"),
    ]
}

/// Verifies: [EXTENSION] §4.2: node and edge tables, their rows, their
/// CONNECTION constraints and SYS_GRAPH are as they were after reopening
/// from the log and from a checkpoint; identities drawn after are new, if
/// not next; a node's deletion still finds its edges through the
/// adjacency index, which reopening kept.
#[test]
fn node_and_edge_tables_are_kept_across_reopening_4_2() {
    let scratch = Scratch::new("graph-native-reopen");
    let dir = scratch.path("db");
    let (ann, rome) = (node("PERSON", 0), node("CITY", 0));
    let before = {
        let db = Database::open(&dir).unwrap();
        db.schema(SOCIAL).unwrap();
        let a = db.session("A");
        a.run("INSERT INTO PERSON VALUES ('ANN', 30), ('BOB', 40)").unwrap();
        a.run("INSERT INTO CITY VALUES ('ROME')").unwrap();
        a.run(&format!("INSERT INTO LIVES_IN ($FROM_ID, $TO_ID) VALUES ('{ann}', '{rome}')")).unwrap();
        a.run(&format!("INSERT INTO KNOWS ($FROM_ID, $TO_ID) VALUES ('{ann}', '{ann}')")).unwrap();
        a.commit().unwrap();
        shown(&db)
    };
    {
        let db = Database::open(&dir).unwrap();
        assert_eq!(shown(&db), before);
        let a = db.session("A");
        a.run("INSERT INTO PERSON VALUES ('CY', 50)").unwrap();
        a.commit().unwrap();
        assert_eq!(rows(&a, UNIQUE), ["YES"]);
        refused(&a, "DELETE FROM PERSON WHERE NAME = 'ANN'", (-201, "GRF-0052"));
        db.checkpoint().unwrap();
    }
    let db = Database::open(&dir).unwrap();
    let a = db.session("A");
    a.run("DELETE FROM KNOWS").unwrap();
    a.run("DELETE FROM PERSON WHERE NAME = 'ANN'").unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM LIVES_IN"), ["0"]);
    a.run("INSERT INTO PERSON VALUES ('DI', 60)").unwrap();
    assert_eq!(rows(&a, UNIQUE), ["YES"]);
    a.commit().unwrap();
}
