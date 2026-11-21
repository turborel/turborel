//! How fast hash joins run (RULES.md D368, D369): of JF, a row store table
//! of 1,000,000 rows, and JFC, a column store one of the same rows, joined
//! with a selective build side of 2,000 scattered keys, whose Bloom filter
//! drops 99% of the probe's rows; with one of a narrow range of keys in
//! the probe's order, whose bounds skip the column store's groups; with a
//! large build side of 1,000,000 rows; with one of 4 rows of each key, so
//! each probe row matches 4; on two keys; by LEFT, RIGHT and FULL outer
//! joins; and with the large build side past a work area of 4 MiB, its
//! partitions spilled. Each runs at EXECUTOR.DEGREE 1 and at the system's
//! threads, and the selective ones at EXECUTOR.RUNTIME_FILTERS OFF too.
//! Run with `cargo xtask bench`, on the harness (RULES.md R-BENCH-1); with
//! `ONLY_VAR` of `analytics.rs` set to `joins`, these alone.

use super::harness::{Bench, Case, scatter};
use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;

/// Rows of JF, of JFC and of JL.
const ROWS: u64 = 1_000_000;
/// The keys of H.
const KEYS: u64 = 200_000;
/// Rows of JS, the selective build side, and of JR, the range.
const SELECTIVE: u64 = 2_000;
/// Rows of JD, 4 of each of G's 1,000 values.
const DUPLICATES: u64 = 4_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 1_000;

/// Queries of `{t}`, each with the name it is reported by.
const QUERIES: [(&str, &str); 9] = [
    ("join, 2,000 scattered keys built", "SELECT COUNT(*), SUM({t}.A) FROM {t} JOIN JS ON {t}.H = JS.H"),
    ("join, a range of 2,000 keys built", "SELECT COUNT(*), SUM({t}.A) FROM {t} JOIN JR ON {t}.K = JR.K"),
    ("join, 1,000,000 rows built", "SELECT COUNT(*), SUM(JL.V) FROM {t} JOIN JL ON {t}.K = JL.K"),
    ("join, 4 rows of each key built", "SELECT COUNT(*), SUM(JD.V) FROM {t} JOIN JD ON {t}.G = JD.G"),
    ("join on two keys", "SELECT COUNT(*), SUM(JM.V) FROM {t} JOIN JM ON {t}.G = JM.G AND {t}.A = JM.A"),
    ("LEFT join, 2,000 keys built", "SELECT COUNT(*), SUM(JS.V) FROM {t} LEFT JOIN JS ON {t}.H = JS.H"),
    ("RIGHT join, 2,000 keys built", "SELECT COUNT(*), SUM(JS.V) FROM {t} RIGHT JOIN JS ON {t}.H = JS.H"),
    ("FULL join, 2,000 keys built", "SELECT COUNT(*), SUM(JS.V) FROM {t} FULL JOIN JS ON {t}.H = JS.H"),
    ("join on a decimal key", "SELECT COUNT(*), SUM(JS.V) FROM {t} JOIN JS ON {t}.D = JS.D"),
];

