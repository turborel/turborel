//! \[EXTENSION\] The column store (RULES.md D31): a
//! table's rows frozen into groups of their columns' values once committed
//! before every snapshot, read, changed, indexed, kept unique and
//! recovered as a table of the row store's are.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;

use crate::common::{Scramble, Scratch, after_crash, exec, os, rows, sorted};

/// The rows of `ROWS` written: K, a key; G, one of 7 groups; S, a text,
/// null for every 11th; N, a number in the order the rows were written.
const ROWS: i64 = 5000;

/// The tables R, of the row store, and C, of the column store in
/// groups of 2048 rows, of schema A, each with `ROWS` rows, C's frozen.
fn both(db: &Database) -> Session {
    let a = written(db);
    db.maintain().unwrap();
    a
}

/// As `both`, C's rows left for the maintenance thread to freeze.
fn written(db: &Database) -> Session {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, S VARCHAR(20), N INTEGER)
           CREATE TABLE C (K INTEGER PRIMARY KEY, G INTEGER, S VARCHAR(20), N INTEGER)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..ROWS).step_by(500) {
            let values: Vec<String> = (first..first + 500)
                .map(|k| {
                    let s = if k % 11 == 0 { "NULL".to_owned() } else { format!("'s{}'", k % 97) };
                    format!("({k}, {}, {s}, {})", k % 7, k.checked_div(10).unwrap())
                })
                .collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    a.commit().unwrap();
    a
}

/// The queries each table answers alike, `{t}` its name.
const QUERIES: [&str; 10] = [
    "SELECT COUNT(*), SUM(N), MIN(K), MAX(K) FROM {t}",
    "SELECT K, S FROM {t} WHERE K BETWEEN 2040 AND 2060",
    "SELECT K FROM {t} WHERE N >= 480 AND N < 482",
    "SELECT G, COUNT(*), SUM(N), COUNT(S) FROM {t} GROUP BY G",
    "SELECT K FROM {t} WHERE S IS NULL AND K < 100",
    "SELECT K, N FROM {t} WHERE S = 's5'",
    "SELECT COUNT(*) FROM {t} WHERE G = 3 AND N > 250",
    "SELECT K FROM {t} WHERE K = 4999",
    "SELECT DISTINCT G FROM {t} WHERE K > 4990",
    "SELECT T.K FROM {t} T, R WHERE T.K = R.K AND R.N = 7",
];

/// What `s` reads of table `table` by each query, sorted.
fn answers(s: &Session, table: &str) -> Vec<Vec<String>> {
    QUERIES.iter().map(|q| sorted(s, &q.replace("{t}", table))).collect()
}

/// Verifies: [EXTENSION] RULES.md D31, the column store: a table of it
/// gives every query the rows a table of the row store gives, its rows
/// frozen into groups: counts and sums, ranges of its key and of another
/// column, groups, nulls, equalities, a join.
#[test]
fn a_column_table_answers_as_a_row_table() {
    let db = Database::new();
    let a = both(&db);
    assert_eq!(answers(&a, "C"), answers(&a, "R"));
}

