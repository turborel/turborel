//! The adjacency index, `USING adjacency`, through SQL: lookups of an edge
//! table's source finding the rows a scan finds, of its compacted
//! adjacency and of what was written since; kept as rows change, rolled
//! back, of each version a reader may see, reopened and recovered after a
//! crash, of a budget too small for its compacted adjacency, read by a
//! temporary table, used by the planner for a join, refused where it
//! cannot be made, and shown by the Information Schema.

mod uses;

use std::path::Path;

use turborel::db::{Database, OpenOptions, Session};
use turborel::params::Params;

use crate::common::{Scratch, after_crash, exec, expected, os, rows};

/// How many edges each table holds.
const EDGES: u64 = 1_500;
/// How many vertices they join.
const VERTICES: u64 = 100;

/// The next of a seeded sequence of pseudo-random numbers.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

/// Schema G's tables E, of the adjacency index E_ADJ of `with` on its
/// (SRC, DST, AMOUNT), AMOUNT included, and P, of none; each of the same
/// `EDGES` edges among `VERTICES` vertices of seed `seed`.
fn graph(db: &Database, seed: u64, with: &str) -> Session {
    let table = "(ID INTEGER NOT NULL, SRC INTEGER NOT NULL, DST INTEGER NOT NULL, AMOUNT INTEGER NOT NULL)";
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION G CREATE TABLE E {table} CREATE TABLE P {table}")).unwrap();
    let s = db.session("G");
    let mut state = seed;
    let edges: Vec<String> = (0..EDGES)
        .map(|id| format!("({id}, {}, {}, {})", next(&mut state) % VERTICES, next(&mut state) % VERTICES, next(&mut state) % 1_000))
        .collect();
    for chunk in edges.chunks(100) {
        for t in ["E", "P"] {
            exec(&s, &format!("INSERT INTO {t} VALUES {}", chunk.join(", "))).unwrap();
        }
    }
    exec(&s, "COMMIT WORK").unwrap();
    exec(&s, &format!("CREATE INDEX E_ADJ ON E (SRC, DST, AMOUNT) USING adjacency WITH (ADJACENCY.INCLUDE = 1{with})")).unwrap();
    s
}

/// The rows of `query`, `{}` its table, of E and of P.
fn both(s: &Session, query: &str) -> (Result<Vec<String>, turborel::error::Code>, Result<Vec<String>, turborel::error::Code>) {
    (rows(s, &query.replace("{}", "E")), rows(s, &query.replace("{}", "P")))
}

/// Whether the plan of `query` of E reads E_ADJ.
fn probed(s: &Session, query: &str) -> bool {
    s.physical_plan(&query.replace("{}", "E"), &Params::new()).unwrap().contains("E_ADJ")
}

/// Checks that lookups of E by every source, of a source and destination,
/// and of covered columns alone, find what P's scans find.
fn agree(s: &Session, what: &str) {
    for v in (0..=VERTICES).step_by(7) {
        for query in [
            format!("SELECT ID, DST, AMOUNT FROM {{}} WHERE SRC = {v} ORDER BY ID"),
            format!("SELECT ID FROM {{}} WHERE SRC = {v} AND DST = {} ORDER BY ID", (v * 3) % VERTICES),
            format!("SELECT DST, AMOUNT FROM {{}} WHERE SRC = {v} ORDER BY DST, AMOUNT"),
        ] {
            let (e, p) = both(s, &query);
            assert_eq!(e, p, "{what}: {query}");
        }
    }
}

/// How many compacted adjacencies hold memory of the budget, as its
/// owner, G, sees them.
fn compacted(db: &Database, s: &Session) -> usize {
    let _ = db.create_user("G", b"g");
    db.set_owner("G").unwrap();
    let held =
        "SELECT COUNT(*) FROM INFORMATION_SCHEMA.MEMORY_RESERVATIONS WHERE RESERVED_BY = 'an adjacency index''s compacted adjacency'";
    rows(s, held).unwrap()[0].parse().unwrap()
}

/// The changes the tests make of `{}`, E and P alike.
const CHANGES: [&str; 4] = [
    "INSERT INTO {} VALUES (9001, 7, 8, 5), (9002, 7, 9, 6), (9003, 100, 7, 1)",
    "UPDATE {} SET DST = 3 WHERE ID < 40",
    "UPDATE {} SET AMOUNT = AMOUNT + 1 WHERE SRC = 7",
    "DELETE FROM {} WHERE ID BETWEEN 50 AND 90 OR SRC = 14",
];

/// Makes each of `CHANGES` of E and P.
fn change(s: &Session) {
    for c in CHANGES {
        for t in ["E", "P"] {
            exec(s, &c.replace("{}", t)).unwrap();
        }
    }
}

