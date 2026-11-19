//! Serializable reads through a geometry index record the envelopes they
//! read, not the whole table (D134).

use super::populated;
use crate::common::{rows, serializable};
use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;

/// A read of the points of SHAPES near (100, 100), through its index.
const NEAR: &str = "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_MAKEENVELOPE(99.5, 99.5, 101.5, 101.5, 4326)) = 1 ORDER BY ID";

/// The serializable database of `populated`, and a session that read
/// `query` through the index and then wrote a row of its own, so that its
/// commit is checked.
fn reader(query: &str) -> (Database, Session) {
    let db = populated();
    serializable(&db);
    let a = db.session("A");
    let plan = a.physical_plan(query, &Params::new()).unwrap();
    assert!(plan.contains("SHAPES_G"), "{plan}");
    rows(&a, query).unwrap();
    a.run("INSERT INTO SHAPES VALUES (9000, NULL)").unwrap();
    (db, a)
}

/// Whether `a` commits after another session, its snapshot earlier, runs
/// `write` and commits: false if `a` is refused as not serializable.
fn commits_after(db: &Database, a: &Session, write: &str) -> bool {
    let b = db.session("A");
    b.run(write).unwrap();
    b.commit().unwrap();
    match a.commit() {
        Ok(()) => true,
        Err(error) => {
            assert_eq!(error.code, Code::Conflict, "{error:?}");
            false
        }
    }
}

/// Verifies: [DESIGN] D29, D134: a serializable read through a geometry
/// index conflicts with a later commit that adds a row whose envelope
/// meets the window it read, and no longer with one that adds a row
/// elsewhere, as the whole-table read it was before did.
#[test]
fn geometry_reads_conflict_only_with_writes_in_their_window() {
    let (db, a) = reader(NEAR);
    assert!(!commits_after(&db, &a, "INSERT INTO SHAPES VALUES (5000, ST_POINT(100.5, 100.5, 4326))"), "inside");
    let (db, a) = reader(NEAR);
    let line = "INSERT INTO SHAPES VALUES (5000, ST_GEOMFROMTEXT('LINESTRING (90 101, 99.5 120)', 4326))";
    assert!(!commits_after(&db, &a, line), "an envelope meeting the window's edge");
    let (db, a) = reader(NEAR);
    assert!(commits_after(&db, &a, "INSERT INTO SHAPES VALUES (5000, ST_POINT(500, 500, 4326))"), "outside");
    let (db, a) = reader(NEAR);
    assert!(commits_after(&db, &a, "DELETE FROM SHAPES WHERE ID = 700"), "a deletion outside");
    let (db, a) = reader(NEAR);
    assert!(!commits_after(&db, &a, "DELETE FROM SHAPES WHERE ID = 101"), "a deletion inside");
}

/// Verifies: [DESIGN] D29, D134: an update that moves a geometry into the
/// window read, or out of it, conflicts, as either version meets it; one
/// that moves a geometry outside it to elsewhere outside it does not.
#[test]
fn geometry_moves_across_a_region_border_conflict() {
    let (db, a) = reader(NEAR);
    assert!(!commits_after(&db, &a, "UPDATE SHAPES SET G = ST_POINT(100.5, 100.5, 4326) WHERE ID = 700"), "moved in");
    let (db, a) = reader(NEAR);
    assert!(!commits_after(&db, &a, "UPDATE SHAPES SET G = ST_POINT(700, 700, 4326) WHERE ID = 100"), "moved out");
    let (db, a) = reader(NEAR);
    assert!(commits_after(&db, &a, "UPDATE SHAPES SET G = ST_POINT(800.5, 3, 4326) WHERE ID = 700"), "moved outside");
}

/// Verifies: [DESIGN] D85, D134: a NULL geometry is in no region; an
/// empty one is matched where the index keys it, at the origin, so it
/// conflicts with a window holding the origin only. A read of a NULL or
/// empty window reads no row, and conflicts with nothing.
#[test]
fn geometry_regions_handle_nulls_and_empty_geometries() {
    let empty = "INSERT INTO SHAPES VALUES (5000, ST_GEOMFROMTEXT('POINT EMPTY', 4326))";
    let (db, a) = reader(NEAR);
    assert!(commits_after(&db, &a, "INSERT INTO SHAPES VALUES (5000, NULL)"), "a NULL");
    let (db, a) = reader(NEAR);
    assert!(commits_after(&db, &a, empty), "an empty geometry, the origin outside");
    let origin = "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_MAKEENVELOPE(-0.5, -0.5, 0.5, 0.5, 4326)) = 1 ORDER BY ID";
    let (db, a) = reader(origin);
    assert!(!commits_after(&db, &a, empty), "an empty geometry, the origin inside");
    for nothing in [
        "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, CAST(NULL AS GEOMETRY(4326))) = 1",
        "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_GEOMFROMTEXT('POINT EMPTY', 4326)) = 1",
    ] {
        let (db, a) = reader(nothing);
        assert!(commits_after(&db, &a, "INSERT INTO SHAPES VALUES (5000, ST_POINT(0, 0, 4326))"), "{nothing}");
    }
}

