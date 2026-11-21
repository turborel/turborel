//! INFORMATION_SCHEMA.TABLE_STATISTICS and INDEX_STATISTICS (RULES.md
//! D522): each base table's rows, the bytes it takes, when it was last
//! changed and compacted; each index's searches, the entries they gave and
//! what its method knows of its size; and who sees which (RULES.md D520).

use std::sync::Arc;

use turborel::clock::FixedClock;
use turborel::db::Database;

use crate::common::rows;

/// The instant the clock stands at: 2024-02-29 12:34:56.789012 UTC.
const NOW: i64 = 1_709_210_096_789_012;

/// A database whose owner is OWNER, its clock at `NOW`, with X's tables T
/// of 1,000 rows, an index on K, and U of 10 rows.
fn owned() -> (Database, Arc<FixedClock>) {
    let db = Database::new();
    let clock = Arc::new(FixedClock::at(NOW));
    db.set_clock(Arc::clone(&clock) as Arc<dyn turborel::clock::Clock>).unwrap();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER, V CHARACTER VARYING(20)) CREATE TABLE U (K INTEGER)").unwrap();
    let x = db.session("X");
    x.run("CREATE INDEX T_K ON T (K)").unwrap();
    let values: Vec<String> = (0..1000).map(|k| format!("({k}, 'v{k}')")).collect();
    x.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    x.run("INSERT INTO U VALUES (1), (2), (3), (4), (5), (6), (7), (8), (9), (10)").unwrap();
    x.commit().unwrap();
    (db, clock)
}

/// T's row of INDEX_STATISTICS, as the owner sees it: its searches, the
/// entries they gave, its entries and its height.
fn uses(db: &Database) -> Vec<String> {
    let sql = "SELECT SEARCHES, ENTRIES_READ, ENTRY_COUNT, HEIGHT, BYTES FROM INFORMATION_SCHEMA.INDEX_STATISTICS WHERE INDEX_NAME = 'T_K'";
    rows(&db.session("OWNER"), sql).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.1]; [DESIGN] RULES.md
/// D522: a table's rows and engine; a row store's bytes not known before
/// its statistics are gathered, then its rows times their width,
/// estimated; an empty table none, counted; when a transaction that wrote
/// it last committed, unchanged by one rolled back, and none for a table
/// never written.
#[test]
fn a_tables_rows_bytes_and_last_change_are_shown() {
    let (db, clock) = owned();
    let owner = db.session("OWNER");
    let sql = "SELECT TABLE_NAME, ENGINE_NAME, ROW_COUNT, BYTES, BYTES_ESTIMATED, LAST_CHANGED IS NULL, LAST_COMPACTED \
               FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_SCHEMA = 'X' ORDER BY TABLE_NAME";
    assert_eq!(rows(&owner, sql).unwrap(), ["T,rowstore,1000,NULL,NULL,FALSE,NULL", "U,rowstore,10,NULL,NULL,FALSE,NULL"], "no width yet");
    db.schema("CREATE SCHEMA AUTHORIZATION E CREATE TABLE EMPTY (K INTEGER)").unwrap();
    let empty =
        "SELECT ROW_COUNT, BYTES, BYTES_ESTIMATED, LAST_CHANGED FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_NAME = 'EMPTY'";
    assert_eq!(rows(&owner, empty).unwrap(), ["0,0,NO,NULL"], "nothing held, never written");
    let x = db.session("X");
    db.analyze("X.T").unwrap();
    let bytes = "SELECT ROW_COUNT, BYTES > 1000, BYTES_ESTIMATED FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_NAME = 'T'";
    assert_eq!(rows(&owner, bytes).unwrap(), ["1000,TRUE,YES"], "estimated from the width gathered");
    let changed = "SELECT LAST_CHANGED FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_NAME = 'T'";
    let first = rows(&owner, changed).unwrap();
    assert_eq!(first, ["2024-02-29 12:34:56.789012+00:00"], "written and committed");
    clock.set(NOW + 60_000_000);
    x.run("DELETE FROM T WHERE K = 0").unwrap();
    x.rollback().unwrap();
    assert_eq!(rows(&owner, changed).unwrap(), first, "a rolled back write changes nothing");
    x.run("DELETE FROM T WHERE K = 0").unwrap();
    x.commit().unwrap();
    let later =
        "SELECT LAST_CHANGED > TIMESTAMP '2024-02-29 12:35:00+00:00' FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_NAME = 'T'";
    assert_eq!(rows(&owner, later).unwrap(), ["TRUE"], "its commit's instant");
}

