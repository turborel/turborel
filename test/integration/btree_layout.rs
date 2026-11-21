//! The B-link node layout of RULES.md D500 across a crash: an index whose
//! keys share long prefixes, and a table read through its entries' hints,
//! written back to the data file as a small pool needs room, checkpointed,
//! then changed and crashed; reopened, the indexes find exactly what they
//! found before the crash, by whole keys and by ranges.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Opened, Session};
use turborel::params::Params;

use crate::common::{Scratch, after_crash, number, rows, small_pool, sorted};

/// The database in `dir`, with a small pool, so pages go to the data file
/// as it runs, and checkpoints only when asked.
fn open(dir: &Path) -> (Database, Opened) {
    let db = Database::new();
    small_pool(&db, 32);
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
    db.set_setting("CHECKPOINT.AFTER_BYTES", &number(i64::MAX)).unwrap();
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    (db, opened)
}

/// Order lines of warehouses 1 and 2, ten districts each: a key of four
/// columns whose first two are shared by every key of a leaf.
fn fill(s: &Session, orders: std::ops::Range<i32>) {
    for w in 1..=2 {
        for d in 1..=10 {
            for o in orders.clone() {
                let lines: Vec<String> = (1..=5).map(|n| format!("({w}, {d}, {o}, {n}, 'item {:04}')", (o * 7 + n) % 97)).collect();
                s.run(&format!("INSERT INTO OL VALUES {}", lines.join(", "))).unwrap();
            }
        }
    }
    s.commit().unwrap();
}

/// What the queries of the test answer in `s`.
fn answers(s: &Session) -> Vec<Vec<String>> {
    [
        "SELECT V FROM OL WHERE W = 2 AND D = 7 AND O = 25 AND N = 3",
        "SELECT O, N FROM OL WHERE W = 1 AND D = 10 AND O BETWEEN 8 AND 14",
        "SELECT W, D, O, N FROM OL WHERE V = 'item 0040'",
        "SELECT COUNT(*) FROM OL",
        "SELECT COUNT(*) FROM OL WHERE V > 'item 0090'",
    ]
    .iter()
    .map(|q| sorted(s, q))
    .collect()
}

/// Verifies: [DESIGN] RULES.md D25, D500: indexes of four-column keys, nodes
/// keeping their shared prefixes and key heads, and a table read by its
/// entries' hints, checkpointed, then changed by inserts, deletes that
/// empty leaves, and updates of an indexed column, then crashed: reopened
/// and recovered, a key lookup, a range, a lookup by the second index and
/// counts answer as before the crash, through the indexes.
#[test]
fn prefixed_nodes_and_hinted_rows_recover_after_a_crash() {
    let scratch = Scratch::new("btree-layout-crash");
    let dir = scratch.path("db");
    let (crashed, _) = open(&dir);
    crashed
        .schema(
            "CREATE SCHEMA AUTHORIZATION APP CREATE TABLE OL (W INTEGER NOT NULL, D INTEGER NOT NULL, O INTEGER NOT NULL, \
             N INTEGER NOT NULL, V VARCHAR(30), PRIMARY KEY (W, D, O, N))",
        )
        .unwrap();
    crashed.create_index("OL_V", "APP.OL", &["V", "W"], "btree").unwrap();
    let s = crashed.session("APP");
    fill(&s, 1..20);
    crashed.checkpoint().unwrap();
    fill(&s, 20..40);
    s.run("DELETE FROM OL WHERE O BETWEEN 5 AND 12").unwrap();
    s.run("UPDATE OL SET V = 'item 0040' WHERE O = 30 AND N = 1").unwrap();
    s.commit().unwrap();
    s.run("DELETE FROM OL WHERE W = 1").unwrap();
    let before = answers(&s);
    s.rollback().unwrap();
    let expected = answers(&s);
    assert_ne!(before, expected, "the rolled back delete was seen by its own transaction");
    assert_eq!(expected[3], ["3100"], "2 x 10 x (39 - 8) x 5");

    let (db, opened) = open(&dir);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    let r = db.session("APP");
    assert_eq!(answers(&r), expected);
    let plan = r.physical_plan("SELECT V FROM OL WHERE W = 2 AND D = 7 AND O = 25 AND N = 3", &Params::new()).unwrap();
    assert!(plan.contains("Index"), "{plan}");
    assert_eq!(rows(&r, "SELECT COUNT(*) FROM OL WHERE W = 1 AND D = 1").unwrap(), ["155"]);
    drop((db, s, crashed));
}