/// Verifies: [EXTENSION] RULES.md D31, D29: frozen rows changed: updated
/// and deleted, the change seen by its transaction at once and by others
/// once it commits, a reader of an earlier snapshot reading them as they
/// were; a change rolled back leaves them as they were; a key of a frozen
/// row is kept unique; two transactions changing one frozen row conflict.
#[test]
fn frozen_rows_change_as_any() {
    let db = Database::new();
    let a = both(&db);
    let reader = db.session("A");
    assert_eq!(rows(&reader, "SELECT N FROM C WHERE K = 10"), Ok(vec!["1".to_owned()]), "the reader's snapshot");
    a.run("UPDATE C SET N = -1 WHERE K BETWEEN 10 AND 12").unwrap();
    a.run("DELETE FROM C WHERE K = 20").unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM C WHERE K IN (10, 20)"), Ok(vec!["10,-1".to_owned()]));
    a.commit().unwrap();
    assert_eq!(rows(&reader, "SELECT K, N FROM C WHERE K IN (10, 20)"), Ok(vec!["10,1".to_owned(), "20,2".to_owned()]), "as it was");
    reader.commit().unwrap();
    assert_eq!(rows(&reader, "SELECT K, N FROM C WHERE K IN (10, 20)"), Ok(vec!["10,-1".to_owned()]), "once committed");
    a.run("DELETE FROM C WHERE K < 100").unwrap();
    a.rollback().unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM C WHERE K < 100"), Ok(vec!["99".to_owned()]), "rolled back");
    assert_eq!(exec(&a, "INSERT INTO C VALUES (30, 0, 'x', 0)").err(), Some(Code::Constraint), "a frozen key is kept unique");
    a.commit().unwrap();
    let other = db.session("A");
    a.run("UPDATE C SET N = 0 WHERE K = 40").unwrap();
    assert_eq!(exec(&other, "UPDATE C SET N = 1 WHERE K = 40").err(), Some(Code::Conflict));
    a.commit().unwrap();
    assert_eq!(exec(&other, "UPDATE C SET N = 1 WHERE K = 40").err(), Some(Code::Conflict), "a write over a newer commit");
    other.rollback().unwrap();
    assert_eq!(exec(&other, "UPDATE C SET N = 1 WHERE K = 40").map(|_| ()), Ok(()), "a transaction begun after");
}

/// The database in `dir`, opened as after a crash of any that has it open.
fn open(dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D31, D25: a table of the column store
/// is recovered: frozen rows and rows changed after, when the database
/// stopped without closing, and by the checkpoint of a clean close.
#[test]
fn column_tables_are_recovered() {
    let scratch = Scratch::new("column-layout");
    let dir = scratch.path("db");
    let crashed = open(&dir);
    let a = both(&crashed);
    a.run("UPDATE C SET N = -1 WHERE K < 5").unwrap();
    a.run("DELETE FROM C WHERE K BETWEEN 100 AND 199").unwrap();
    a.commit().unwrap();
    crashed.checkpoint().unwrap();
    a.run("UPDATE C SET N = -2 WHERE K BETWEEN 5 AND 9").unwrap();
    a.run("UPDATE R SET N = -1 WHERE K < 5").unwrap();
    a.run("UPDATE R SET N = -2 WHERE K BETWEEN 5 AND 9").unwrap();
    a.run("DELETE FROM R WHERE K BETWEEN 100 AND 199").unwrap();
    a.commit().unwrap();
    let db = open(&dir);
    let s = db.session("A");
    assert_eq!(answers(&s, "C"), answers(&s, "R"), "after a crash");
    s.commit().unwrap();
    db.close().unwrap();
    drop((s, db));
    let db = open(&dir);
    let s = db.session("A");
    assert_eq!(answers(&s, "C"), answers(&s, "R"), "after a clean close");
    drop((s, db, a, crashed));
}

/// Verifies: [EXTENSION] RULES.md D31, D41: ALTER TABLE ... USING makes a
/// table anew in the other store, its rows kept, its options the new
/// store's; the column store's options are refused of the row store's
/// table; an index made on a table of frozen rows finds them.
#[test]
fn a_table_changes_its_store() {
    let db = Database::new();
    let a = both(&db);
    assert_eq!(
        exec(&a, "CREATE TABLE X (K INTEGER) WITH (COLUMNSTORE.GROUP_ROWS = 4096)").err(),
        Some(Code::SyntaxRule),
        "not the row store's"
    );
    db.create_index("C_N", "A.C", &["N"], "btree").unwrap();
    assert_eq!(rows(&a, "SELECT K FROM C WHERE N = 250"), rows(&a, "SELECT K FROM R WHERE N = 250"));
    a.run("ALTER TABLE C USING rowstore").unwrap();
    a.commit().unwrap();
    assert_eq!(answers(&a, "C"), answers(&a, "R"));
    a.run("ALTER TABLE R USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 4096)").unwrap();
    a.commit().unwrap();
    assert_eq!(answers(&a, "C"), answers(&a, "R"));
    let engines =
        rows(&a, "SELECT TABLE_NAME, ENGINE_NAME FROM INFORMATION_SCHEMA.TABLE_OPTIONS WHERE OPTION_NAME = 'COLUMNSTORE.GROUP_ROWS'");
    assert_eq!(engines, Ok(vec!["R,columnstore".to_owned()]));
}

/// The bytes of the files of the database in `dir` whose names begin with
/// `data`: the row store's file and its shadows.
fn data_bytes(dir: &Path) -> Vec<u8> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("data")) && path.is_file() {
            all.extend(std::fs::read(&path).unwrap());
        }
    }
    all
}

