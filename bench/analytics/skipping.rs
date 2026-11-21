//! How much of a column store table its filters skip (RULES.md D510-D514):
//! of HITS, a table shaped as the ClickBench one, its rows frozen, counts
//! and sums by selective ranges of a scattered integer, ranges of dates in
//! the rows' order, prefixes of LIKE and ranges of texts, IN lists and the
//! combinations of these; then the same after a few rows of every group
//! are updated, so the tree shadows them; and rows fetched through an
//! index (RULES.md R-BENCH-1). `ROWS_VAR` sets its rows.

use super::harness::{Bench, Case, scatter};
use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;

/// The variable that sets the rows of HITS, `ROWS` if not set.
const ROWS_VAR: &str = "TURBOREL_BENCH_SKIPPING_ROWS";
/// Rows of HITS by default.
const ROWS: u64 = 4_000_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 2_000;
/// Times a measured run reads its query.
const REPEATS: u64 = 3;
/// Days of HITS' dates.
const DAYS: u64 = 365;

/// The rows of HITS.
fn rows() -> u64 {
    std::env::var(ROWS_VAR).ok().and_then(|v| v.parse().ok()).unwrap_or(ROWS)
}

/// Schema B with HITS, in the column store, its rows frozen.
fn database(rows: u64) -> Database {
    let db = filled(rows, "");
    db.maintain().unwrap();
    db
}

/// Schema B with HITS, in the column store, of `options`, its rows
/// written and none frozen: K its key, in order; EDATE a date, in the
/// rows' order; COUNTER a counter of 100, mostly in order; USERID
/// scattered, each row's own; REGION one of 10,000, scattered; WIDTH one
/// of 20; URL a text of 500 sites and 100,000 pages; PHRASE empty in nine
/// rows of ten, else one of 1,000. Background work runs only when asked.
fn filled(rows: u64, options: &str) -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE HITS (K BIGINT, EDATE DATE, COUNTER INTEGER, USERID BIGINT, REGION INTEGER, WIDTH INTEGER,
             URL VARCHAR(100), PHRASE VARCHAR(40)) USING columnstore {options}"
    ))
    .unwrap();
    let s = db.session("B");
    for first in (0..rows).step_by(PER_INSERT as usize) {
        let values: Vec<String> = (first..(first + PER_INSERT).min(rows)).map(|k| row(k, rows)).collect();
        s.run(&format!("INSERT INTO HITS VALUES {}", values.join(", "))).unwrap();
        s.commit().unwrap();
    }
    db
}

/// The values of HITS' row `k` of `rows`.
fn row(k: u64, rows: u64) -> String {
    let c = scatter(k, rows);
    let day = k * DAYS / rows;
    let (month, date) = (1 + day / 31 % 12, 1 + day % 28);
    let counter = (k * 100 / rows + c % 3) % 100;
    let phrase = if c.is_multiple_of(10) { format!("phrase {}", c / 10 % 1000) } else { String::new() };
    format!(
        "({k}, DATE '2013-{month:02}-{date:02}', {counter}, {c}, {}, {}, 'http://site{}.example/page{}', '{phrase}')",
        c % 10_000,
        (c % 20) * 64,
        c % 500,
        c % 100_000,
    )
}

/// The rows of `sql`, each fetched, counted.
fn fetched(s: &Session, sql: &str) -> u64 {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut rows = 0;
    while cursor.fetch().unwrap().is_some() {
        rows += 1;
    }
    s.commit().unwrap();
    rows
}

