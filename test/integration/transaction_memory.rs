//! The memory a transaction holds (RULES.md D38): the coordinator's undo
//! entries, for engines that do not undo their own changes and for
//! indexes, are held up to WORK_MEMORY, then written to runs of a spill
//! area reserved of the memory budget, and read back to undo a statement
//! or the transaction [X3.135-1986 §7.3 GR3b i, §8.9 GR1].

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::{Decimal, Value};

use crate::common::{exec, locking_database, rows, spill_bytes};

/// The smallest WORK_MEMORY, 64 KiB.
const SMALL: i128 = 64 << 10;
/// Rows of `T`: each changed adds undo entries for its row and two indexes.
const ROWS: usize = 3000;

/// `db` with `T (K, V)` of `ROWS` rows, K unique and V indexed, committed.
fn filled(db: &Database) {
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHARACTER(40))").unwrap();
    db.create_index("T_V", "S.T", &["V"], "btree").unwrap();
    let s = db.session("S");
    for from in (0..ROWS).step_by(500) {
        let values: Vec<String> = (from..from + 500).map(|k| format!("({k}, 'v{k}')")).collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
}

/// Verifies: [X3.135-1986 §7.3 GR3b i, §8.9 GR1]; [DESIGN] RULES.md D38:
/// a transaction's undo entries past WORK_MEMORY are written to a spill
/// area reserved of the budget; a statement that fails after them is
/// undone, its changes read back from the area, leaving those before it;
/// ROLLBACK undoes the rest, rows and index entries; the area is given
/// back as the transaction ends; whatever undoes the rows, the engine or
/// the coordinator.
#[test]
fn undo_entries_spill_and_are_read_back() {
    for db in [Database::new(), locking_database()] {
        filled(&db);
        db.set_setting("WORK_MEMORY", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
        let s = db.session("S");
        s.run("UPDATE T SET V = 'changed'").unwrap();
        assert!(spill_bytes(&db) > 0, "an area reserved");
        // Every K the same: a UNIQUE violation at the statement's end.
        assert!(exec(&s, "UPDATE T SET K = 7").is_err());
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE V = 'changed'"), Ok(vec![ROWS.to_string()]));
        assert_eq!(rows(&s, "SELECT COUNT(DISTINCT K) FROM T"), Ok(vec![ROWS.to_string()]), "the failed statement undone");
        s.rollback().unwrap();
        assert_eq!(spill_bytes(&db), 0, "the area given back");
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE V = 'changed'"), Ok(vec!["0".into()]));
        assert_eq!(rows(&s, "SELECT K FROM T WHERE V = 'v2999'"), Ok(vec!["2999".into()]), "the index as it was");
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE K = 7"), Ok(vec!["1".into()]));
    }
}

/// Two sessions of a database of `T (K, V)`, K unique, of a thousand rows,
/// tracking `tracked` rows of a transaction's reads or writes.
fn tracking(tracked: u32) -> (Database, Session, Session) {
    let db = Database::new();
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER) WITH (TRANSACTIONS.TRACKED_ROWS = {tracked})"
    ))
    .unwrap();
    let s = db.session("S");
    let values: Vec<String> = (0..1000).map(|k| format!("({k}, 0)")).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    let other = db.session("S");
    (db, s, other)
}

/// Whether `s`, having read `T` where `K` is each of `read`, then written
/// row 500 while `other` wrote rows `wrote` and committed, commits.
fn commits(s: &Session, other: &Session, (read, wrote): (&[i32], &[i32])) -> Result<(), Code> {
    for k in read {
        rows(s, &format!("SELECT V FROM T WHERE K = {k}")).unwrap();
    }
    for k in wrote {
        other.run(&format!("UPDATE T SET V = V + 1 WHERE K = {k}")).unwrap();
    }
    other.commit().unwrap();
    s.run("UPDATE T SET V = V + 1 WHERE K = 500").unwrap();
    s.commit().map_err(|e| e.code)
}

/// Verifies: [X3.135-1986 §4.16]; [DESIGN] RULES.md D38, the row store's
/// TRANSACTIONS.TRACKED_ROWS: past it, a transaction's reads of a table
/// count as reading every row, and a transaction's writes as writing every
/// row, so a commit another's change of rows it never read refuses (SQLCODE
/// -911); within it, they are tracked row by row, and it commits; the
/// option changes in place.
#[test]
fn reads_and_writes_past_the_rows_tracked_count_as_every_row() {
    let (db, s, other) = tracking(2);
    assert_eq!(commits(&s, &other, (&[1, 2, 3], &[9])), Err(Code::Conflict), "its reads");
    assert_eq!(commits(&s, &other, (&[1], &[6, 7, 8])), Err(Code::Conflict), "the other's writes");
    s.run("ALTER TABLE T SET (TRANSACTIONS.TRACKED_ROWS = 100)").unwrap();
    s.commit().unwrap();
    assert_eq!(commits(&s, &other, (&[1, 2, 3], &[9])), Ok(()));
    assert_eq!(commits(&s, &other, (&[1], &[6, 7, 8])), Ok(()));
    drop(db);
}

