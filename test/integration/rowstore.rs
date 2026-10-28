//! The row-store layout (RULES.md D31): tables as B-link trees of records
//! keyed by row id. Statements give the same results on it as on the
//! memory engine; long values go out of line, by a threshold set per table,
//! and their pages are freed with their rows; rows too wide are refused;
//! row ids increase, are never given again, and go on past a table opened
//! again; and threads write a table at once.

use std::sync::Arc;
use std::thread;

use turborel::catalog::{ColumnDesc, QualifiedName, TableDesc};
use turborel::db::Database;
use turborel::disk::buffer::{BufferPool, Clock, FileId};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::OsFileSystem;
use turborel::disk::pager::Pager;
use turborel::engine::rows::{ROW_STORE, RowStore, RowTable, TableFiles, TableOptions};
use turborel::error::Code;
use turborel::log::MemoryLog;
use turborel::params::Params;
use turborel::storage::{Row, StorageEngine, TableStorage};
use turborel::txn::NO_TRANSACTION;
use turborel::value::{DataType, Value};

use crate::common::{Scratch, crc32c, exec, number, rows};

/// Where a row store keeps its tables: `pool`'s pager file `file`, older
/// versions in memory.
fn files(pool: &Arc<BufferPool>, file: FileId) -> TableFiles {
    TableFiles::new(Arc::clone(pool), file, Arc::new(MemoryLog::new()))
}

/// A pool of `frames` frames over a new pager file of 4 KiB pages.
fn pool(scratch: &Scratch, frames: usize) -> (Arc<BufferPool>, FileId, Arc<Pager>) {
    let hash = crc32c();
    let pager = Arc::new(Pager::create(&OsFileSystem, &scratch.path("rows"), hash, Pipeline::none(), 4096).unwrap());
    let pool = Arc::new(BufferPool::new(frames, Box::new(Clock::new(frames))).unwrap());
    let file = pool.add_pager(Arc::clone(&pager));
    (pool, file, pager)
}

/// A database whose tables are in the row store, if `store` is given.
fn database(store: Option<RowStore>) -> Database {
    let db = Database::new();
    if let Some(store) = store {
        db.register_engine(Box::new(store)).unwrap();
        db.use_engine(ROW_STORE).unwrap();
    }
    db
}

/// Runs `script` and returns what each statement gave: the rows of a query
/// (starting with SELECT), else the outcome, or the SQLCODE of a failure.
fn transcript(db: &Database, script: &[&str]) -> Vec<String> {
    let s = db.session("A");
    let mut p = Params::new().declare("KOUT", DataType::Exact { precision: 9, scale: 0 }, Value::Null);
    script
        .iter()
        .map(|sql| match *sql {
            "COMMIT" => format!("{:?}", s.commit().map_err(|e| e.code)),
            "ROLLBACK" => format!("{:?}", s.rollback().map_err(|e| e.code)),
            _ if sql.starts_with("SELECT") => format!("{sql}: {:?}", rows(&s, sql)),
            _ if sql.starts_with("DECLARE") => format!("{:?}", s.declare_cursor(sql).map_err(|e| e.code)),
            _ if sql.starts_with("FETCH") => format!("{sql}: {:?} {:?}", s.execute(sql, &mut p).map_err(|e| e.code), p.value("KOUT")),
            _ => format!("{sql}: {:?}", s.execute(sql, &mut p).map_err(|e| e.code)),
        })
        .collect()
}