/// The queries measured, each named; `{r}` stands for HITS' rows.
const QUERIES: [(&str, &str); 14] = [
    ("COUNT of 0.1% by a scattered range", "SELECT COUNT(*) FROM HITS WHERE USERID < {r} / 1000"),
    ("COUNT of 1% by a scattered range", "SELECT COUNT(*) FROM HITS WHERE USERID < {r} / 100"),
    ("SUM of 10% by a scattered range", "SELECT SUM(WIDTH) FROM HITS WHERE USERID < {r} / 10"),
    ("COUNT of a month of dates in order", "SELECT COUNT(*) FROM HITS WHERE EDATE BETWEEN DATE '2013-07-01' AND DATE '2013-07-31'"),
    ("COUNT by a LIKE prefix of one site", "SELECT COUNT(*) FROM HITS WHERE URL LIKE 'http://site7.example/%'"),
    ("COUNT by a LIKE prefix of 111 sites", "SELECT COUNT(*) FROM HITS WHERE URL LIKE 'http://site1%'"),
    ("COUNT by a range of texts", "SELECT COUNT(*) FROM HITS WHERE PHRASE >= 'phrase 5' AND PHRASE < 'phrase 6'"),
    ("COUNT of non-empty texts", "SELECT COUNT(*) FROM HITS WHERE PHRASE <> ''"),
    ("COUNT by IN of 3 regions", "SELECT COUNT(*) FROM HITS WHERE REGION IN (1, 2, 3)"),
    ("COUNT by an equal text", "SELECT COUNT(*) FROM HITS WHERE PHRASE = 'phrase 77'"),
    (
        "SUM by a counter and a range of dates",
        "SELECT SUM(WIDTH) FROM HITS WHERE COUNTER = 62 AND EDATE BETWEEN DATE '2013-08-01' AND DATE '2013-08-31'",
    ),
    (
        "COUNT by dates, IN and a scattered range",
        "SELECT COUNT(*) FROM HITS WHERE EDATE >= DATE '2013-03-01' AND REGION IN (5, 6, 7, 8) AND USERID < {r} / 10",
    ),
    ("COUNT by a scattered range and a LIKE prefix", "SELECT COUNT(*) FROM HITS WHERE USERID < {r} / 100 AND URL LIKE 'http://site4%'"),
    ("MIN and MAX of texts", "SELECT MIN(PHRASE), MAX(URL) FROM HITS"),
];

/// The choices of what a frozen group keeps whose freezing is measured,
/// each named, as HITS' options.
const FREEZES: [(&str, &str); 2] =
    [("imprints", "WITH (COLUMNSTORE.IMPRINTS = 'ON')"), ("no imprints", "WITH (COLUMNSTORE.IMPRINTS = 'OFF')")];

/// The queries measured again once rows of every group are updated.
const AFTER: [usize; 4] = [1, 3, 8, 11];

/// Measures each query of HITS, then again after updates, and fetches
/// through an index; and last, the freezing of a quarter of its rows,
/// with each choice of what a group keeps.
pub fn workloads(bench: &Bench) {
    let rows = rows();
    let db = database(rows);
    let s = db.session("B");
    s.run("SET EXECUTOR.DEGREE = 1").unwrap();
    let measure = |label: &str, (name, sql): (&str, &str)| {
        let sql = sql.replace("{r}", &rows.to_string());
        bench.measure(Case::new(format!("{name} ({label})"), "rows"), || {
            (0..REPEATS).for_each(|_| {
                fetched(&s, &sql);
            });
            rows * REPEATS
        });
    };
    for query in QUERIES {
        measure("skipping", query);
    }
    // A row of every 4,096 changed, the tree's entries shadowing them.
    s.run("UPDATE HITS SET WIDTH = WIDTH + 1 WHERE MOD(K, 4096) = 7").unwrap();
    s.commit().unwrap();
    for at in AFTER {
        measure("skipping, updated", QUERIES[at]);
    }
    s.run("CREATE INDEX HITS_REGION ON HITS (REGION)").unwrap();
    s.commit().unwrap();
    measure("skipping, index", ("fetch 4 columns of 0.01% through an index", "SELECT K, EDATE, URL, WIDTH FROM HITS WHERE REGION = 4321"));
    measure("skipping, index", ("fetch 2 columns of 0.03% through an index", "SELECT K, WIDTH FROM HITS WHERE REGION IN (11, 12, 13)"));
    for (name, options) in FREEZES {
        let case = Case::new(format!("freeze, {name} (skipping, write)"), "rows");
        bench.measure_fresh(
            case,
            || filled(rows / 4, options),
            |db| {
                db.maintain().unwrap();
                rows / 4
            },
        );
    }
}
