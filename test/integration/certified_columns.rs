//! \[DESIGN\] SERIALIZABLE certified by column (RULES.md D448): a read
//! conflicts with a write that committed meanwhile only if the write
//! changed a column the read consumed, so a transaction reading one
//! column of a row commits beside one that changed another, as an order
//! entry reading a warehouse's tax does beside a payment adding to its
//! year-to-date total; one reading the changed column, by id, by a scan,
//! or by the order a scan ranks rows in, still fails, and an insertion
//! still meets every read of its table.

use turborel::db::{Database, Session};
use turborel::params::Params;

/// W (ID, TAX, YTD) keyed by ID, with an index on TAX, holding warehouse
/// 1 (TAX 5, YTD 100), 2 (TAX 7, YTD 200) and more of YTD 0, enough that
/// a read of one goes through an index; and H, a history the readers
/// write to.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A \
         CREATE TABLE W (ID INTEGER NOT NULL PRIMARY KEY, TAX INTEGER, YTD INTEGER) \
         CREATE TABLE H (N INTEGER)",
    )
    .unwrap();
    db.create_index("W_TAX", "A.W", &["TAX"], "btree").unwrap();
    let s = db.session("A");
    let more: Vec<String> = (3..300).map(|i| format!("({i}, {}, 0)", i + 1000)).collect();
    s.run(&format!("INSERT INTO W VALUES (1, 5, 100), (2, 7, 200), {}", more.join(", "))).unwrap();
    s.commit().unwrap();
    db
}

/// A session of `db` whose transaction is SERIALIZABLE.
fn serializable(db: &Database) -> Session {
    let s = db.session("A");
    s.run("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").unwrap();
    s
}

/// Whether a transaction that ran `read`, while another changed
/// warehouse 1 by `change` and committed, then wrote H, commits.
fn commits(read: &str, change: &str) -> bool {
    let db = database();
    let (reader, writer) = (serializable(&db), serializable(&db));
    let plan = reader.physical_plan(read, &Params::new()).unwrap();
    assert!(!read.contains("WHERE ID") || plan.contains("Index"), "by the key's index: {plan}");
    assert!(!read.contains("WHERE TAX") || plan.contains("W_TAX"), "by W_TAX: {plan}");
    crate::common::rows(&reader, read).unwrap();
    writer.run(change).unwrap();
    writer.commit().unwrap();
    reader.run("INSERT INTO H VALUES (1)").unwrap();
    reader.commit().is_ok()
}

/// Verifies: [DESIGN] RULES.md D448: a read of TAX by the key commits
/// beside a change of YTD; of YTD, it does not.
#[test]
fn a_read_by_key_meets_only_the_columns_it_consumed() {
    let pay = "UPDATE W SET YTD = YTD + 10 WHERE ID = 1";
    assert!(commits("SELECT TAX FROM W WHERE ID = 1", pay), "TAX was not changed");
    assert!(!commits("SELECT YTD FROM W WHERE ID = 1", pay), "YTD was");
    assert!(!commits("SELECT TAX FROM W WHERE ID = 1", "UPDATE W SET TAX = 6 WHERE ID = 1"));
}

/// Verifies: [DESIGN] RULES.md D448: a scan of every row's TAX commits
/// beside a change of YTD, as does one whose condition the changed row
/// stays out of; one whose condition it moves into, whose order reads
/// YTD, or one beside an insertion or a deletion, does not.
#[test]
fn a_scan_meets_the_columns_its_condition_and_order_read() {
    let pay = "UPDATE W SET YTD = YTD + 10 WHERE ID = 1";
    assert!(commits("SELECT SUM(TAX) FROM W", pay));
    assert!(commits("SELECT SUM(TAX) FROM W WHERE YTD > 150", pay), "row 1 stayed out of the condition");
    let into = "UPDATE W SET YTD = YTD + 100 WHERE ID = 1";
    assert!(!commits("SELECT SUM(TAX) FROM W WHERE YTD > 150", into), "row 1 moved into it");
    assert!(!commits("SELECT ID FROM W ORDER BY YTD FETCH FIRST 1 ROW ONLY", pay), "the order read YTD");
    assert!(!commits("SELECT SUM(TAX) FROM W", "INSERT INTO W VALUES (300, 1, 0)"), "a phantom");
    assert!(!commits("SELECT COUNT(*) FROM W", "DELETE FROM W WHERE ID = 2"), "a row gone");
}

/// Verifies: [DESIGN] RULES.md D448: a probe of the index on TAX commits
/// beside a change of YTD, and not beside one moving a row into its range.
#[test]
fn an_index_probe_meets_its_key_columns() {
    assert!(commits("SELECT ID FROM W WHERE TAX = 5", "UPDATE W SET YTD = 0 WHERE ID = 1"));
    assert!(!commits("SELECT ID FROM W WHERE TAX = 7", "UPDATE W SET TAX = 7 WHERE ID = 1"), "a row moved into it");
}

/// Verifies: [DESIGN] RULES.md D448, D29: write skew still fails, its
/// reads and writes meeting on the column both read.
#[test]
fn write_skew_still_fails() {
    let db = database();
    let (a, b) = (serializable(&db), serializable(&db));
    assert_eq!(crate::common::rows(&a, "SELECT SUM(YTD) FROM W").unwrap(), vec!["300".to_owned()]);
    assert_eq!(crate::common::rows(&b, "SELECT SUM(YTD) FROM W").unwrap(), vec!["300".to_owned()]);
    a.run("UPDATE W SET YTD = 0 WHERE ID = 1").unwrap();
    b.run("UPDATE W SET YTD = 0 WHERE ID = 2").unwrap();
    a.commit().unwrap();
    assert!(b.commit().is_err());
}
