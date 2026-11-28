//! The first filter over a column store's scan judges only the rows the
//! scan kept of each batch, and passes over the conjuncts the scan says it
//! judged exactly of them [RULES.md D1266]: the rows given are those the
//! filter alone would give, of every kind of conjunct a scan is told, of
//! rows written and changed after the freeze, of runs judged in part and
//! of nulls.

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows, scanned};

/// The rows written to each table before the freeze, four groups of the
/// column store's.
const MANY: usize = 8192;

/// One row's values, in the tables' order of columns, as SQL writes them:
/// N one of a hundred numbers, null every 7th row; D a number of two
/// decimal places; F an approximate number, null every 13th; DT a date; T
/// a truth value, null every 19th; U a web address of the row's own, null
/// every 11th, and B the same; W one of six words, null every 17th, and C
/// the same, padded; P a text of the row's own.
fn values(k: usize) -> String {
    let null_or = |null: bool, v: String| if null { "NULL".to_owned() } else { v };
    let word = if k % 50 == 3 { "a rare one" } else { ["red", "green", "blue", "a green one", "grey"][k % 5] };
    let part = if k % 97 == 5 { "find-me" } else { "other" };
    let u = null_or(k.is_multiple_of(11), format!("'http://site{}.example/{part}/page{}'", (k * 7919) % 300, (k * 104_729) % 50_000));
    format!(
        "({k}, {n}, {d}.{c:02}, {f}, DATE '1998-{month:02}-{day:02}', {t}, {u}, {u}, {w}, '{word}', 'payload of row {k}')",
        n = null_or(k.is_multiple_of(7), (k % 100).to_string()),
        d = (k * 37) % 20,
        c = (k * 13) % 100,
        f = null_or(k.is_multiple_of(13), format!("{}.5E0", (k * 31) % 64)),
        month = k % 12 + 1,
        day = k % 28 + 1,
        t = null_or(k.is_multiple_of(19), if k.is_multiple_of(3) { "TRUE" } else { "FALSE" }.to_owned()),
        w = null_or(k.is_multiple_of(17), format!("'{word}'")),
    )
}

/// Tables R, of the row store, whose scan judges nothing and says
/// nothing, and C, of the column store in groups of 2,048 rows, alike:
/// `MANY` rows written, then frozen.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    let columns = "(K INTEGER PRIMARY KEY, N INTEGER, D DECIMAL(9, 2), F DOUBLE PRECISION, DT DATE, T BOOLEAN, U VARCHAR(80), \
                   B VARCHAR(80) COLLATE A.BYTES, W VARCHAR(20), C CHARACTER(12), P VARCHAR(40))";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE COLLATION BYTES FOR SQL_TEXT FROM UCS_BASIC NO PAD
           CREATE TABLE R {columns}
           CREATE TABLE C {columns} USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)"
    ))
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..MANY).step_by(512) {
            let written: Vec<String> = (first..first + 512).map(values).collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", written.join(", "))).unwrap();
        }
        a.commit().unwrap();
    }
    db.maintain().unwrap();
    (db, a)
}

/// Rows written, changed and deleted after the freeze, in both tables,
/// and not frozen: the column store's scan gives them from its tree,
/// judged by the filter alone.
fn changed(a: &Session) {
    for table in ["R", "C"] {
        let written: Vec<String> = (20_000..20_300).map(values).collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", written.join(", "))).unwrap();
        a.run(&format!("UPDATE {table} SET N = 99 - N, W = 'grey', U = 'http://moved.example/find-me' WHERE MOD(K, 37) = 4")).unwrap();
        a.run(&format!("UPDATE {table} SET N = NULL, F = 1.5E0, T = TRUE WHERE MOD(K, 53) = 9")).unwrap();
        a.run(&format!("DELETE FROM {table} WHERE MOD(K, 41) = 5")).unwrap();
        a.commit().unwrap();
    }
}

/// The ways a filter over a scan runs: by batches, a row at a time, and
/// in parallel; each the settings of a session of its own.
const WAYS: [&[&str]; 3] = [
    &[],
    &["SET EXECUTOR.BATCHES = 'OFF'"],
    &["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"],
];

/// A session of `db` with the settings `set`.
fn session(db: &Database, set: &[&str]) -> Session {
    let a = db.session("A");
    for setting in set {
        exec(&a, setting).unwrap();
    }
    a
}

/// Asserts the keys of the rows of C that `predicate` keeps, each way a
/// filter runs, are those of R, whose scan says nothing, so that its
/// filter judges every conjunct of every row; and gives how many.
fn as_the_row_store(db: &Database, predicate: &str) -> usize {
    let wanted = rows(&db.session("A"), &format!("SELECT K FROM R WHERE {predicate} ORDER BY K")).unwrap();
    for set in WAYS {
        let kept = rows(&session(db, set), &format!("SELECT K FROM C WHERE {predicate} ORDER BY K")).unwrap();
        assert_eq!(kept, wanted, "{set:?}: {predicate}");
    }
    wanted.len()
}

