//! Packed builds and reclaimed leaves seen through SQL (D139): the same
//! answers as an index built row by row, and recovery after a crash during
//! a packed build or after deletes that took leaves out.

use super::*;

const ROWS: i32 = 6_000;

fn spread(db: &Database, table: &str) {
    let session = db.session("S");
    for start in (0..ROWS).step_by(500) {
        let values: Vec<_> = (start..start + 500).map(|id| format!("({id}, {}, {})", id * 17 % 997, id * 71 % 991)).collect();
        session.run(&format!("INSERT INTO {table} VALUES {}", values.join(","))).unwrap();
    }
    session.commit().unwrap();
}

fn tables(db: &Database) {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S CREATE TABLE P (ID INTEGER NOT NULL PRIMARY KEY, X DOUBLE PRECISION, Y DOUBLE PRECISION) \
         CREATE TABLE Q (ID INTEGER NOT NULL PRIMARY KEY, X DOUBLE PRECISION, Y DOUBLE PRECISION)",
    )
    .unwrap();
}

fn windows(table: &str) -> Vec<String> {
    (0..40)
        .map(|n| {
            let (x, y) = (n * 23 % 960, n * 41 % 950);
            format!("SELECT ID FROM {table} WHERE X >= {x} AND X <= {} AND Y >= {y} AND Y <= {} ORDER BY ID", x + 40, y + 35)
        })
        .collect()
}

fn answers(session: &turborel::db::Session, table: &str) -> Vec<Vec<String>> {
    windows(table).iter().map(|q| rows(session, q).unwrap()).collect()
}

/// Verifies: [DESIGN] D139: an rtree index made by CREATE INDEX over
/// filled rows, packed, and one filled row by row as rows are
/// inserted, give identical answers to every window, also after deletes
/// that take leaves out of both, and the plan reads the index.
#[test]
fn packed_and_inserted_indexes_give_identical_results() {
    let db = Database::new();
    tables(&db);
    db.create_index("Q_XY", "S.Q", &["X", "Y"], "rtree").unwrap();
    spread(&db, "P");
    spread(&db, "Q");
    let session = db.session("S");
    session.run("CREATE INDEX P_XY ON P (X, Y) USING RTREE").unwrap();
    session.commit().unwrap();
    let plan = session.physical_plan(&windows("P")[0], &Params::new()).unwrap();
    assert!(plan.contains("P_XY"), "{plan}");
    assert_eq!(answers(&session, "P"), answers(&session, "Q"));
    assert!(answers(&session, "P").iter().map(Vec::len).sum::<usize>() > 100);
    for table in ["P", "Q"] {
        session.run(&format!("DELETE FROM {table} WHERE X < 600 OR Y > 800")).unwrap();
        session.commit().unwrap();
    }
    assert_eq!(answers(&session, "P"), answers(&session, "Q"));
    assert_eq!(db.index_entries("P_XY").unwrap(), db.index_entries("Q_XY").unwrap());
}

/// The database in `dir`, recovering after a crash.
fn recovered(dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [DESIGN] D139: a crash after a packed CREATE INDEX was
/// logged, before any checkpoint held its pages, recovers by building it
/// again from the rows, packed; one after deletes reclaimed leaves, with a
/// checkpoint between them holding retired pages, recovers the index as
/// the rows left it, and the recovered index takes inserts again.
#[test]
fn packed_builds_and_reclaimed_leaves_recover_after_a_crash() {
    let scratch = Scratch::new("spatial-packing-crash");
    let dir = scratch.path("db");
    let original = Database::new();
    original.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    tables(&original);
    spread(&original, "P");
    original.checkpoint().unwrap();
    original.create_index("P_XY", "S.P", &["X", "Y"], "rtree").unwrap();
    let expected = answers(&original.session("S"), "P");
    drop(original);
    let built = recovered(&dir);
    let session = built.session("S");
    assert_eq!(answers(&session, "P"), expected);
    assert_eq!(built.index_entries("P_XY").unwrap(), usize::try_from(ROWS).unwrap());
    session.run("DELETE FROM P WHERE X < 300").unwrap();
    session.commit().unwrap();
    built.checkpoint().unwrap();
    session.run("DELETE FROM P WHERE X < 700 OR Y < 400").unwrap();
    session.commit().unwrap();
    let after = answers(&session, "P");
    let left = built.index_entries("P_XY").unwrap();
    drop((session, built));
    let reclaimed = recovered(&dir);
    let session = reclaimed.session("S");
    assert_eq!(answers(&session, "P"), after);
    assert_eq!(reclaimed.index_entries("P_XY").unwrap(), left);
    session.run("INSERT INTO P VALUES (100000, 10, 10)").unwrap();
    session.commit().unwrap();
    assert_eq!(rows(&session, "SELECT ID FROM P WHERE X >= 9 AND X <= 11 AND Y >= 9 AND Y <= 11").unwrap(), ["100000"]);
}

/// Verifies: [DESIGN] D139: deletes that empty branches and shorten the
/// tree, a checkpoint holding the shorter tree between them, then a crash,
/// recover the index as the rows left it; it grows tall again as rows
/// return and answers every window as a table without an index does.
#[test]
fn a_shortened_tree_recovers_after_a_crash() {
    let scratch = Scratch::new("spatial-shrink-crash");
    let dir = scratch.path("db");
    let original = Database::new();
    original.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    tables(&original);
    original.create_index("P_XY", "S.P", &["X", "Y"], "rtree").unwrap();
    spread(&original, "P");
    let session = original.session("S");
    session.run("DELETE FROM P WHERE ID >= 40").unwrap();
    session.commit().unwrap();
    original.checkpoint().unwrap();
    session.run("DELETE FROM P WHERE ID >= 10").unwrap();
    session.commit().unwrap();
    drop((session, original));
    let shortened = recovered(&dir);
    let session = shortened.session("S");
    assert_eq!(shortened.index_entries("P_XY").unwrap(), 10);
    assert_eq!(rows(&session, "SELECT ID FROM P WHERE X >= 0 AND X <= 1000 AND Y >= 0 AND Y <= 1000 ORDER BY ID").unwrap().len(), 10);
    session.run("DELETE FROM P").unwrap();
    session.commit().unwrap();
    spread(&shortened, "P");
    spread(&shortened, "Q");
    assert_eq!(answers(&session, "P"), answers(&session, "Q"));
    assert_eq!(shortened.index_entries("P_XY").unwrap(), usize::try_from(ROWS).unwrap());
}