/// Verifies: [X3.135-1986 §7.3 GR3b i, §4.16]; [DESIGN] RULES.md D38, the
/// row store's TRANSACTIONS.HELD_BYTES: a transaction's changes to a table
/// past them are written to a spill area reserved of the budget; a
/// statement that fails after them is undone, read back; the rest commit,
/// and once their garbage is collected the area is given back.
#[test]
fn row_store_changes_spill_and_are_read_back() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHARACTER(40)) WITH (TRANSACTIONS.HELD_BYTES = 4096)",
    )
    .unwrap();
    let s = db.session("S");
    for from in (0..ROWS).step_by(500) {
        let values: Vec<String> = (from..from + 500).map(|k| format!("({k}, 'v{k}')")).collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    s.run("UPDATE T SET V = 'changed' WHERE K < 1500").unwrap();
    assert!(spill_bytes(&db) > 0, "an area reserved");
    assert!(exec(&s, "UPDATE T SET K = 7").is_err());
    // Every row as it was before the failed statement, each its own.
    let expected: Vec<String> = (0..ROWS).map(|k| if k < 1500 { format!("{k},changed") } else { format!("{k},v{k}") }).collect();
    let mut found: Vec<String> = rows(&s, "SELECT K, V FROM T").unwrap().iter().map(|r| r.trim_end().to_owned()).collect();
    found.sort_by_key(|r| r.split(',').next().and_then(|k| k.parse::<usize>().ok()));
    assert_eq!(found, expected, "the failed statement undone");
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE V = 'changed'"), Ok(vec!["1500".into()]));
    s.commit().unwrap();
    assert_eq!(spill_bytes(&db), 0, "the areas given back");
}

/// Verifies: [X3.135-1986 §4.16]; [DESIGN] RULES.md D38: a committed
/// transaction's changed rows, past TRANSACTIONS.HELD_BYTES, are kept as
/// each column's least and greatest value, so a probe of a key it did not
/// write, but within them, counts as a phantom (SQLCODE -911); within the
/// bytes, the rows are kept, and the same probe commits.
#[test]
fn changed_rows_past_the_bytes_held_are_kept_as_bounds() {
    for (held, expected) in [(4096, Err(Code::Conflict)), (1 << 20, Ok(()))] {
        let db = Database::new();
        db.schema(&format!(
            "CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER) WITH (TRANSACTIONS.HELD_BYTES = {held})"
        ))
        .unwrap();
        let (s, other) = (db.session("S"), db.session("S"));
        let values: Vec<String> = (0..1000).map(|k| format!("({k}, 0)")).collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
        s.commit().unwrap();
        assert_eq!(rows(&s, "SELECT V FROM T WHERE K = 1551"), Ok(vec![]));
        let evens: Vec<String> = (1500..1600).step_by(2).map(|k| format!("({k}, 1)")).collect();
        other.run(&format!("INSERT INTO T VALUES {}", evens.join(", "))).unwrap();
        other.commit().unwrap();
        s.run("UPDATE T SET V = 2 WHERE K = 500").unwrap();
        assert_eq!(s.commit().map_err(|e| e.code), expected, "{held} bytes held");
    }
}

/// Verifies: [X3.135-1986 §4.16]; [DESIGN] RULES.md D29, D38: garbage past
/// TRANSACTIONS.HELD_BYTES, kept while a reader may read it, is read as an
/// index is built a segment at a time, so the index finds what each
/// transaction sees; once the reader ends, collection removes the versions
/// and their entries as it goes, and the index has entries only of the
/// rows now.
#[test]
fn garbage_is_read_and_collected_a_segment_at_a_time() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER) WITH (TRANSACTIONS.HELD_BYTES = 4096)")
        .unwrap();
    let (s, reader) = (db.session("S"), db.session("S"));
    for from in (0..ROWS).step_by(500) {
        let values: Vec<String> = (from..from + 500).map(|k| format!("({k}, {k})")).collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    assert_eq!(rows(&reader, "SELECT K FROM T WHERE V = 17"), Ok(vec!["17".into()]));
    // Each row's first new version replaced by its second: own garbage.
    s.run("UPDATE T SET V = V + 3000").unwrap();
    s.run("UPDATE T SET V = V + 3000").unwrap();
    s.commit().unwrap();
    db.create_index("T_V", "S.T", &["V"], "btree").unwrap();
    let plan = s.explain("SELECT K FROM T WHERE V = 17", &turborel::params::Params::new()).unwrap();
    assert!(plan.contains("T_V"), "{plan}");
    assert_eq!(rows(&reader, "SELECT K FROM T WHERE V = 17"), Ok(vec!["17".into()]), "the reader's snapshot");
    assert_eq!(rows(&s, "SELECT K FROM T WHERE V = 6017"), Ok(vec!["17".into()]));
    // The version its transaction replaced, read by no one, has its entry,
    // read from the garbage, for collection to take back.
    let plan = s.explain("SELECT K FROM T WHERE V = 3017", &turborel::params::Params::new()).unwrap();
    assert!(plan.contains("est. rows=1,"), "{plan}");
    reader.commit().unwrap();
    s.commit().unwrap();
    for (v, expected) in [(17, vec![]), (3017, vec![]), (6017, vec!["17".to_owned()])] {
        assert_eq!(rows(&s, &format!("SELECT K FROM T WHERE V = {v}")), Ok(expected), "V = {v}");
    }
    // The index counts its entries for a probe: none left of the versions
    // collected, the first replaced and the one its transaction replaced.
    // Once the maintenance thread sums up the zones again, they show no
    // row holds either value, and a scan reading none costs nothing.
    for v in [17, 3017] {
        let plan = s.explain(&format!("SELECT K FROM T WHERE V = {v}"), &turborel::params::Params::new()).unwrap();
        let by_index = plan.contains("using T_V") && plan.contains("est. rows=0,");
        let by_zones = plan.contains("SeqScan S.T") && plan.contains("cost=0.0)") && plan.contains("(est. rows=0)");
        assert!(by_index || by_zones, "{plan}");
    }
}