/// Conjuncts of every kind a scan is told, alone and together, and some
/// it is not told beside them, a comparison of two columns among them:
/// comparisons of exact and approximate
/// numbers, dates, truth values and strings, padded and by octets; IS
/// NULL and IS NOT NULL; IN and NOT IN, short and long; ORs of them; LIKE
/// and NOT LIKE that keep few rows, and that keep most, which the scan
/// stops judging part way through a run.
const PREDICATES: [&str; 48] = [
    "N < 50",
    "N = 7",
    "N <> 3",
    "N >= 10 AND N < 90",
    "50 > N",
    "D > 12.345",
    "D = 10.50",
    "D <= 10",
    "D <> 3.333",
    "F > 2.5E-1",
    "F <= 5",
    "F = 31.5E0",
    "DT >= DATE '1998-06-15'",
    "DT < DATE '1998-02-03' AND N > 20",
    "T = TRUE",
    "T <> TRUE",
    "N IS NULL",
    "N IS NOT NULL",
    "W IS NULL",
    "U IS NOT NULL AND W IS NOT NULL",
    "N IN (1, 2, 3, 50, 51)",
    "N NOT IN (1, 2, 3)",
    "N IN (1, 3, 5, 7, 9, 11, 13, 15, 17, 19, 21, 23)",
    "N NOT IN (1, 3, 5, 7, 9, 11, 13, 15, 17, 19, 21, 23)",
    "W IN ('red', 'grey')",
    "W NOT IN ('red', 'a rare one')",
    "N = 1 OR N = 2 OR N IS NULL",
    "N < 5 OR N > 95",
    "W = 'red' OR W = 'blue'",
    "W = 'red'",
    "W <> 'green'",
    "W < 'c'",
    "C = 'red'",
    "C <> 'grey' AND C >= 'b'",
    "U > 'http://site2'",
    "B > 'http://site2'",
    "B <> ''",
    "U LIKE '%find-me%'",
    "U LIKE 'http://%'",
    "U NOT LIKE '%other%'",
    "W LIKE '%e%' AND N < 80",
    "W NOT LIKE '%rare%' AND U LIKE '%site1%' AND N IS NOT NULL",
    "N + K > 4000 AND N < 50 AND W <> 'red'",
    "K / (N + 1000) >= 0 AND N < 50",
    "N < D AND N < 50 AND W <> 'red'",
    "D < F AND U LIKE 'http://%' AND N IS NOT NULL",
    "U LIKE '%http%'",
    "U LIKE '%site1%'",
];

/// Verifies: [X3.135-1986 §5.11, §5.13, §5.14, §5.15, §5.18] (RULES.md
/// D1266, D15) a filter over a column store's scan that passes over the
/// conjuncts the scan judged exactly gives the rows a filter that judges
/// every conjunct of every row gives, as the row store's does: of each
/// kind of conjunct a scan is told and of conjuncts it is not told
/// beside them, of a condition that can raise an error, which is told
/// nothing, of nulls in every column judged, by batches, a row at a time
/// and in parallel; and the same once rows are written, changed and
/// deleted after the freeze, which the scan gives from its tree
/// unjudged and says nothing of.
#[test]
fn a_filter_that_passes_over_what_the_scan_judged_gives_the_same_rows() {
    let (db, a) = tables();
    let (mut some, mut most) = (0_usize, 0_usize);
    for predicate in PREDICATES {
        let kept = as_the_row_store(&db, predicate);
        some += usize::from(kept > 0 && kept * 8 < MANY);
        most += usize::from(kept * 8 >= MANY && kept < MANY);
    }
    assert!(some > 5 && most > 16, "predicates that keep few rows ({some}) and many ({most}) are both met");
    changed(&a);
    for predicate in PREDICATES {
        as_the_row_store(&db, predicate);
    }
}

/// Verifies: [EXTENSION] (RULES.md D1266, D376) EXPLAIN ANALYZE shows the
/// filter over a column store's scan judging fewer rows: of a condition
/// the scan judges exactly that keeps most rows of every run, the scan
/// gives the filter the rows it kept alone, as many as the filter keeps,
/// where a run given whole gave every row of it; by batches and in
/// parallel. Of a LIKE of no prefix that the scan stops judging part way
/// through a run, beside a conjunct it judges exactly, it gives more than
/// the filter keeps, texts it did not reach among them, and fewer than
/// the table holds. Of such a LIKE alone, of which nothing is judged
/// exactly, and of
/// a conjunct it does not judge, it gives every row, as of a condition that can
/// raise an error (RULES.md D15), which is told nothing and sees every
/// row its scan gives.
#[test]
fn the_filter_is_given_only_the_rows_the_scan_kept() {
    let (db, _) = tables();
    let exactly = ["N < 50", "N IS NOT NULL", "N NOT IN (1, 2, 3)", "W <> 'green'", "DT >= DATE '1998-03-01' AND F > 2.5E-1"];
    for (set, predicate) in [WAYS[0], WAYS[2]].into_iter().flat_map(|set| exactly.map(|p| (set, p))) {
        let a = session(&db, set);
        let kept = rows(&a, &format!("SELECT K FROM C WHERE {predicate}")).unwrap().len();
        assert!(kept * 8 >= MANY && kept < MANY, "{predicate} keeps most rows and not all: {kept}");
        let given = scanned(&a, &format!("SELECT K, P FROM C WHERE {predicate}"));
        assert_eq!(given, kept, "{set:?}: the filter is given the rows the scan kept of {predicate}");
    }
    let a = db.session("A");
    let kept = rows(&a, "SELECT K FROM C WHERE N IS NOT NULL AND U LIKE '%site1%'").unwrap().len();
    let given = scanned(&a, "SELECT K, P FROM C WHERE N IS NOT NULL AND U LIKE '%site1%'");
    let not_null = (0..MANY).filter(|k| !k.is_multiple_of(7)).count();
    assert!(kept < given && given <= not_null, "a LIKE judged in part beside a null test: {given} rows given, {kept} kept");
    assert_eq!(scanned(&a, "SELECT K, P FROM C WHERE U LIKE '%http%'"), MANY, "a LIKE judged in part alone: nothing said");
    assert_eq!(scanned(&a, "SELECT K, P FROM C WHERE N + K >= 0"), MANY, "a conjunct the scan is not told");
    assert_eq!(scanned(&a, "SELECT K, P FROM C WHERE K / (N + 1000) >= 0 AND N < 50"), MANY, "a condition that can raise");
}