/// The database in `dir`, the test encodings registered.
fn sealing(dir: &Path) -> Database {
    let db = Database::new();
    db.register_encoding(Box::new(Scramble)).unwrap();
    db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D31, D48: a table of the column store
/// whose option ENCODINGS names an encoding seals its groups' pages as it
/// seals its rows: once its rows are frozen and checkpointed, the data
/// file holds none of their text, and they read back after a reopening.
#[test]
fn frozen_pages_pass_through_the_tables_encodings() {
    let scratch = Scratch::new("column-encodings");
    let dir = scratch.path("db");
    let db = sealing(&dir);
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER, S VARCHAR(30))
           USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048, ENCODINGS = 'test scramble')",
    )
    .unwrap();
    let s = db.session("S");
    for first in (0..3000).step_by(500) {
        let values: Vec<String> = (first..first + 500).map(|k| format!("({k}, 'secret {k}')")).collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    db.checkpoint().unwrap();
    assert!(!data_bytes(&dir).windows(7).any(|w| w == b"secret "), "no row of T is stored as it is");
    drop(s);
    db.close().unwrap();
    let again = sealing(&dir);
    let s = again.session("S");
    assert_eq!(rows(&s, "SELECT COUNT(*), MAX(S) FROM T WHERE K < 3000"), Ok(vec!["3000,secret 999".to_owned()]));
    s.commit().unwrap();
    again.close().unwrap();
}

/// Verifies: [DESIGN] RULES.md D31, D35: the planner's costs of a table of
/// the column store: a scan of a few of its columns costs less than of a
/// table of the row store, so a fifth of its rows is counted by a scan where a
/// table of the row store asks its index; a few rows are asked of the index in
/// either.
#[test]
fn a_column_table_is_scanned_where_a_row_table_is_probed() {
    let db = Database::new();
    let a = both(&db);
    db.create_index("R_N", "A.R", &["N"], "btree").unwrap();
    db.create_index("C_N", "A.C", &["N"], "btree").unwrap();
    let physical = |sql: &str| physical(&a, sql);
    // A fifth of the rows: N is k / 10, so N below 100 is k below 1000.
    let fifth = "SELECT COUNT(*) FROM {t} WHERE N BETWEEN 0 AND 99";
    assert!(physical(&fifth.replace("{t}", "R")).contains("IndexScan"), "rows: the index");
    assert!(!physical(&fifth.replace("{t}", "C")).contains("IndexScan"), "columns: a scan");
    let few = "SELECT COUNT(*) FROM {t} WHERE N = 7";
    assert!(physical(&few.replace("{t}", "C")).contains("IndexScan"), "a few rows: the index");
    assert_eq!(rows(&a, &fifth.replace("{t}", "C")), rows(&a, &fifth.replace("{t}", "R")));
}