/// `db`, in memory of a budget of 3 MiB, its pool of 1 MiB, with `T (K,
/// S)`, K unique and S indexed, of a hundred rows committed, its changes
/// and undo entries spilled past 4 KiB and 64 KiB; then, in one
/// transaction, rows inserted one a statement until the budget refuses one
/// (SQLCODE -918), that statement undone. Its session, and the rows then
/// in `T`.
fn past_the_budget(db: &Database) -> (Session, usize) {
    const MIB: i128 = 1 << 20;
    db.set_setting("MEMORY_BUDGET", &Value::Exact(Decimal::new(3 * MIB, 0))).unwrap();
    db.set_setting("STORAGE.POOL_BYTES", &Value::Exact(Decimal::new(MIB, 0))).unwrap();
    db.set_setting("WORK_MEMORY", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE, S CHARACTER(500)) WITH (TRANSACTIONS.HELD_BYTES = 4096)",
    )
    .unwrap();
    db.create_index("T_S", "S.T", &["S"], "btree").unwrap();
    let s = db.session("S");
    let values: Vec<String> = (0..100).map(|k| format!("({k}, 's{k}')")).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    let refused = (100_i32..100_000_i32).find_map(|k| s.run(&format!("INSERT INTO T VALUES ({k}, 's{k}')")).err().map(|e| (k, e)));
    let (k, error) = refused.expect("refused past the budget");
    assert_eq!(error.code, Code::Resources, "{}", error.msg);
    let k = usize::try_from(k).unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T"), Ok(vec![k.to_string()]), "the refused statement undone");
    (s, k)
}

/// Verifies: [X3.135-1986 §7.3 GR3b i, §8.9 GR1]; [DESIGN] RULES.md D19,
/// D38: undoing is never refused by the memory budget. Regression test
/// (R-TEST-7): a transaction of a database in memory that inserts a row a
/// statement until MEMORY_BUDGET refuses one (SQLCODE -918) has that
/// statement undone, the rows before kept; ROLLBACK then undoes the rest,
/// rows and index entries, damaging no table, where reading its changes
/// back to undo them was refused for want of memory and damaged it.
#[test]
fn a_transaction_past_the_budget_rolls_back() {
    let db = Database::new();
    let (s, _) = past_the_budget(&db);
    s.rollback().unwrap();
    assert_eq!(db.damaged_tables().unwrap(), vec![]);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T"), Ok(vec!["100".into()]));
    assert_eq!(rows(&s, "SELECT K FROM T WHERE S = 's99'"), Ok(vec!["99".into()]), "the index as it was");
    assert_eq!(rows(&s, "SELECT K FROM T WHERE S = 's100'"), Ok(vec![]));
    s.run("INSERT INTO T VALUES (100, 's100')").unwrap();
    s.commit().unwrap();
}

/// Verifies: [X3.135-1986 §7.3 GR3b i, §8.9 GR1]; [DESIGN] RULES.md D38:
/// a statement refused by the memory budget (SQLCODE -918) is undone and
/// the transaction goes on: its commit, reading its changes back for its
/// check without a page more of the budget, keeps the rows before it, and
/// their index entries. Regression test (R-TEST-7): the commit's check was
/// refused for want of memory, and its rollback damaged the table.
#[test]
fn a_transaction_past_the_budget_commits_what_it_did() {
    let db = Database::new();
    let (s, k) = past_the_budget(&db);
    s.commit().unwrap();
    assert_eq!(db.damaged_tables().unwrap(), vec![]);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T"), Ok(vec![k.to_string()]));
    let last = k - 1;
    assert_eq!(rows(&s, &format!("SELECT K FROM T WHERE S = 's{last}'")), Ok(vec![last.to_string()]));
}
