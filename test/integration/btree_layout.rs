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

/// Verifies: [DESIGN] RULES.md D34, D501: an index holds the entries of
/// versions a reader may not see, and a row found by one is kept only if
/// the version the reader sees has the entry's key, checked in place. A
/// reader at a snapshot before an update of an indexed column finds the
/// row by its old key and not its new one, by equality, by a range and by
/// a key not unique; a reader after it, the other way.
#[test]
fn a_row_is_found_by_the_key_its_seen_version_has() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER NOT NULL, K INTEGER, G INTEGER, PRIMARY KEY (ID))").unwrap();
    db.create_index("T_K", "APP.T", &["K"], "btree").unwrap();
    db.create_index("T_G", "APP.T", &["G", "K"], "btree").unwrap();
    let s = db.session("APP");
    for id in 1..=200 {
        s.run(&format!("INSERT INTO T VALUES ({id}, {id}, {})", id % 4)).unwrap();
    }
    s.commit().unwrap();
    let old = db.session("APP");
    old.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    let probes = [
        "SELECT ID FROM T WHERE K = 50",
        "SELECT ID FROM T WHERE K = 5000",
        "SELECT ID FROM T WHERE K BETWEEN 49 AND 51",
        "SELECT COUNT(*) FROM T WHERE G = 2",
        "SELECT ID FROM T WHERE G = 2 AND K = 50",
    ];
    let before: Vec<Vec<String>> = probes.iter().map(|q| sorted(&old, q)).collect();
    assert_eq!(before, [vec!["50"], vec![], vec!["49", "50", "51"], vec!["50"], vec!["50"]]);
    s.run("UPDATE T SET K = 5000, G = 3 WHERE ID = 50").unwrap();
    s.commit().unwrap();
    let after: Vec<Vec<String>> = probes.iter().map(|q| sorted(&old, q)).collect();
    assert_eq!(after, before, "the snapshot's versions, by their keys");
    let new = db.session("APP");
    let seen: Vec<Vec<String>> = probes.iter().map(|q| sorted(&new, q)).collect();
    assert_eq!(seen, [vec![], vec!["50"], vec!["49", "51"], vec!["49"], vec![]]);
    let plan = new.physical_plan(probes[0], &Params::new()).unwrap();
    assert!(plan.contains("T_K"), "{plan}");
}

/// The key of row `id` of group `g`: 1,690 bytes 'p' then its number, or,
/// for `id` 0, 845 bytes 'p' then 'q': above every other key of the group,
/// sharing half their prefix.
fn long_value(id: i32) -> String {
    if id == 0 { format!("{}q", "p".repeat(845)) } else { format!("{}{id:06}", "p".repeat(1_690)) }
}

/// What the groups `groups` of LONGV answer in `s`: each group's rows by
/// the index, and its high key found by equality.
fn long_answers(s: &Session, groups: std::ops::RangeInclusive<i32>) -> Vec<Vec<String>> {
    let mut found = vec![sorted(s, "SELECT COUNT(*) FROM LONGV")];
    for g in groups {
        found.push(sorted(s, &format!("SELECT COUNT(*) FROM LONGV WHERE G = {g} AND V >= 'p'")));
        found.push(sorted(s, &format!("SELECT K FROM LONGV WHERE G = {g} AND V = '{}'", long_value(0))));
    }
    found
}

/// Verifies: [DESIGN] RULES.md D25, D500, D31: an index of keys of 1,696
/// bytes sharing all but their last few, written in order a group at a
/// time, so the rightmost leaf of each group holds as many of them as
/// fit, then a key above them that shares only half their prefix, which
/// splits that leaf with each part keeping its own prefix; some
/// checkpointed, the rest crashed: reopened and recovered, the index
/// finds every key of every group, as the log's changes are made again
/// and split the leaves again.
#[test]
fn leaves_split_by_keys_without_their_prefix_recover_after_a_crash() {
    let scratch = Scratch::new("btree-layout-multi");
    let dir = scratch.path("db");
    let (crashed, _) = open(&dir);
    crashed
        .schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE LONGV (K INTEGER NOT NULL PRIMARY KEY, G INTEGER NOT NULL, V VARCHAR(1700))")
        .unwrap();
    crashed.create_index("LONGV_GV", "APP.LONGV", &["G", "V"], "btree").unwrap();
    let s = crashed.session("APP");
    let mut k = 0;
    let groups = 190..=230;
    for g in groups.clone() {
        for id in (1..=g).chain([0]) {
            k += 1;
            s.run(&format!("INSERT INTO LONGV VALUES ({k}, {g}, '{}')", long_value(id))).unwrap();
        }
        s.commit().unwrap();
        if g == 210 {
            crashed.checkpoint().unwrap();
        }
    }
    let expected = long_answers(&s, groups.clone());
    assert_eq!(expected[0], [(groups.clone().map(|g| g + 1).sum::<i32>()).to_string()]);

    let (db, opened) = open(&dir);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(long_answers(&db.session("APP"), groups), expected);
    drop((db, s, crashed));
}
