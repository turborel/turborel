//! Multi-version concurrency control in the row store (RULES.md D29, D31):
//! readers see the snapshot they began with and never wait; the second
//! writer of a row is refused; transactions that are not serializable are
//! refused at commit, and those that read only never are; UNIQUE sees
//! rows a transaction cannot; indexes hold every version's entry; old
//! versions are collected once no transaction can read them; and threads
//! moving money between accounts keep the total.
//! [X3.135-1986 §4.16]: concurrent execution is serializable.

use std::sync::Arc;
use std::thread;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::buffer::{BufferPool, Clock};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::{Durability, OsFileSystem};
use turborel::disk::pager::Pager;
use turborel::engine::rows::{ROW_STORE, RowStore, TableFiles, TableOptions};
use turborel::error::Code;
use turborel::log::{FileLogOptions, MemoryLog};

use crate::common::{Scratch, crc32c, os, rows};

/// A database whose tables are in a row store, with `schema`; and where
/// the row store keeps them, to look at its undo store.
fn database(scratch: &Scratch, schema: &str) -> (Database, TableFiles) {
    let hash = crc32c();
    let pager = Pager::create(&OsFileSystem, &scratch.path("rows"), hash, Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(256, Box::new(Clock::new(256))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let db = Database::new();
    db.register_engine(Box::new(RowStore::new(files.clone(), TableOptions::default()))).unwrap();
    db.use_engine(ROW_STORE).unwrap();
    db.schema(schema).unwrap();
    (db, files)
}

fn run(s: &Session, sql: &str) -> Result<(), Code> {
    s.run(sql).map(drop).map_err(|e| e.code)
}

fn values(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap()
}

const ACCOUNTS: &str = "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER)";

/// Two sessions on a table of rows (1, 10) and (2, 20), committed.
fn accounts(scratch: &Scratch) -> (Database, TableFiles, Session, Session) {
    let (db, files) = database(scratch, ACCOUNTS);
    let s = db.session("A");
    run(&s, "INSERT INTO T VALUES (1, 10)").unwrap();
    run(&s, "INSERT INTO T VALUES (2, 20)").unwrap();
    s.commit().unwrap();
    let other = db.session("A");
    (db, files, s, other)
}

/// Verifies: [X3.135-1986 §4.16] a transaction's changes are invisible to
/// others until it commits; [DESIGN] RULES.md D29, snapshots: a reader
/// keeps seeing what was committed when it began, through the other's
/// commit, and never waits for a writer, nor a writer for it; once it ends,
/// its next transaction sees the commit.
#[test]
fn readers_see_their_snapshot_and_never_wait() {
    let scratch = Scratch::new("mvcc-snapshot");
    let (_db, _files, writer, reader) = accounts(&scratch);
    assert_eq!(values(&reader, "SELECT V FROM T WHERE K = 1"), ["10"], "the reader's snapshot begins");
    run(&writer, "UPDATE T SET V = 11 WHERE K = 1").unwrap();
    run(&writer, "INSERT INTO T VALUES (3, 30)").unwrap();
    assert_eq!(values(&reader, "SELECT K, V FROM T"), ["1,10", "2,20"], "no dirty read, no wait");
    writer.commit().unwrap();
    assert_eq!(values(&reader, "SELECT K, V FROM T"), ["1,10", "2,20"], "the same snapshot after the commit");
    reader.commit().unwrap();
    assert_eq!(values(&reader, "SELECT K, V FROM T"), ["1,11", "2,20", "3,30"], "a new transaction sees it");
}

/// Verifies: [DESIGN] RULES.md D29, write conflicts: writers of different
/// rows, found by key, never conflict, and both commit; a row another open
/// transaction has changed is refused to a second writer (SQLCODE -911),
/// the statement undone and the transaction going on, though it read the
/// row, so its commit is refused once the first commits; a transaction
/// whose snapshot is older than the first's commit is refused the row too
/// (no lost update). A rolled-back change frees the row at once.
#[test]
fn the_second_writer_of_a_row_is_refused() {
    let scratch = Scratch::new("mvcc-writers");
    let (_db, _files, first, second) = accounts(&scratch);
    run(&first, "UPDATE T SET V = V + 1 WHERE K = 1").unwrap();
    run(&second, "UPDATE T SET V = V + 100 WHERE K = 2").unwrap();
    first.commit().unwrap();
    second.commit().unwrap();
    assert_eq!(values(&first, "SELECT K, V FROM T ORDER BY K"), ["1,11", "2,120"], "both committed");
    first.commit().unwrap();

    run(&first, "UPDATE T SET V = V + 1 WHERE K = 1").unwrap();
    assert_eq!(run(&second, "UPDATE T SET V = V + 100 WHERE K = 1"), Err(Code::Conflict));
    first.commit().unwrap();
    assert_eq!(second.commit().map_err(|e| e.code), Err(Code::Conflict), "it read the row the first changed");

    assert_eq!(values(&second, "SELECT V FROM T WHERE K = 1"), ["12"], "a snapshot before the next change");
    run(&first, "UPDATE T SET V = 0 WHERE K = 1").unwrap();
    first.commit().unwrap();
    assert_eq!(run(&second, "UPDATE T SET V = V + 100 WHERE K = 1"), Err(Code::Conflict), "changed after its snapshot");
    second.rollback().unwrap();

    run(&first, "DELETE FROM T WHERE K = 2").unwrap();
    first.rollback().unwrap();
    run(&second, "UPDATE T SET V = 5 WHERE K = 2").unwrap();
    second.commit().unwrap();
    assert_eq!(values(&first, "SELECT K, V FROM T ORDER BY K"), ["1,0", "2,5"]);
}

/// Verifies: [X3.135-1986 §4.16] serializable execution; [DESIGN] RULES.md
/// D29, validation: two transactions that each read both rows and update a
/// different one (write skew) cannot both commit: the second is refused
/// (SQLCODE -911) and rolled back. A transaction that read a row a
/// committed one changed, then wrote elsewhere, is refused; one that only
/// read never is.
#[test]
fn write_skew_is_refused_at_commit() {
    let scratch = Scratch::new("mvcc-skew");
    let (db, _files, a, b) = accounts(&scratch);
    assert_eq!(values(&a, "SELECT SUM(V) FROM T"), ["30"]);
    assert_eq!(values(&b, "SELECT SUM(V) FROM T"), ["30"]);
    run(&a, "UPDATE T SET V = V - 30 WHERE K = 1").unwrap();
    run(&b, "UPDATE T SET V = V - 30 WHERE K = 2").unwrap();
    a.commit().unwrap();
    assert_eq!(b.commit().map_err(|e| e.code), Err(Code::Conflict));
    assert_eq!(values(&b, "SELECT SUM(V) FROM T"), ["0"], "only the first took effect");
    b.commit().unwrap();

    let reader = db.session("A");
    assert_eq!(values(&reader, "SELECT V FROM T WHERE K = 1"), ["-20"]);
    run(&a, "UPDATE T SET V = 5 WHERE K = 1").unwrap();
    a.commit().unwrap();
    reader.commit().unwrap();
    assert_eq!(values(&reader, "SELECT V FROM T WHERE K = 1"), ["5"]);
    run(&a, "UPDATE T SET V = 6 WHERE K = 1").unwrap();
    a.commit().unwrap();
    run(&reader, "UPDATE T SET V = 7 WHERE K = 2").unwrap();
    assert_eq!(reader.commit().map_err(|e| e.code), Err(Code::Conflict), "it read K = 1 before the change");
}

/// Verifies: [X3.135-1986 §4.16] serializable execution; [DESIGN] RULES.md
/// D29, phantoms: a transaction that counted rows by a condition, through
/// a scan or through an index, and wrote, is refused at commit if another
/// committed meanwhile a row the condition matches.
#[test]
fn phantoms_are_refused_at_commit() {
    let scratch = Scratch::new("mvcc-phantom");
    let (db, _files, a, b) = accounts(&scratch);
    db.create_index("T_V", "A.T", &["V"], "btree").unwrap();
    for (counting, sql) in [("a scan", "SELECT COUNT(*) FROM T WHERE K > 0"), ("an index", "SELECT COUNT(*) FROM T WHERE V = 99")] {
        let _ = values(&a, sql);
        run(&b, "INSERT INTO T (K, V) SELECT MAX(K) + 1, 99 FROM T").unwrap_or_else(|_| run(&b, "INSERT INTO T VALUES (9, 99)").unwrap());
        b.commit().unwrap();
        run(&a, "INSERT INTO T VALUES (100, 1)").unwrap();
        assert_eq!(a.commit().map_err(|e| e.code), Err(Code::Conflict), "{counting}");
        run(&b, "DELETE FROM T WHERE V = 99").unwrap();
        b.commit().unwrap();
    }
}

/// Verifies: [X3.135-1986 §4.16] serializable execution; [DESIGN] RULES.md
/// D29, phantoms of a scan by several conditions: the scan reads only the
/// rows each of its filter's conditions matches, so a row committed
/// meanwhile that one of them does not match refuses no commit, and one
/// that each matches does.
#[test]
fn a_scan_reads_the_rows_all_its_conditions_match() {
    let scratch = Scratch::new("mvcc-conjunction");
    let (_db, _files, a, b) = accounts(&scratch);
    let _ = values(&a, "SELECT COUNT(*) FROM T WHERE K > 0 AND V < 50");
    run(&b, "INSERT INTO T VALUES (50, 99)").unwrap();
    b.commit().unwrap();
    run(&a, "INSERT INTO T VALUES (100, 1)").unwrap();
    a.commit().unwrap();
    let _ = values(&a, "SELECT COUNT(*) FROM T WHERE K > 0 AND V < 50");
    run(&b, "INSERT INTO T VALUES (60, 5)").unwrap();
    b.commit().unwrap();
    run(&a, "INSERT INTO T VALUES (101, 1)").unwrap();
    assert_eq!(a.commit().map_err(|e| e.code), Err(Code::Conflict), "a row both match");
}

/// Verifies: [X3.135-1986 §6.4 GR2] UNIQUE; [DESIGN] RULES.md D29: a key
/// another open transaction has inserted is in doubt, so a second insert of
/// it is refused with SQLCODE -911; once that transaction commits, the
/// second insert violates the constraint (SQLCODE -201), though its
/// snapshot does not see the row; once it rolls back instead, the second
/// insert succeeds. A key freed by an uncommitted delete is in doubt too.
#[test]
fn unique_sees_rows_a_transaction_cannot() {
    let scratch = Scratch::new("mvcc-unique");
    let (_db, _files, a, b) = accounts(&scratch);
    let _ = values(&b, "SELECT K FROM T");
    run(&a, "INSERT INTO T VALUES (5, 0)").unwrap();
    assert_eq!(run(&b, "INSERT INTO T VALUES (5, 1)"), Err(Code::Conflict));
    a.commit().unwrap();
    assert_eq!(values(&b, "SELECT COUNT(*) FROM T WHERE K = 5"), ["0"], "b's snapshot does not see it");
    assert_eq!(run(&b, "INSERT INTO T VALUES (5, 1)"), Err(Code::Constraint));
    // It scanned the table, to which a row was added meanwhile: its commit
    // would be refused.
    b.rollback().unwrap();
    run(&a, "INSERT INTO T VALUES (6, 0)").unwrap();
    a.rollback().unwrap();
    run(&b, "INSERT INTO T VALUES (6, 1)").unwrap();
    b.commit().unwrap();
    run(&a, "DELETE FROM T WHERE K = 6").unwrap();
    assert_eq!(run(&b, "INSERT INTO T VALUES (6, 2)"), Err(Code::Conflict), "the delete may roll back");
}

/// Verifies: [DESIGN] RULES.md D29, indexes: a reader using an index finds
/// rows by the values its snapshot sees, though another transaction has
/// since changed them and committed: by the old value, not the new; a new
/// transaction by the new value, not the old. An index created while old
/// versions are still read holds their entries too.
#[test]
fn indexes_find_rows_by_the_version_a_reader_sees() {
    let scratch = Scratch::new("mvcc-index");
    let (db, _files, writer, reader) = accounts(&scratch);
    db.create_index("T_V", "A.T", &["V"], "hash").unwrap();
    let by_value = |s: &Session, v: i32| values(s, &format!("SELECT K FROM T WHERE V = {v}"));
    assert_eq!(by_value(&reader, 10), ["1"]);
    run(&writer, "UPDATE T SET V = 15 WHERE K = 1").unwrap();
    writer.commit().unwrap();
    assert_eq!((by_value(&reader, 10), by_value(&reader, 15)), (vec!["1".to_owned()], vec![]), "the old value");
    db.create_index("T_V2", "A.T", &["V"], "btree").unwrap();
    assert_eq!(values(&reader, "SELECT K FROM T WHERE V < 12"), ["1"], "the new index has the old version's entry");
    reader.commit().unwrap();
    assert_eq!((by_value(&reader, 10), by_value(&reader, 15)), (vec![], vec!["1".to_owned()]), "the new value");
}

/// Verifies: [X3.135-1986 §7.3 GR3b i] a failed statement's changes are
/// cancelled and the transaction goes on, in an engine undoing its own
/// changes: an UPDATE of every row that ends with a duplicate key leaves
/// every row as it was.
#[test]
fn a_failed_statement_is_undone_by_the_engine() {
    let scratch = Scratch::new("mvcc-statement");
    let (_db, _files, s, _) = accounts(&scratch);
    run(&s, "UPDATE T SET V = 0 WHERE K = 2").unwrap();
    assert_eq!(run(&s, "UPDATE T SET K = 1, V = 99"), Err(Code::Constraint));
    assert_eq!(values(&s, "SELECT K, V FROM T"), ["1,10", "2,0"], "the failed statement undone, the one before kept");
    s.commit().unwrap();
}

/// Verifies: [DESIGN] RULES.md D29, D31, garbage collection: older versions
/// stay in the undo store while a transaction may read them, here a
/// reader that began before many updates, and are released when it ends;
/// deleted rows leave the table then. With no transaction open, nothing is
/// pinned.
#[test]
fn old_versions_are_collected_once_unread() {
    let scratch = Scratch::new("mvcc-gc");
    let (_db, files, writer, reader) = accounts(&scratch);
    assert_eq!(values(&reader, "SELECT SUM(V) FROM T"), ["30"]);
    for v in 0..50 {
        run(&writer, &format!("UPDATE T SET V = {v} WHERE K = 1")).unwrap();
        writer.commit().unwrap();
    }
    run(&writer, "DELETE FROM T WHERE K = 2").unwrap();
    writer.commit().unwrap();
    assert!(files.undo().pinned() >= 50, "the reader may still read them: {}", files.undo().pinned());
    assert_eq!(values(&reader, "SELECT SUM(V) FROM T"), ["30"], "and does");
    reader.commit().unwrap();
    run(&writer, "UPDATE T SET V = 100 WHERE K = 1").unwrap();
    writer.commit().unwrap();
    assert_eq!(files.undo().pinned(), 0, "nothing pinned once no transaction can read it");
    assert_eq!(values(&reader, "SELECT K, V FROM T"), ["1,100"]);
}

/// Verifies: [X3.135-1986 §4.16] serializable execution under load; [DESIGN]
/// RULES.md D29: threads moving money between accounts, each transfer a
/// transaction run again when refused (SQLCODE -911), keep the total, and
/// every transfer is applied once; readers summing meanwhile always see
/// the total.
#[test]
fn transfers_on_threads_keep_the_total() {
    let scratch = Scratch::new("mvcc-transfers");
    let (db, _files) = database(&scratch, ACCOUNTS);
    let accounts = 8;
    let s = db.session("A");
    for k in 0..accounts {
        run(&s, &format!("INSERT INTO T VALUES ({k}, 100)")).unwrap();
    }
    s.commit().unwrap();
    let db = Arc::new(db);
    let handles: Vec<_> = (0..4)
        .map(|t| {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                let s = db.session("A");
                let mut done = 0;
                let mut n = t;
                while done < 50 {
                    n += 1;
                    let (from, to) = (n % accounts, (n * 3 + 1) % accounts);
                    let moved = run(&s, &format!("UPDATE T SET V = V - 1 WHERE K = {from}"))
                        .and_then(|()| run(&s, &format!("UPDATE T SET V = V + 1 WHERE K = {to}")))
                        .and_then(|()| s.commit().map_err(|e| e.code));
                    match moved {
                        Ok(()) => done += 1,
                        Err(Code::Conflict) => s.rollback().unwrap(),
                        Err(other) => panic!("transfer failed with {other:?}"),
                    }
                }
            })
        })
        .collect();
    let reader = db.session("A");
    for _ in 0..50 {
        assert_eq!(values(&reader, "SELECT SUM(V) FROM T"), ["800"]);
        reader.commit().unwrap();
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(values(&reader, "SELECT SUM(V), COUNT(*) FROM T"), ["800,8"]);
}

/// The segments of the log of older row versions in `dir`.
fn undo_segments(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir.join("undo")).map_or(0, Iterator::count)
}