/// Verifies: [DESIGN] RULES.md D31, row store: a workload of definitions,
/// inserts (long values among them), searched and positioned updates and
/// deletes, a UNIQUE violation undone, a rollback, an index, joins,
/// grouping and ordering gives exactly the results it gives on the memory
/// engine.
#[test]
fn statements_give_the_memory_engines_results() {
    let long = |c: char| c.to_string().repeat(2500);
    let (a, b, c) = (long('a'), long('b'), long('c'));
    let script = [
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, S CHAR(3000), N DECIMAL(9,2)) CREATE TABLE U (K INTEGER, V CHAR(4))",
        &format!("INSERT INTO T VALUES (1, '{a}', 1.5)"),
        "INSERT INTO T VALUES (2, 'two', NULL)",
        &format!("INSERT INTO T VALUES (3, '{b}', -7)"),
        "INSERT INTO T VALUES (1, 'dup', 0)",
        "INSERT INTO U VALUES (1, 'u1')",
        "INSERT INTO U VALUES (3, 'u3')",
        "INSERT INTO U VALUES (3, 'u33')",
        "COMMIT",
        "SELECT K, S, N FROM T ORDER BY K",
        &format!("UPDATE T SET S = '{c}' WHERE K = 2"),
        "UPDATE T SET S = 'short' WHERE K = 1",
        "SELECT K, S FROM T WHERE K < 3 ORDER BY K DESC",
        "DELETE FROM T WHERE K = 3",
        "ROLLBACK",
        "SELECT COUNT(*), SUM(N) FROM T",
        "SELECT T.K, U.V FROM T, U WHERE T.K = U.K ORDER BY U.V",
        "SELECT K, COUNT(*) FROM U GROUP BY K",
        "DECLARE C CURSOR FOR SELECT K FROM T WHERE K > 1",
        "OPEN C",
        "FETCH C INTO KOUT",
        "UPDATE T SET N = N + 100 WHERE CURRENT OF C",
        "FETCH C INTO KOUT",
        "DELETE FROM T WHERE CURRENT OF C",
        "FETCH C INTO KOUT",
        "CLOSE C",
        "SELECT K, N FROM T",
        "INSERT INTO T VALUES (4, 'four', 4)",
        "UPDATE T SET K = K + 10",
        "SELECT K FROM T",
        "COMMIT",
    ];
    let scratch = Scratch::new("rowstore-same");
    let (pool, file, _) = pool(&scratch, 64);
    let on_rows = {
        let db = database(Some(RowStore::new(files(&pool, file), TableOptions::default())));
        db.schema(script[0]).unwrap();
        transcript(&db, &script[1..])
    };
    let expected = {
        let db = database(None);
        db.schema(script[0]).unwrap();
        transcript(&db, &script[1..])
    };
    let failed: Vec<&String> = expected.iter().filter(|r| r.contains(": Err(")).collect();
    assert_eq!(failed.len(), 1, "only the duplicate key fails: {failed:#?}");
    for (got, want) in on_rows.iter().zip(&expected) {
        assert_eq!(got, want);
    }
    assert_eq!(on_rows.len(), expected.len());
}

fn chars(s: &str, length: u32) -> Value {
    Value::Char(format!("{s:<width$}", width = usize::try_from(length).unwrap()))
}