/// Schema B: JF and JFC, `ROWS` rows each, K their key in order, G one of
/// 1,000, H one of `KEYS` scattered, A one of 97, D H as a price; JL, a
/// row of each of their keys; JS, `SELECTIVE` of H's keys scattered; JR,
/// `SELECTIVE` keys of K in a narrow range; JD, 4 rows of each G; JM, a
/// row of each G and A of 50 values. Background work runs only when asked.
fn database() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE JF (K INTEGER, G INTEGER, H INTEGER, A INTEGER, D DECIMAL(9,2))
           CREATE TABLE JFC (K INTEGER, G INTEGER, H INTEGER, A INTEGER, D DECIMAL(9,2)) USING columnstore
           CREATE TABLE JL (K INTEGER, V INTEGER)
           CREATE TABLE JS (H INTEGER, D DECIMAL(9,2), V INTEGER)
           CREATE TABLE JR (K INTEGER, V INTEGER)
           CREATE TABLE JD (G INTEGER, V INTEGER)
           CREATE TABLE JM (G INTEGER, A INTEGER, V INTEGER)",
    )
    .unwrap();
    let s = db.session("B");
    for first in (0..ROWS).step_by(PER_INSERT as usize) {
        let facts: Vec<String> = (first..first + PER_INSERT).map(fact).collect();
        for table in ["JF", "JFC"] {
            s.run(&format!("INSERT INTO {table} VALUES {}", facts.join(", "))).unwrap();
        }
        let built: Vec<String> = (first..first + PER_INSERT).map(|k| format!("({}, {k})", scatter(k, ROWS))).collect();
        s.run(&format!("INSERT INTO JL VALUES {}", built.join(", "))).unwrap();
        s.commit().unwrap();
    }
    let keys = |n: u64| scatter(n * (KEYS / SELECTIVE) + 3, KEYS);
    insert(&s, "JS", (0..SELECTIVE).map(|n| format!("({}, {}.{:02}, {n})", keys(n), keys(n) / 100, keys(n) % 100)));
    insert(&s, "JR", (0..SELECTIVE).map(|n| format!("({}, {n})", ROWS / 2 + n)));
    insert(&s, "JD", (0..DUPLICATES).map(|n| format!("({}, {n})", n % 1_000)));
    insert(&s, "JM", (0..1_000_u64).flat_map(|g| (0..50_u64).map(move |a| format!("({g}, {}, {})", a * 2, g + a))));
    s.commit().unwrap();
    db.maintain().unwrap();
    (db, s)
}

/// The values of JF's row `k`.
fn fact(k: u64) -> String {
    let h = scatter(k, KEYS);
    format!("({k}, {}, {h}, {}, {}.{:02})", k % 1_000, k % 97, h / 100, h % 100)
}

/// Inserts `rows` into `table`, `PER_INSERT` at a time.
fn insert(s: &Session, table: &str, rows: impl Iterator<Item = String>) {
    let rows: Vec<String> = rows.collect();
    for chunk in rows.chunks(PER_INSERT as usize) {
        s.run(&format!("INSERT INTO {table} VALUES {}", chunk.join(", "))).unwrap();
    }
}

/// Rows `sql` gives, each fetched.
fn fetched(s: &Session, sql: &str) -> u64 {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut rows = 0;
    while cursor.fetch().unwrap().is_some() {
        rows += 1;
    }
    s.commit().unwrap();
    rows
}

/// Measures `sql` of `table`, reported as `name`, at each degree of
/// `degrees`.
fn measured(bench: &Bench, s: &Session, (name, sql): (&str, &str), degrees: &[usize]) {
    for &degree in degrees {
        s.run(&format!("SET EXECUTOR.DEGREE = {degree}")).unwrap();
        bench.measure(Case::new(name, "rows").threads(degree), || {
            fetched(s, sql);
            ROWS
        });
    }
    s.run("SET EXECUTOR.DEGREE TO DEFAULT").unwrap();
}

/// The workloads of each table, alone and in parallel, the selective ones
/// without runtime filters too, and the large one past a work area.
pub fn workloads(bench: &Bench) {
    let (_db, s) = database();
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    for (table, label) in [("JF", ""), ("JFC", " (columnstore)")] {
        for (name, sql) in QUERIES {
            measured(bench, &s, (&format!("{name}{label}"), &sql.replace("{t}", table)), &[1, threads]);
        }
        // Of a build without the setting, as it was: the same runs.
        let _ = s.run("SET EXECUTOR.RUNTIME_FILTERS = 'OFF'");
        for (name, sql) in QUERIES.iter().take(2) {
            measured(bench, &s, (&format!("{name}{label}, no runtime filter"), &sql.replace("{t}", table)), &[1]);
        }
        let _ = s.run("SET EXECUTOR.RUNTIME_FILTERS TO DEFAULT");
        s.run("SET MEMORY.WORK_BYTES = 4194304").unwrap();
        let (name, sql) = QUERIES[2];
        measured(bench, &s, (&format!("{name}{label}, 4 MiB work area"), &sql.replace("{t}", table)), &[1]);
        s.run("SET MEMORY.WORK_BYTES TO DEFAULT").unwrap();
    }
}