/// Verifies: [DESIGN] RULES.md D522: a column store's frozen groups are
/// counted by their pages, and the maintenance that froze them stamps
/// when it was last compacted.
#[test]
fn a_column_stores_groups_are_counted_and_its_compaction_stamped() {
    let (db, _clock) = owned();
    db.schema("CREATE SCHEMA AUTHORIZATION C CREATE TABLE G (K INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)").unwrap();
    let c = db.session("C");
    // Two INSERTs, neither a load, whose rows a load would freeze itself.
    for first in [0, 2048] {
        let values: Vec<String> = (first..first + 2048).map(|k| format!("({k})")).collect();
        c.run(&format!("INSERT INTO G VALUES {}", values.join(", "))).unwrap();
    }
    c.commit().unwrap();
    let sql = "SELECT ROW_COUNT, BYTES > 0, LAST_COMPACTED IS NULL FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_NAME = 'G'";
    let owner = db.session("OWNER");
    db.maintain().unwrap();
    assert_eq!(rows(&owner, sql).unwrap(), ["4096,TRUE,FALSE"], "frozen and stamped");
    let estimated = "SELECT BYTES_ESTIMATED FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_NAME = 'G'";
    assert_eq!(rows(&owner, estimated).unwrap(), ["NO"], "every row frozen, every byte counted");
}

/// Verifies: [DESIGN] RULES.md D522: an index scan counts one search and
/// each entry it gave; an index join one search a probe; a B-link tree's
/// entries and height are known, its bytes not; a dropped index's counts
/// go with it, one made again under its name counting afresh.
#[test]
fn an_indexs_searches_and_entries_are_counted() {
    let (db, _clock) = owned();
    assert_eq!(uses(&db), ["0,0,1000,2,NULL"], "never searched");
    let x = db.session("X");
    assert_eq!(rows(&x, "SELECT V FROM T WHERE K = 5").unwrap(), ["v5"]);
    assert_eq!(rows(&x, "SELECT COUNT(*) FROM T WHERE K BETWEEN 10 AND 19").unwrap(), ["10"]);
    assert_eq!(uses(&db), ["2,11,1000,2,NULL"], "two scans, eleven entries");
    assert_eq!(rows(&x, "SELECT COUNT(*) FROM U JOIN T ON T.K = U.K").unwrap(), ["10"]);
    assert_eq!(uses(&db), ["12,21,1000,2,NULL"], "a probe of each of U's ten rows");
    x.commit().unwrap();
    x.run("DROP INDEX T_K").unwrap();
    x.run("CREATE INDEX T_K ON T (K)").unwrap();
    assert_eq!(uses(&db), ["0,0,1000,2,NULL"], "made again");
}

/// Verifies: [DESIGN] RULES.md D520, D522: the owner sees every table's
/// and index's row; another user those of the tables on which it holds a
/// privilege; one granted the owner's role MONITOR every one. Pairs:
/// (owner, privilege, role) = (yes, -, -), (no, none, none), (no, SELECT
/// on U, none), (no, none, MONITOR).
#[test]
fn a_user_sees_the_tables_it_holds_a_privilege_on() {
    let (db, _clock) = owned();
    db.create_user("Y", b"x").unwrap();
    let tables = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_SCHEMA = 'X' ORDER BY TABLE_NAME";
    let indexes = "SELECT INDEX_NAME FROM INFORMATION_SCHEMA.INDEX_STATISTICS WHERE TABLE_SCHEMA = 'X'";
    assert_eq!(rows(&db.session("OWNER"), tables).unwrap(), ["T", "U"]);
    let y = db.session("Y");
    assert_eq!(rows(&y, tables).unwrap(), Vec::<String>::new(), "no privilege");
    assert_eq!(rows(&y, indexes).unwrap(), Vec::<String>::new());
    let x = db.session("X");
    x.run("GRANT SELECT ON U TO Y").unwrap();
    x.commit().unwrap();
    assert_eq!(rows(&y, tables).unwrap(), ["U"], "the table granted");
    assert_eq!(rows(&y, indexes).unwrap(), Vec::<String>::new(), "U has no index");
    let owner = db.session("OWNER");
    owner.run("CREATE ROLE MONITOR").unwrap();
    owner.run("GRANT MONITOR TO Y").unwrap();
    owner.commit().unwrap();
    assert_eq!(rows(&y, tables).unwrap(), ["T", "U"], "MONITOR shows them all");
    assert_eq!(rows(&y, indexes).unwrap(), ["T_K"]);
}