/// Verifies: [DESIGN] D29, D85, D134: each probe of an inner join through
/// the geometry index records its outer row's envelope. A write meeting
/// one conflicts, one between them does not.
#[test]
fn geometry_join_probes_record_each_window() {
    let join = |db: &Database| {
        let s = db.session("A");
        s.run("CREATE TABLE WINDOWS (ID INTEGER, G GEOMETRY(4326))").unwrap();
        s.run("INSERT INTO WINDOWS VALUES (10, ST_MAKEENVELOPE(300.2, 300.2, 300.8, 300.8, 4326)), (11, ST_POINT(900.5, 900.5, 4326)), (12, NULL)").unwrap();
        s.commit().unwrap();
    };
    let query = "SELECT P.ID, W.ID FROM WINDOWS W JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1";
    for (write, commits) in [
        ("INSERT INTO SHAPES VALUES (5000, ST_POINT(300.5, 300.5, 4326))", false),
        ("INSERT INTO SHAPES VALUES (5000, ST_POINT(900.5, 900.5, 4326))", false),
        ("INSERT INTO SHAPES VALUES (5000, ST_POINT(600.5, 600.5, 4326))", true),
    ] {
        let db = populated();
        join(&db);
        serializable(&db);
        let a = db.session("A");
        let plan = a.physical_plan(query, &Params::new()).unwrap();
        assert!(plan.contains("IndexNestedLoop") && plan.contains("SHAPES_G"), "{plan}");
        assert_eq!(rows(&a, query).unwrap(), Vec::<String>::new());
        a.run("INSERT INTO WINDOWS VALUES (20, NULL)").unwrap();
        assert_eq!(commits_after(&db, &a, write), commits, "{write}");
    }
}

/// Verifies: [DESIGN] D38, D134: past TRANSACTIONS.TRACKED_ROWS envelopes,
/// a transaction's regions merge into their hull. A write between the
/// windows read, inside the hull, then conflicts; one beyond it does not;
/// within the budget, the one between them does not either.
#[test]
fn geometry_regions_past_the_budget_merge_into_their_hull() {
    let windows = [(100.2, 100.8), (200.2, 200.8), (400.2, 400.8)];
    for (tracked, write, commits) in [
        (2, "INSERT INTO SHAPES VALUES (5000, ST_POINT(300.5, 300.5, 4326))", false),
        (2, "INSERT INTO SHAPES VALUES (5000, ST_POINT(450.5, 450.5, 4326))", true),
        (3, "INSERT INTO SHAPES VALUES (5000, ST_POINT(300.5, 300.5, 4326))", true),
    ] {
        let db = populated();
        db.session("A").run(&format!("ALTER TABLE SHAPES SET (TRANSACTIONS.TRACKED_ROWS = {tracked})")).unwrap();
        serializable(&db);
        let a = db.session("A");
        for (low, high) in windows {
            let query = format!("SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_MAKEENVELOPE({low}, {low}, {high}, {high}, 4326)) = 1");
            assert_eq!(rows(&a, &query).unwrap(), Vec::<String>::new());
        }
        a.run("INSERT INTO SHAPES VALUES (9000, NULL)").unwrap();
        assert_eq!(commits_after(&db, &a, write), commits, "{tracked} {write}");
    }
}

/// Verifies: [DESIGN] D38, D134: once a committed transaction's changes
/// pass TRANSACTIONS.HELD_BYTES and are kept as each column's least and
/// greatest value, every region may match them, so even changes outside
/// the window read refuse the reader; within the bytes held they do not.
#[test]
fn geometry_regions_match_changes_kept_as_bounds() {
    let far: Vec<String> = (0..300).map(|i| format!("({}, ST_POINT({}, -50, 4326))", 5000 + i, 5000 + i)).collect();
    let write = format!("INSERT INTO SHAPES VALUES {}", far.join(","));
    for (held, commits) in [(None, true), (Some(4096), false)] {
        let db = populated();
        if let Some(held) = held {
            db.session("A").run(&format!("ALTER TABLE SHAPES SET (TRANSACTIONS.HELD_BYTES = {held})")).unwrap();
        }
        serializable(&db);
        let a = db.session("A");
        rows(&a, NEAR).unwrap();
        a.run("INSERT INTO SHAPES VALUES (9000, NULL)").unwrap();
        assert_eq!(commits_after(&db, &a, &write), commits, "{held:?}");
    }
}
