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

/// The database in `dir`, the test encoding `Scramble` registered.
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
/// table of the row store, so a seventh of its rows, by a column whose
/// values every block holds, is counted by a scan where a table of the row
/// store, the pool holding it, asks its index; a hundredth is asked of the
/// index in either; three sevenths are counted by a scan in either; and
/// a fifth of the rows by a column in their order is counted by a scan in
/// either, each skipping the blocks its zone maps rule out.
#[test]
fn a_column_table_is_scanned_where_a_row_table_is_probed() {
    let db = Database::new();
    let a = both(&db);
    let indexed =
        [("R_G", "A.R", "G"), ("C_G", "A.C", "G"), ("R_N", "A.R", "N"), ("C_N", "A.C", "N"), ("R_S", "A.R", "S"), ("C_S", "A.C", "S")];
    for (index, table, column) in indexed {
        db.create_index(index, table, &[column], "btree").unwrap();
    }
    db.maintain().unwrap();
    let physical = |sql: &str| physical(&a, sql);
    let some = "SELECT COUNT(*) FROM {t} WHERE G = 3";
    assert!(physical(&some.replace("{t}", "R")).contains("IndexScan"), "rows: the index");
    assert!(!physical(&some.replace("{t}", "C")).contains("IndexScan"), "columns: a scan");
    // S is 's' and K modulo 97.
    let few = "SELECT COUNT(*) FROM {t} WHERE S = 's5'";
    let sevenths = "SELECT COUNT(*) FROM {t} WHERE G <= 2";
    for table in ["R", "C"] {
        assert!(physical(&few.replace("{t}", table)).contains("IndexScan"), "{table}: a hundredth, the index");
        assert!(!physical(&sevenths.replace("{t}", table)).contains("IndexScan"), "{table}: three sevenths, a scan");
    }
    // A fifth of the rows: N is k / 10, so N below 100 is k below 1000.
    let fifth = "SELECT COUNT(*) FROM {t} WHERE N BETWEEN 0 AND 99";
    for table in ["R", "C"] {
        assert!(!physical(&fifth.replace("{t}", table)).contains("IndexScan"), "{table}: zone maps, a scan");
    }
    assert_eq!(rows(&a, &some.replace("{t}", "C")), rows(&a, &some.replace("{t}", "R")));
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
/// committed, by a column and by an expression alike, none missed nor seen twice,
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

/// Verifies: [EXTENSION] RULES.md D31, D41, COLUMNSTORE.ORDER: a table of
/// the column store whose groups are sorted by columns answers as a table
/// of the row store; the option names the table's columns as it does, and
/// refuses a name of none, one named twice, or more than eight; a column
/// it names is not dropped until it names it no more.
#[test]
fn groups_sorted_by_columns_answer_alike() {
    let db = Database::new();
    let a = both(&db);
    a.run("CREATE TABLE O (K INTEGER PRIMARY KEY, G INTEGER, S VARCHAR(20), N INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048, COLUMNSTORE.ORDER = 's, \"G\"')").unwrap();
    a.run("INSERT INTO O SELECT * FROM R").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    assert_eq!(answers(&a, "O"), answers(&a, "R"));
    a.run("UPDATE O SET N = -N WHERE K < 900").unwrap();
    a.run("DELETE FROM O WHERE K BETWEEN 900 AND 999").unwrap();
    a.run("UPDATE R SET N = -N WHERE K < 900").unwrap();
    a.run("DELETE FROM R WHERE K BETWEEN 900 AND 999").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    assert_eq!(answers(&a, "O"), answers(&a, "R"), "groups written again");
    let order = "SELECT OPTION_VALUE FROM INFORMATION_SCHEMA.TABLE_OPTIONS WHERE TABLE_NAME = 'O' AND OPTION_NAME = 'COLUMNSTORE.ORDER'";
    assert_eq!(rows(&a, order), Ok(vec!["S, G".to_owned()]), "as the table names them");
    for refused in ["'Q'", "'S, s'", "'K, G, S, N, K, G, S, N, K'"] {
        assert_eq!(exec(&a, &format!("ALTER TABLE O SET (COLUMNSTORE.ORDER = {refused})")).err(), Some(Code::SyntaxRule), "{refused}");
    }
    assert_eq!(exec(&a, "ALTER TABLE O DROP COLUMN G RESTRICT").err(), Some(Code::SyntaxRule), "ORDER names it");
    a.run("ALTER TABLE O SET (COLUMNSTORE.ORDER = 'S')").unwrap();
    a.run("ALTER TABLE O DROP COLUMN G RESTRICT").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM O"), rows(&a, "SELECT COUNT(*) FROM R"));
}

/// Equalities of S and N, `{t}` the table: of values held and not, padded
/// with spaces, an approximate number equal to an exact one, a list, and
/// the null value, which equals nothing.
const EQUALITIES: [&str; 8] = [
    "SELECT K FROM {t} WHERE S = 's5'",
    "SELECT K FROM {t} WHERE S = 's5   '",
    "SELECT K FROM {t} WHERE S = 'absent'",
    "SELECT K, S FROM {t} WHERE N = 250",
    "SELECT K FROM {t} WHERE N = 2.5E2",
    "SELECT COUNT(*) FROM {t} WHERE N IN (1, 499, 9999)",
    "SELECT COUNT(*) FROM {t} WHERE S IN ('s1', 's96', 'none') AND N > 100",
    "SELECT COUNT(*) FROM {t} WHERE N = CAST(NULL AS INTEGER)",
];

/// Verifies: [EXTENSION] RULES.md D31, D41, COLUMNSTORE.BLOOM: a table of
/// the column store whose groups keep Bloom filters of columns answers as
/// a table of the row store, an equality or IN of one of them too, of
/// values held or not, equal though not alike; once the option names
/// others, its groups are written again and answer alike; it names the
/// table's columns as COLUMNSTORE.ORDER does.
#[test]
fn groups_with_bloom_filters_answer_alike() {
    let db = Database::new();
    let a = both(&db);
    a.run("CREATE TABLE F (K INTEGER PRIMARY KEY, G INTEGER, S VARCHAR(20), N INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048, COLUMNSTORE.BLOOM = 'S, n')").unwrap();
    a.run("INSERT INTO F SELECT * FROM R").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    let equalities = |t: &str| EQUALITIES.iter().map(|q| sorted(&a, &q.replace("{t}", t))).collect::<Vec<_>>();
    assert_eq!(answers(&a, "F"), answers(&a, "R"));
    assert_eq!(equalities("F"), equalities("R"));
    let bloom = "SELECT OPTION_VALUE FROM INFORMATION_SCHEMA.TABLE_OPTIONS WHERE TABLE_NAME = 'F' AND OPTION_NAME = 'COLUMNSTORE.BLOOM'";
    assert_eq!(rows(&a, bloom), Ok(vec!["S, N".to_owned()]), "as the table names them");
    assert_eq!(exec(&a, "ALTER TABLE F SET (COLUMNSTORE.BLOOM = 'Q')").err(), Some(Code::SyntaxRule), "no column Q");
    a.run("ALTER TABLE F SET (COLUMNSTORE.BLOOM = 'G, K')").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    assert_eq!(equalities("F"), equalities("R"), "groups written again");
    assert_eq!(sorted(&a, "SELECT N FROM F WHERE G = 3 AND K = 3"), sorted(&a, "SELECT N FROM R WHERE G = 3 AND K = 3"));
}

/// Queries of approximate numbers, `{t}` the table.
const APPROXIMATE: [&str; 9] = [
    "SELECT SUM(P), AVG(P), MIN(P), MAX(P), COUNT(P), COUNT(*) FROM {t}",
    "SELECT COUNT(*) FROM {t} WHERE P > 50.25E0",
    "SELECT K FROM {t} WHERE P = 12.5E0",
    "SELECT K FROM {t} WHERE P = 12.5",
    "SELECT COUNT(*) FROM {t} WHERE P IN (1.25E0, 3, 0) OR P IS NULL",
    "SELECT COUNT(*) FROM {t} WHERE P NOT IN (1.25E0, 3)",
    "SELECT G, SUM(P), MIN(P), COUNT(P) FROM {t} GROUP BY G",
    "SELECT P, COUNT(*) FROM {t} WHERE K < 300 GROUP BY P",
    "SELECT MAX(Q), MIN(Q), SUM(Q) FROM {t}",
];

/// Verifies: [EXTENSION] RULES.md D31, approximate numbers by batch: a
/// table of the column store of approximate numbers, decimals of few
/// digits, nulls, -0 and numbers of no few digits among them, gives what
/// a table of the row store gives: sums, averages, extremes and counts,
/// comparisons with approximate and exact constants, IN and NOT IN,
/// groups by them and of them [X3.135-1986 §5.8, §5.11 GR4, §5.13].
#[test]
fn approximate_numbers_answer_alike() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE RF (K INTEGER PRIMARY KEY, G INTEGER, P DOUBLE PRECISION, Q REAL)
           CREATE TABLE CF (K INTEGER PRIMARY KEY, G INTEGER, P DOUBLE PRECISION, Q REAL) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["RF", "CF"] {
        let values: Vec<String> = (0..5000_i64)
            .map(|k| {
                let p = match k % 97 {
                    0 => "NULL".to_owned(),
                    1 => "-0.0E0".to_owned(),
                    2 => "3.3333333333333335E-1".to_owned(),
                    _ => format!("{}.{}E0", k % 101, [0, 25, 5, 75][usize::try_from(k % 4).unwrap()]),
                };
                format!("({k}, {}, {p}, {}.1E0)", k % 7, k % 13)
            })
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    for q in APPROXIMATE {
        assert_eq!(sorted(&a, &q.replace("{t}", "CF")), sorted(&a, &q.replace("{t}", "RF")), "{q}");
    }
    a.run("UPDATE CF SET P = 1.25E0 WHERE K < 50").unwrap();
    a.run("UPDATE RF SET P = 1.25E0 WHERE K < 50").unwrap();
    a.commit().unwrap();
    for q in APPROXIMATE {
        assert_eq!(sorted(&a, &q.replace("{t}", "CF")), sorted(&a, &q.replace("{t}", "RF")), "after updates: {q}");
    }
}

/// Queries in an order, `{t}` the table: of the columns it is sorted by,
/// a filter among them, and of others or descending, which it sorts.
const ORDERED: [&str; 6] = [
    "SELECT S, K FROM {t} ORDER BY S",
    "SELECT S, G, N FROM {t} ORDER BY S, G",
    "SELECT S, K FROM {t} WHERE G = 3 AND K > 1000 ORDER BY S",
    "SELECT S, K FROM {t} WHERE S = 's5' ORDER BY S",
    "SELECT S, K FROM {t} ORDER BY S DESC",
    "SELECT N, K FROM {t} ORDER BY N",
];

/// The first column of each row of `sql`, in its order, and all of its
/// rows sorted: two orders give the same if each sorts the same keys
/// alike, whatever order their ties take.
fn in_order(s: &Session, sql: &str) -> (Vec<String>, Vec<String>) {
    let found = rows(s, sql).unwrap();
    let keys = found.iter().map(|r| r.split(',').next().unwrap_or_default().to_owned()).collect();
    let mut all = found;
    all.sort();
    (keys, all)
}

/// Verifies: [EXTENSION] RULES.md D31 [X3.135-1986 §8.3 GR3b], ordered
/// reads: a table of the column store whose groups are sorted by columns
/// gives an ORDER BY of the first of them, ascending, its rows merged as
/// they are read, no sort planned: the rows a table of the row store gives
/// in that order, nulls where ORDER BY puts them, with rows changed,
/// deleted and not yet frozen among them, of a filter too; one descending,
/// or of other columns, sorted; groups sorted otherwise, once the option
/// names others, each sorted as it is read, then written again.
#[test]
fn groups_read_in_their_order_answer_alike() {
    let db = Database::new();
    let a = both(&db);
    a.run("CREATE TABLE O (K INTEGER PRIMARY KEY, G INTEGER, S VARCHAR(20), N INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048, COLUMNSTORE.ORDER = 'S, G')").unwrap();
    a.run("INSERT INTO O SELECT * FROM R").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    let alike = |when: &str| {
        for q in ORDERED {
            assert_eq!(in_order(&a, &q.replace("{t}", "O")), in_order(&a, &q.replace("{t}", "R")), "{when}: {q}");
        }
    };
    alike("frozen");
    let plan = physical(&a, "SELECT S, K FROM O ORDER BY S");
    assert!(plan.contains("in order of") && !plan.contains("Sort"), "{plan}");
    assert!(physical(&a, "SELECT S, K FROM O ORDER BY S DESC").contains("Sort"));
    for table in ["O", "R"] {
        a.run(&format!("UPDATE {table} SET S = 'a changed' WHERE K < 40")).unwrap();
        a.run(&format!("DELETE FROM {table} WHERE K BETWEEN 900 AND 960")).unwrap();
        a.run(&format!("INSERT INTO {table} VALUES (90001, 1, 'z new', 1), (90002, 2, NULL, 2), (90003, 3, 'b new', 3)")).unwrap();
    }
    alike("changed, not yet frozen");
    a.commit().unwrap();
    alike("committed");
    // Named others, its groups sorted otherwise until written again: each
    // read and sorted.
    a.run("ALTER TABLE O SET (COLUMNSTORE.ORDER = 'G')").unwrap();
    a.commit().unwrap();
    let by_g = "SELECT G, K FROM {t} ORDER BY G";
    assert!(physical(&a, &by_g.replace("{t}", "O")).contains("in order of"));
    assert_eq!(in_order(&a, &by_g.replace("{t}", "O")), in_order(&a, &by_g.replace("{t}", "R")), "groups sorted otherwise");
    db.maintain().unwrap();
    assert_eq!(in_order(&a, &by_g.replace("{t}", "O")), in_order(&a, &by_g.replace("{t}", "R")), "groups written again");
}