/// The plan `s` makes of `sql`, as optimized.
fn physical(s: &Session, sql: &str) -> String {
    let plan = s.explain(sql, &turborel::params::Params::new()).unwrap();
    plan.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// Verifies: [DESIGN] RULES.md D31: the database's maintenance thread
/// freezes a table's rows once committed, no call asking it to: a scan of
/// a fifth of them is then planned where their index was.
#[test]
fn rows_are_frozen_in_the_background() {
    let db = Database::new();
    let a = written(&db);
    db.create_index("C_N", "A.C", &["N"], "btree").unwrap();
    let fifth = "SELECT COUNT(*) FROM C WHERE N BETWEEN 0 AND 99";
    let until = Instant::now().checked_add(Duration::from_secs(30)).unwrap();
    while physical(&a, fifth).contains("IndexScan") {
        assert!(Instant::now() < until, "frozen within 30 seconds");
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(rows(&a, fifth), Ok(vec!["1000".to_owned()]));
}

/// A table C of schema A, of the column store in groups of 2048 rows:
/// K, a key; M, K's remainder by 3.
fn table_c(db: &Database) -> Session {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE C (K INTEGER PRIMARY KEY, M INTEGER)
           USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    db.session("A")
}

/// Inserts into C, by `s`, the rows of K from `first` to `last`.
fn insert_c(s: &Session, (first, last): (i64, i64)) {
    let values: Vec<String> = (first..=last).map(|k| format!("({k}, {})", k % 3)).collect();
    s.run(&format!("INSERT INTO C VALUES {}", values.join(", "))).unwrap();
}

/// Verifies: [DESIGN] RULES.md D31, D29: rows changed as they are frozen:
/// a writer deletes and updates some of the rows it just committed as the
/// maintenance thread freezes them, never refused; the rows deleted stay
/// deleted, once their deletions are collected and their groups written.
#[test]
fn rows_changed_as_they_are_frozen_stay_changed() {
    let db = Database::new();
    let w = table_c(&db);
    let mut deleted = Vec::new();
    for round in 0..6_i64 {
        let first = round.checked_mul(6000).unwrap();
        insert_c(&w, (first, first.checked_add(5999).unwrap()));
        w.commit().unwrap();
        // A few, soon done, of a group the freeze reads after its first.
        let from = first.checked_add(3000).unwrap();
        let range = format!("K + 0 BETWEEN {from} AND {}", from.checked_add(199).unwrap());
        w.run(&format!("DELETE FROM C WHERE {range} AND M = 0")).unwrap();
        w.run(&format!("UPDATE C SET M = 3 WHERE {range} AND M = 1")).unwrap();
        w.commit().unwrap();
        deleted.push(range);
    }
    db.maintain().unwrap();
    let left = format!("SELECT COUNT(*) FROM C WHERE M = 0 AND ({})", deleted.join(" OR "));
    assert_eq!(rows(&w, &left), Ok(vec!["0".to_owned()]), "deleted");
    assert_eq!(rows(&w, "SELECT COUNT(*), SUM(M) FROM C"), Ok(vec!["35598,36804".to_owned()]));
}

/// Whether `got`, a count and a sum of K, is of the rows of K from 0 on,
/// a multiple of 1000 of them: a state the writer of
/// `rows_read_as_they_are_frozen_are_read_once` commits.
fn first_thousands(got: &[String]) -> bool {
    let Some((count, sum)) = got.first().and_then(|r| r.split_once(',')) else { return false };
    let Ok(count) = count.parse::<i64>() else { return false };
    let all = count.checked_mul(count.saturating_sub(1)).and_then(|n| n.checked_div(2));
    let expected = if count == 0 { Some("NULL".to_owned()) } else { all.map(|n| n.to_string()) };
    count % 1000 == 0 && expected.as_deref() == Some(sum)
}

/// Verifies: [DESIGN] RULES.md D31: rows frozen as they are read: a reader
/// of a table of the column store counts the rows of a state a writer
/// committed, by batch and row by row alike, none missed nor seen twice,
/// while the maintenance thread freezes them, and takes out their entries.
#[test]
fn rows_read_as_they_are_frozen_are_read_once() {
    let db = Database::new();
    let w = table_c(&db);
    let done = AtomicBool::new(false);
    thread::scope(|scope| {
        let reader = scope.spawn(|| {
            let r = db.session("A");
            let mut reads = 0_u32;
            while !done.load(Ordering::Acquire) {
                let by_batch = rows(&r, "SELECT COUNT(*), SUM(K) FROM C").unwrap();
                let by_row = rows(&r, "SELECT COUNT(K + 0), SUM(K) FROM C").unwrap();
                r.commit().unwrap();
                assert_eq!(by_batch, by_row, "one snapshot, either way");
                assert!(first_thousands(&by_batch), "a state committed: {by_batch:?}");
                reads = reads.saturating_add(1);
            }
            reads
        });
        for round in 0..40_i64 {
            let first = round.checked_mul(1000).unwrap();
            insert_c(&w, (first, first.checked_add(999).unwrap()));
            w.commit().unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        done.store(true, Ordering::Release);
        assert!(reader.join().unwrap() > 0, "the reader read");
    });
}