/// Verifies: [DESIGN] RULES.md D29, D31: a database kept in a directory
/// keeps its older row versions on disk, in a log of their own (`undo/`):
/// a reader that began before many updates reads the rows as they were,
/// from there; once it ends, the space is taken back as transactions end;
/// opened again, the database begins the log anew, removing what a run
/// before left.
#[test]
fn older_versions_of_a_kept_database_are_on_disk() {
    let scratch = Scratch::new("undo-on-disk");
    let dir = scratch.path("db");
    let undo = FileLogOptions { segment_size: 64 << 10, durability: Durability::None, ..FileLogOptions::default() };
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions { undo: undo.clone(), ..OpenOptions::default() }).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(100))").unwrap();
    let writer = db.session("S");
    for k in 0..100 {
        writer.run(&format!("INSERT INTO T VALUES ({k}, 'v0')")).unwrap();
    }
    writer.commit().unwrap();
    let reader = db.session("S");
    let mut cursor = reader.open_cursor("SELECT V FROM T").unwrap();
    assert_eq!(cursor.fetch().unwrap().map(|r| r[0].to_string().trim_end().to_owned()), Some("v0".to_owned()));
    for round in 1..=20 {
        writer.run(&format!("UPDATE T SET V = '{round:x>90}'")).unwrap();
        writer.commit().unwrap();
    }
    let held = undo_segments(&dir);
    assert!(held > 3, "2,000 older versions of 90 bytes, in segments of 64 KiB: {held}");
    let mut seen = 1;
    while let Some(row) = cursor.fetch().unwrap() {
        assert_eq!(row[0].to_string().trim_end(), "v0", "as when the reader began");
        seen += 1;
    }
    assert_eq!(seen, 100);
    drop(cursor);
    reader.commit().unwrap();
    for round in 21..=22 {
        writer.run(&format!("UPDATE T SET V = '{round:x>90}'")).unwrap();
        writer.commit().unwrap();
    }
    let kept = undo_segments(&dir);
    assert!(kept < held, "taken back once no reader needs them: {kept} of {held}");
    drop((writer, reader));
    db.close().unwrap();
    drop(db);
    std::fs::write(dir.join("undo").join("segment-0000000099.log"), b"left").unwrap();
    let db = open();
    assert_eq!(undo_segments(&dir), 1, "begun anew");
    let after = rows(&db.session("S"), "SELECT DISTINCT V FROM T").unwrap();
    assert_eq!(after.iter().map(|v| v.trim_end().to_owned()).collect::<Vec<_>>(), [format!("{:x>90}", 22)]);
}

/// Verifies: [DESIGN] RULES.md D29: versions a reader still held when
/// their table was last collected are collected once it ends, though no
/// transaction touches their table again: the horizon moves only as a
/// transaction ends, and each that ends collects every table with versions
/// awaiting collection, their index entries going with them.
#[test]
fn an_idle_tables_garbage_is_collected_once_readers_end() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER) CREATE TABLE U (K INTEGER NOT NULL, V INTEGER)").unwrap();
    db.create_index("U_V", "A.U", &["V"], "btree").unwrap();
    let s = db.session("A");
    run(&s, "INSERT INTO T VALUES (1)").unwrap();
    run(&s, "INSERT INTO U VALUES (1, 10), (2, 20), (3, 30)").unwrap();
    s.commit().unwrap();
    let reader = db.session("A");
    assert_eq!(values(&reader, "SELECT K FROM T"), ["1"], "a snapshot, held open, of T alone");
    run(&s, "UPDATE U SET V = V + 1").unwrap();
    s.commit().unwrap();
    assert_eq!(db.index_entries("U_V").unwrap(), 6, "the reader may read the older versions");
    reader.commit().unwrap();
    assert_eq!(db.index_entries("U_V").unwrap(), 3, "collected as the reader ended");
}
