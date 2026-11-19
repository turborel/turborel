//! Serializable reads of distance windows and outer joins through a
//! geometry index record the widened window they read (D134, D135).

use super::populated;
use crate::common::{rows, serializable};
use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;

/// The serializable database of `populated`, its table WINDOWS of one point
/// at (600.5, 900.5), and a session that read `query` through SHAPES_G,
/// its plan showing `shown`, and then wrote a row of its own, so that its
/// commit is checked.
fn reader(query: &str, shown: &str) -> (Database, Session) {
    let db = populated();
    let s = db.session("A");
    s.run("CREATE TABLE WINDOWS (ID INTEGER, G GEOMETRY(4326))").unwrap();
    s.run("INSERT INTO WINDOWS VALUES (10, ST_POINT(600.5, 900.5, 4326)), (11, NULL)").unwrap();
    s.commit().unwrap();
    serializable(&db);
    let a = db.session("A");
    let plan = a.physical_plan(query, &Params::new()).unwrap();
    assert!(plan.contains(shown) && plan.contains("SHAPES_G"), "{plan}");
    rows(&a, query).unwrap();
    a.run("INSERT INTO WINDOWS VALUES (20, NULL)").unwrap();
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

fn insert(x: f64, y: f64) -> String {
    format!("INSERT INTO SHAPES VALUES (5000, ST_POINT({x}, {y}, 4326))")
}

/// Verifies: [DESIGN] D29, D134, D135: a distance filter through the
/// geometry index reads the value's envelope widened by the limit, so a
/// write within the limit, or in the window's corner beyond it, conflicts,
/// and one beyond the window does not.
#[test]
fn distance_reads_record_their_widened_window() {
    let query = "SELECT ID FROM SHAPES WHERE ST_DISTANCE(G, ST_POINT(300.5, 900.5, 4326)) <= 2";
    for (x, y, commits) in [(302.0, 900.5, false), (302.3, 902.3, false), (303.0, 900.5, true), (300.5, 897.0, true)] {
        let (db, a) = reader(query, "IndexScan");
        assert_eq!(commits_after(&db, &a, &insert(x, y)), commits, "({x}, {y})");
    }
}

/// Verifies: [DESIGN] D29, D134, D135: each probe of a LEFT outer join on
/// a distance reads its outer row's widened window, and a null outer
/// geometry reads none; a FULL join then reads the whole table for the
/// rows no probe matched, so even a write far from every window conflicts.
#[test]
fn outer_join_reads_record_their_widened_windows() {
    let left = "SELECT W.ID, P.ID FROM WINDOWS W LEFT JOIN SHAPES P ON ST_DISTANCE(P.G, W.G) < 1";
    for (x, y, commits) in [(601.2, 900.5, false), (602.0, 900.5, true), (50.5, 20.5, true)] {
        let (db, a) = reader(left, "LeftOuterIndexNestedLoop");
        assert_eq!(commits_after(&db, &a, &insert(x, y)), commits, "({x}, {y})");
    }
    let full = "SELECT W.ID, P.ID FROM WINDOWS W FULL JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1";
    let (db, a) = reader(full, "FullOuterIndexNestedLoop");
    assert!(!commits_after(&db, &a, &insert(50.5, 20.5)), "a FULL join reads every row");
}
