//! Adjacency indexes of a graph's edge tables [GS §13.2]: CREATE
//! ADJACENCY INDEX ON GRAPH, the indexes an ENFORCED graph is given, and
//! a GRAPH_TABLE followed through them as it is without, after changes,
//! reopened and after a crash.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Session};

use super::{BANK, FIXTURE, rows};
use crate::common::{Scratch, after_crash, os};

/// The shortest paths of transfers from and to each account, and the
/// accounts that share a phone.
const QUERIES: &[&str] = &[
    "SELECT A, B, T FROM GRAPH_TABLE (BANK MATCH ANY SHORTEST (a)-[t:Transfer]->+(b) COLUMNS (a.id AS A, b.id AS B, ARRAY_AGG(t.id) AS T))",
    "SELECT A, B, N FROM GRAPH_TABLE (BANK MATCH ANY SHORTEST (a)<-[t:Transfer]-+(b) COLUMNS (a.id AS A, b.id AS B, COUNT(t.*) AS N))",
    "SELECT A, B FROM GRAPH_TABLE (BANK MATCH (a:Account)~[:HasPhone]~(p)~[:HasPhone]~(b:Account) COLUMNS (a.id AS A, b.id AS B))",
];

/// The rows of each of `QUERIES` in `s`.
fn answers(s: &Session) -> Vec<Vec<String>> {
    QUERIES.iter().map(|q| rows(s, q)).collect()
}

/// The names of the indexes of the database's tables of the adjacency
/// method.
fn adjacency_indexes(s: &Session) -> Vec<String> {
    rows(s, "SELECT INDEX_NAME FROM INFORMATION_SCHEMA.INDEXES WHERE METHOD_NAME = 'adjacency'")
}

/// The fixture in `db`, its graph made of BANK as `edit` changes it,
/// committed; a session of A.
fn fixture(db: &Database, edit: impl Fn(&str) -> String) -> Session {
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    for sql in FIXTURE {
        s.run(sql).unwrap();
    }
    s.run(&edit(BANK)).unwrap();
    s.run("COMMIT WORK").unwrap();
    s
}

/// Verifies: [EXTENSION] [GS §13.2] an ENFORCED graph's edge tables are
/// each given an adjacency index of their ends as it is created; a
/// TRUSTED graph's are not, and its matches are the same without them.
#[test]
fn enforced_graphs_are_indexed_13_2() {
    let db = Database::new();
    let s = fixture(&db, |g| g.to_owned() + " OPTIONS (TRUSTED MODE)");
    assert_eq!(adjacency_indexes(&s), Vec::<String>::new());
    let without = answers(&s);
    s.run("DROP PROPERTY GRAPH BANK").unwrap();
    s.run(BANK).unwrap();
    assert!(adjacency_indexes(&s).is_empty(), "made as it commits");
    s.run("COMMIT WORK").unwrap();
    assert_eq!(adjacency_indexes(&s).len(), 4);
    assert_eq!(answers(&s), without);
    s.run("DROP PROPERTY GRAPH BANK").unwrap();
    s.run(BANK).unwrap();
    s.run("COMMIT WORK").unwrap();
    assert_eq!(adjacency_indexes(&s).len(), 4, "none made twice");
}

/// Verifies: [EXTENSION] [GS §13.2] CREATE ADJACENCY INDEX names an edge
/// table by its alias, keeps one way or both and the columns INCLUDE
/// names; a table not an edge table of the graph is refused, GRF-0012.
#[test]
fn adjacency_index_statement_13_2() {
    let db = Database::new();
    let s = fixture(&db, |g| g.to_owned() + " OPTIONS (TRUSTED MODE)");
    let without = answers(&s);
    s.run("COMMIT WORK").unwrap();
    s.run("CREATE ADJACENCY INDEX FORWARD_ONLY ON GRAPH BANK EDGE TABLE TRANSFER DIRECTION FORWARD INCLUDE (AMOUNT)").unwrap();
    s.run("CREATE ADJACENCY INDEX ON GRAPH BANK EDGE TABLE HAS_PHONE DIRECTION BOTH").unwrap();
    assert_eq!(adjacency_indexes(&s), ["BANK_HAS_PHONE_ADJACENCY", "FORWARD_ONLY"]);
    assert_eq!(answers(&s), without);
    s.run("COMMIT WORK").unwrap();
    let e = s.run("CREATE ADJACENCY INDEX ON GRAPH BANK EDGE TABLE ACCOUNT").unwrap_err();
    assert!(e.sqlcode() == -103 && e.msg.contains("GRF-0012"), "{e:?}");
    assert_eq!(s.run("CREATE ADJACENCY INDEX ON GRAPH BANK EDGE TABLE TRANSFER DIRECTION UP").unwrap_err().sqlcode(), -102);
}

/// Verifies: [EXTENSION] [GS §13.2] a GRAPH_TABLE followed through an
/// adjacency index sees the transaction's own changes, and not those
/// rolled back.
#[test]
fn indexed_matches_follow_changes_13_2() {
    let db = Database::new();
    let s = fixture(&db, str::to_owned);
    let one = |s: &Session| rows(s, "SELECT B FROM GRAPH_TABLE (BANK MATCH (a WHERE a.id = 'a1')-[t:Transfer]->(b) COLUMNS (b.id AS B))");
    assert_eq!(one(&s), ["a3"]);
    s.run("INSERT INTO TRANSFER VALUES ('t9', 'a1', 'a4', DATE '2020-10-01', 5)").unwrap();
    s.run("UPDATE TRANSFER SET SRC = 'a2' WHERE ID = 't1'").unwrap();
    assert_eq!(one(&s), ["a4"]);
    s.run("ROLLBACK WORK").unwrap();
    assert_eq!(one(&s), ["a3"]);
}

/// The database in `dir`, opened of `files`.
fn opened(dir: &Path, crashed: bool) -> Database {
    let db = Database::new();
    let files = if crashed { after_crash() } else { os() };
    db.open_files(&files, dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D25: a graph's adjacency indexes opened
/// again, and after a crash, give the matches they gave, committed
/// changes found and an uncommitted one not.
#[test]
fn indexed_graphs_are_reopened_and_recovered() {
    let scratch = Scratch::new("graph-table-adjacency");
    let dir = scratch.path("db");
    let db = opened(&dir, true);
    let s = fixture(&db, str::to_owned);
    let before = answers(&s);
    s.run("COMMIT WORK").unwrap();
    db.close().unwrap();
    drop((s, db));
    let db = opened(&dir, true);
    let s = db.session("A");
    assert_eq!(answers(&s), before, "reopened");
    db.checkpoint().unwrap();
    s.run("INSERT INTO TRANSFER VALUES ('t9', 'a2', 'a1', DATE '2020-10-01', 5)").unwrap();
    s.run("COMMIT WORK").unwrap();
    let changed = answers(&s);
    s.run("INSERT INTO TRANSFER VALUES ('t10', 'a4', 'a1', DATE '2020-10-01', 5)").unwrap();
    drop(db);
    let db = opened(&dir, true);
    let s = db.session("A");
    assert_eq!(adjacency_indexes(&s).len(), 4);
    assert_eq!(answers(&s), changed, "recovered");
}