/// A table of an integer and a CHAR(`length`), in `store`.
fn table(store: &RowStore, name: &str, length: u32) -> Arc<dyn TableStorage> {
    let columns = vec![
        ColumnDesc { name: "K".into(), data_type: DataType::Exact { precision: 9, scale: 0 }, not_null: true, default: None, domain: None },
        ColumnDesc { name: "S".into(), data_type: DataType::Character { length }, not_null: false, default: None, domain: None },
    ];
    let desc = TableDesc {
        name: QualifiedName { auth: "A".into(), ident: name.into() },
        columns,
        constraints: vec![],
        engine: ROW_STORE.into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    store.create_table(&desc).unwrap().into()
}

fn shown(row: &Row) -> Vec<String> {
    row.iter().map(ToString::to_string).collect()
}

/// Verifies: [DESIGN] RULES.md D31, values out of line: a value over the
/// threshold takes pages of its own, read back whole; one at the threshold
/// stays in its row; replacing a long value with a short one, or deleting
/// its row, frees its pages, so the file stops growing; a scan of the other
/// columns does not read them. A threshold set for one table applies to it
/// alone.
#[test]
fn long_values_go_out_of_line_and_are_freed() {
    let scratch = Scratch::new("rowstore-long");
    let (pool, file, pager) = pool(&scratch, 64);
    let store = RowStore::new(files(&pool, file), TableOptions::default());
    let t = table(&store, "T", 30_000);
    // A long content's overflow pages are allocated as it is written back.
    let free = || {
        pool.flush().unwrap();
        pager.page_count() - pager.free_pages()
    };
    let before = free();
    let long = "é".repeat(20_000);
    let id = t.insert(NO_TRANSACTION, vec![number(1), chars(&long, 30_000)]).unwrap();
    assert!(free() > before + 8, "40,000 bytes take pages of their own");
    assert_eq!(shown(&t.get(NO_TRANSACTION, id).unwrap().unwrap()), shown(&vec![number(1), chars(&long, 30_000)]));
    let misses = pool.counts().misses;
    let keys: Vec<Row> = Arc::clone(&t).scan(NO_TRANSACTION, &[0]).map(|r| r.unwrap().1).collect();
    assert_eq!((keys.len(), pool.counts().misses), (1, misses), "a scan of K reads no value page");
    let with_long = free();
    t.update(NO_TRANSACTION, id, vec![number(1), chars("short", 30_000)]).unwrap();
    assert!(free() < with_long - 8, "the long value's pages are freed");
    for _ in 0..20 {
        t.update(NO_TRANSACTION, id, vec![number(1), chars(&long, 30_000)]).unwrap();
    }
    t.delete(NO_TRANSACTION, id).unwrap();
    assert!(free() <= with_long, "updates and the delete freed what they replaced: {} pages", free());

    store.configure(
        QualifiedName { auth: "A".into(), ident: "SMALL".into() },
        TableOptions { out_of_line: Some(4), ..TableOptions::default() },
    );
    let small = table(&store, "SMALL", 10);
    let other = table(&store, "OTHER", 10);
    let used = free();
    other.insert(NO_TRANSACTION, vec![number(1), chars("0123456789", 10)]).unwrap();
    assert_eq!(free(), used, "ten bytes stay in line by default");
    let id = small.insert(NO_TRANSACTION, vec![number(1), chars("0123456789", 10)]).unwrap();
    assert_eq!(free(), used + 1, "past four bytes, out of line in this table");
    assert_eq!(shown(&small.get(NO_TRANSACTION, id).unwrap().unwrap()), ["1", "0123456789"]);
    small.insert(NO_TRANSACTION, vec![number(2), chars("0123", 10)]).unwrap();
    assert_eq!(free(), used + 1, "four bytes stay");
}

/// Verifies: [DESIGN] RULES.md D31, row store: a row too wide for a leaf
/// even with its long values out of line (here 200 numbers, 1,600 bytes,
/// in 4 KiB pages) is refused with SQLCODE -900, and the statement changes
/// nothing; a row of many long values fits, all of them out of line.
#[test]
fn rows_too_wide_are_refused() {
    let scratch = Scratch::new("rowstore-wide");
    let (pool, file, _) = pool(&scratch, 64);
    let db = database(Some(RowStore::new(files(&pool, file), TableOptions::default())));
    let numbers: Vec<String> = (0..200).map(|i| format!("N{i} DOUBLE PRECISION")).collect();
    let strings: Vec<String> = (0..20).map(|i| format!("S{i} CHAR(1000)")).collect();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE W ({}) CREATE TABLE L ({})", numbers.join(", "), strings.join(", ")))
        .unwrap();
    let s = db.session("A");
    let values = vec!["1"; 200].join(", ");
    assert_eq!(exec(&s, &format!("INSERT INTO W VALUES ({values})")), Err(Code::Unsupported));
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM W"), Ok(vec!["0".into()]));
    let long: Vec<String> = (0..20).map(|i: usize| format!("'{}'", i.to_string().repeat(if i < 10 { 900 } else { 450 }))).collect();
    s.run(&format!("INSERT INTO L VALUES ({})", long.join(", "))).unwrap();
    let back = rows(&s, "SELECT S0, S19 FROM L").unwrap();
    assert_eq!(back.len(), 1);
    assert!(back[0].starts_with(&format!("{} ", "0".repeat(900))) && back[0].contains(&"19".repeat(450)));
}

/// Verifies: [DESIGN] RULES.md D31, row ids: ids increase and are never
/// given again, not after a delete, nor after the table is opened again,
/// which counts its rows; a row restored under an id past those given
/// moves the next id past it, and one restored over a live row replaces
/// it. A page that is not a table's meta page is
/// refused with SQLCODE -916.
#[test]
fn row_ids_increase_and_are_never_given_again() {
    let scratch = Scratch::new("rowstore-ids");
    let (pool, file, _) = pool(&scratch, 64);
    let types = [DataType::Exact { precision: 9, scale: 0 }];
    let files = files(&pool, file);
    let t = RowTable::create(&files, &types, TableOptions::default()).unwrap();
    let ids: Vec<u64> = (0..2000).map(|n| t.insert(NO_TRANSACTION, vec![number(n)]).unwrap()).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]));
    t.delete(NO_TRANSACTION, ids[1999]).unwrap();
    let after = t.insert(NO_TRANSACTION, vec![number(0)]).unwrap();
    assert!(after > ids[1999], "a deleted row's id is not given again");
    t.restore(NO_TRANSACTION, after + 50, vec![number(9)]).unwrap();
    let count = t.row_count();
    t.restore(NO_TRANSACTION, after + 50, vec![number(10)]).unwrap();
    assert_eq!(t.row_count(), count, "restoring a live row replaces it");
    assert_eq!(shown(&t.get(NO_TRANSACTION, after + 50).unwrap().unwrap()), ["10"]);
    assert!(t.insert(NO_TRANSACTION, vec![number(0)]).unwrap() > after + 50);
    let meta = t.meta();
    let count = t.row_count();
    drop(t);
    let again = RowTable::open(&files, meta, &types, TableOptions::default()).unwrap();
    assert_eq!(again.row_count(), count);
    assert!(again.insert(NO_TRANSACTION, vec![number(0)]).unwrap() > after + 51, "past every id given before");
    let (other, mut page) = pool.new_page(file).unwrap();
    page.extend_from_slice(b"not a table");
    drop(page);
    assert_eq!(RowTable::open(&files, other, &types, TableOptions::default()).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Verifies: [DESIGN] RULES.md D31, row store: threads inserting into one
/// table at once each get ids of their own, and every row is found; each
/// thread's ids increase, one after another within its block of 4,096, so
/// threads fill different leaves (`rows/ids.rs`).
#[test]
fn threads_insert_into_one_table_at_once() {
    let scratch = Scratch::new("rowstore-threads");
    let (pool, file, _) = pool(&scratch, 256);
    let store = RowStore::new(files(&pool, file), TableOptions::default());
    let t = table(&store, "T", 20);
    let handles: Vec<_> = (0..4)
        .map(|w| {
            let t = Arc::clone(&t);
            thread::spawn(move || {
                (0..2000)
                    .map(|n| (t.insert(NO_TRANSACTION, vec![number(w * 10_000 + n), chars("x", 20)]).unwrap(), w * 10_000 + n))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let per_thread: Vec<Vec<(u64, i128)>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for ids in &per_thread {
        assert!(ids.windows(2).all(|w| w[0].0 < w[1].0), "a thread's ids increase");
        let jumps = ids.windows(2).filter(|w| w[1].0 != w[0].0 + 1).count();
        assert!(jumps <= 1, "one block of 4,096 holds 2,000 ids, with at most one more taken: {jumps} jumps");
    }
    let mut given: Vec<(u64, i128)> = per_thread.into_iter().flatten().collect();
    given.sort_unstable();
    assert!(given.windows(2).all(|w| w[0].0 < w[1].0), "no id given twice");
    assert_eq!(t.row_count(), 8000);
    for (id, n) in given {
        assert_eq!(t.get(NO_TRANSACTION, id).unwrap().unwrap()[0].to_string(), n.to_string());
    }
}