/// Verifies: [EXTENSION] RULES.md D34, D-GRAPH-ADJACENCY: an adjacency
/// index's lookups of a source, of a source and destination, and of its
/// covered columns, find what a scan of a table without it finds: read of
/// its tree, of its compacted adjacency once maintenance builds it, of
/// that and what was written since merged, and of one built again.
#[test]
fn lookups_find_what_a_scan_finds() {
    let db = Database::new();
    let s = graph(&db, 7, "");
    assert!(probed(&s, "SELECT ID FROM {} WHERE SRC = 5"), "an equality of the source is a lookup");
    agree(&s, "the tree");
    db.maintain().unwrap();
    assert_eq!(compacted(&db, &s), 1, "built by the load");
    agree(&s, "compacted");
    change(&s);
    exec(&s, "COMMIT WORK").unwrap();
    agree(&s, "compacted, and the delta");
    db.maintain().unwrap();
    agree(&s, "compacted again");
}

/// Verifies: [EXTENSION] RULES.md R-ERR-5: the index follows INSERT,
/// UPDATE and DELETE within a transaction, and a rollback takes its
/// entries back, finding what a scan finds throughout.
#[test]
fn the_index_follows_changes_and_rollbacks() {
    let db = Database::new();
    let s = graph(&db, 3, "");
    db.maintain().unwrap();
    change(&s);
    agree(&s, "changed");
    exec(&s, "ROLLBACK WORK").unwrap();
    agree(&s, "rolled back");
    assert_eq!(rows(&s, "SELECT ID FROM E WHERE SRC = 7 AND DST = 9 AND ID > 9000"), expected(&[]));
}

/// Verifies: [EXTENSION] RULES.md D29, D34: the index holds every version
/// a reader may see: a serializable reader, its snapshot taken, finds an
/// edge's old version by its old source, and not by its new one, while
/// the writer, committed, finds it by the new one alone.
#[test]
fn readers_find_the_version_they_see() {
    let db = Database::new();
    crate::common::serializable(&db);
    let writer = graph(&db, 5, "");
    let reader = db.session("G");
    let at = |s: &Session, src: u64| rows(s, &format!("SELECT ID FROM E WHERE SRC = {src} AND ID = 9100"));
    exec(&writer, "INSERT INTO E VALUES (9100, 1, 2, 3)").unwrap();
    exec(&writer, "COMMIT WORK").unwrap();
    db.maintain().unwrap();
    assert_eq!(at(&reader, 1), expected(&["9100"]), "the reader's snapshot begins");
    exec(&writer, "UPDATE E SET SRC = 50 WHERE ID = 9100").unwrap();
    exec(&writer, "COMMIT WORK").unwrap();
    assert_eq!((at(&reader, 1), at(&reader, 50)), (expected(&["9100"]), expected(&[])), "the old version");
    assert_eq!((at(&writer, 1), at(&writer, 50)), (expected(&[]), expected(&["9100"])), "the new one");
    exec(&reader, "COMMIT WORK").unwrap();
    assert_eq!(at(&reader, 50), expected(&["9100"]));
}

/// The database in `dir`, opened of `files`.
fn opened(dir: &Path, crashed: bool) -> Database {
    let db = Database::new();
    let files = if crashed { after_crash() } else { os() };
    db.open_files(&files, dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D25: an index opened again holds what
/// it held at the checkpoint closing it, its lookups the scan's, of its
/// tree and of its compacted adjacency built again.
#[test]
fn the_index_is_reopened() {
    let scratch = Scratch::new("adjacency-reopen");
    let dir = scratch.path("db");
    let db = opened(&dir, false);
    let s = graph(&db, 11, "");
    change(&s);
    exec(&s, "COMMIT WORK").unwrap();
    db.close().unwrap();
    drop((s, db));
    let db = opened(&dir, false);
    let s = db.session("G");
    agree(&s, "reopened");
    db.maintain().unwrap();
    agree(&s, "compacted");
}

/// Verifies: [EXTENSION] RULES.md D25: an index opened again after a
/// crash has its last checkpoint's entries mended by the rows recovery
/// redid: committed changes found, an uncommitted one not.
#[test]
fn the_index_is_recovered_after_a_crash() {
    let scratch = Scratch::new("adjacency-recovery");
    let dir = scratch.path("db");
    let db = opened(&dir, true);
    let s = graph(&db, 13, "");
    db.checkpoint().unwrap();
    change(&s);
    exec(&s, "COMMIT WORK").unwrap();
    exec(&s, "INSERT INTO E VALUES (9500, 7, 7, 7)").unwrap();
    drop(db);
    for _ in 0..2 {
        let db = opened(&dir, true);
        let s = db.session("G");
        agree(&s, "recovered");
        assert_eq!(rows(&s, "SELECT ID FROM E WHERE SRC = 7 AND ID > 9000 ORDER BY ID"), expected(&["9001", "9002"]));
        db.checkpoint().unwrap();
    }
    drop(s);
}
