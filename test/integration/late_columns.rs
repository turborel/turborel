//! \[DESIGN\] Late materialization of the column store (RULES.md D360,
//! D361): a scan judging the conjuncts it is told of their own columns
//! and decoding the others at the rows kept alone, and columns no
//! operator reads before the query's rows are made carried through
//! filters and joins as the places of their rows. Each query gives what a
//! table of the row store gives and what it gives read a row at a time:
//! of nulls, of rows changed and deleted after they were frozen, of a
//! transaction's own changes and an older snapshot, of errors raised as a
//! row at a time raises them, run in parallel, and spilled.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{both_ways, exec, rows};

/// Rows of each table.
const ROWS: i64 = 6000;

/// The values of row `k`: K its key; C each row's own, scattered; A an
/// integer, null of every 9th; D a price; E an approximate number; F a
/// date; T one of 40 texts, null of every 13th; U a text of each row,
/// alike in its parts; S a CHARACTER string; N the rows in order, tenths.
fn row(k: i64) -> String {
    let c = (k * 7919) % ROWS;
    let a = if k % 9 == 0 { "NULL".to_owned() } else { (c % 50).to_string() };
    let t = if k % 13 == 0 { "NULL".to_owned() } else { format!("'name {}'", c % 40) };
    let date = format!("DATE '{}-{:02}-{:02}'", 1992 + c % 7, 1 + k % 12, 1 + c % 28);
    format!(
        "({k}, {c}, {a}, {}.{:02}, {}.5, {date}, {t}, 'customer{k}@mail{}.example.com', 's{}', {})",
        c % 11,
        c % 100,
        c % 97,
        k % 7,
        k % 31,
        k.checked_div(10).unwrap()
    )
}

/// Schema A with R, of the row store, and C, of the column store in groups
/// of 2048 rows, alike, `ROWS` rows each, C's frozen, and 100 more rows
/// each, not frozen; background work done only when asked (JOBS.POLICY
/// MANUAL), so what is read is the same each run.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let columns = "(K INTEGER, C INTEGER, A INTEGER, D DECIMAL(9,2), E DOUBLE PRECISION, F DATE, T VARCHAR(20), U VARCHAR(60), S CHAR(10), N INTEGER)";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R {columns}
           CREATE TABLE C {columns} USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)"
    ))
    .unwrap();
    let a = db.session("A");
    let insert = |(from, to): (i64, i64)| {
        for table in ["R", "C"] {
            let values: Vec<String> = (from..to).map(row).collect();
            exec(&a, &format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    };
    (0..ROWS).step_by(500).for_each(|first| insert((first, first + 500)));
    a.commit().unwrap();
    db.maintain().unwrap();
    insert((ROWS, ROWS + 100));
    a.commit().unwrap();
    (db, a)
}

/// The queries each table answers alike, `{t}` its name.
const QUERIES: [&str; 31] = [
    "SELECT SUM(A), SUM(D), SUM(E) FROM {t} WHERE C < 60",
    "SELECT K, A, D, T, U, S FROM {t} WHERE C < 60",
    "SELECT K, U FROM {t} WHERE C BETWEEN 100 AND 3100",
    "SELECT K, A, U FROM {t} WHERE T = 'name 7'",
    "SELECT K, D FROM {t} WHERE T IN ('name 3', 'name 30') AND C < 3000",
    "SELECT K, S FROM {t} WHERE T NOT IN ('name 3', 'name 4')",
    "SELECT K, A FROM {t} WHERE T IS NULL",
    "SELECT K, T FROM {t} WHERE A IS NULL AND C < 600",
    "SELECT K, A FROM {t} WHERE U = 'customer1234@mail2.example.com'",
    "SELECT K, D FROM {t} WHERE U = 'customer77@mail0.example.com   '",
    "SELECT K, D FROM {t} WHERE U < 'customer2' AND C < 1000",
    "SELECT K, T FROM {t} WHERE U IN ('customer7@mail0.example.com', 'customer8@mail1.example.com', 'nobody')",
    "SELECT K, U FROM {t} WHERE S = 's3' AND C < 900",
    "SELECT SUM(E * D) FROM {t} WHERE F >= DATE '1995-01-01' AND F < DATE '1996-01-01' AND D BETWEEN 5 AND 7 AND A < 24",
    "SELECT K, U FROM {t} WHERE E > 90.5 OR E IS NULL",
    "SELECT K, U FROM {t} WHERE D = 3.07",
    "SELECT K, U FROM {t} WHERE N = 77 OR N = 4000",
    "SELECT K, U, S FROM {t} WHERE C * 2 < 120",
    "SELECT T, COUNT(*), SUM(A), MAX(U) FROM {t} WHERE C < 600 GROUP BY T",
    "SELECT DISTINCT S FROM {t} WHERE C < 300",
    "SELECT K, U FROM {t} WHERE C < 90 ORDER BY U FETCH FIRST 5 ROWS ONLY",
    "SELECT K, A + 1, U FROM {t} WHERE C < 60",
    "SELECT X.K, X.U, R.T FROM {t} X JOIN R ON X.K = R.K WHERE R.C < 60",
    "SELECT X.U, X.S, Y.D FROM {t} X JOIN R Y ON X.N = Y.K WHERE X.C < 600",
    "SELECT K, U FROM {t} WHERE C < 30 UNION SELECT K, U FROM {t} WHERE C BETWEEN 5990 AND 6000",
    "SELECT K FROM {t} WHERE C < 300 AND U LIKE '%mail3%'",
    "SELECT COUNT(A), SUM(A), COUNT(*) FROM {t} WHERE (A = 1 OR A IS NULL) AND C < 3000",
    "SELECT SUM(E), MIN(E), COUNT(*) FROM {t} WHERE E < 3.5",
    "SELECT SUM(D), MAX(D), COUNT(D) FROM {t} WHERE D < 0.30",
    "SELECT Y.S, COUNT(*), SUM(X.A), MAX(X.U) FROM {t} X JOIN R Y ON X.N = Y.K WHERE Y.C < 900 GROUP BY Y.S",
    "SELECT COUNT(*), SUM(X.D), MIN(X.T) FROM {t} X JOIN R Y ON X.K = Y.K JOIN R Z ON X.N = Z.K WHERE Z.C < 2000 AND X.A < 40",
];

/// What `s` reads of table `table` by each query, sorted.
fn answers(s: &Session, table: &str) -> Vec<Vec<String>> {
    QUERIES.iter().map(|q| read(s, &q.replace("{t}", table))).collect()
}

/// The rows of `sql`, sorted.
fn read(s: &Session, sql: &str) -> Vec<String> {
    let mut found = rows(s, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    found.sort();
    found
}

/// Asserts C answers each query as R does, and as it does read a row at a
/// time, `what` saying when.
fn alike(s: &Session, what: &str) {
    for q in QUERIES {
        let batched = read(s, &q.replace("{t}", "C"));
        let (_, by_rows) = both_ways(s, &q.replace("{t}", "C"));
        assert_eq!(batched, read(s, &q.replace("{t}", "R")), "{what}: {q}");
        assert_eq!(batched, by_rows, "{what}, a row at a time: {q}");
    }
}

/// Verifies: [DESIGN] RULES.md D360, D361, late materialization: a column
/// store table gives what a table of the row store gives, and what it
/// gives read a row at a time, of filters of each kind its scan judges,
/// of integers, decimals, approximate numbers, dates, a dictionary of
/// texts, texts coded by symbols, padded and not found, and runs; of
/// nulls; of a filter it is not told; grouped, sorted, distinct, joined,
/// of a union; of a grouping over one join and over two, whose probes
/// carry its unread columns late (D1081); its rows frozen and not.
#[test]
fn late_columns_answer_as_rows() {
    let (_db, a) = tables();
    alike(&a, "frozen");
}

/// Verifies: [DESIGN] RULES.md D360, D361 [X3.135-1986 §4.16], late
/// materialization of rows changed after they were frozen: rows updated
/// and deleted, 1% each, by a transaction not committed, read by it as
/// changed; then committed, read so by a new transaction, and as they
/// were by a transaction whose snapshot is older.
#[test]
fn late_columns_of_changed_rows_answer_as_rows() {
    let (db, a) = tables();
    crate::common::serializable(&db);
    let old = db.session("A");
    let before = answers(&old, "C");
    for t in ["R", "C"] {
        exec(&a, &format!("UPDATE {t} SET A = A + 1000, U = 'changed', T = 'name 7' WHERE C < 60 OR C BETWEEN 3000 AND 3030")).unwrap();
        exec(&a, &format!("DELETE FROM {t} WHERE C BETWEEN 60 AND 120 OR T = 'name 9'")).unwrap();
    }
    alike(&a, "not committed");
    assert_eq!(answers(&old, "C"), before, "unseen by another");
    a.commit().unwrap();
    alike(&a, "committed");
    assert_eq!(answers(&old, "C"), before, "unseen by an older snapshot");
}

/// Verifies: [DESIGN] RULES.md D361, D15, errors of late columns: an
/// expression of a column read late raises of a row kept, as a row at a
/// time and the row store raise it, a data exception; not of a row its filter
/// drops.
#[test]
fn late_columns_raise_as_rows() {
    let (_db, a) = tables();
    for (q, expected) in [
        ("SELECT K, 100 / (A - 7) FROM {t} WHERE C < 600", Err(Code::Data)),
        ("SELECT K, 100 / (A - 7) FROM {t} WHERE C < 600 AND A <> 7", Ok(())),
        ("SELECT SUM(100 / (A - 7)) FROM {t} WHERE C < 600", Err(Code::Data)),
        ("SELECT SUM(100 / (A - 7)) FROM {t} WHERE C < 600 AND A <> 7", Ok(())),
    ] {
        let of = |t: &str| rows(&a, &q.replace("{t}", t)).map(|mut r| r.sort()).map(drop);
        assert_eq!(of("C"), expected, "{q}");
        assert_eq!(of("R"), expected, "{q}, the row store");
        exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
        assert_eq!(of("C"), expected, "{q}, a row at a time");
        exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    }
}

/// Verifies: [DESIGN] RULES.md D360, D361, late materialization run in
/// parallel, by morsels of 256 rows on 4 threads, and spilled, its work
/// area at its least: each query gives what it gives alone and in memory.
#[test]
fn late_columns_answer_in_parallel_and_spilled() {
    let (db, a) = tables();
    let alone = answers(&a, "C");
    for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
        exec(&a, set).unwrap();
    }
    assert_eq!(answers(&a, "C"), alone, "in parallel");
    exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(64 << 10, 0))).unwrap();
    assert_eq!(answers(&a, "C"), alone, "spilled");
}
