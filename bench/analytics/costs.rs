//! What the planner's weights stand for, measured (RULES.md D35, D650):
//! of CR, a row store table of 1,000,000 rows, and CC, a column store one
//! of the same rows, on one thread, a row scanned by batch and a row at a
//! time, for one column and for all; a row of each operand hashed by a
//! hash join of 1,000,000 rows by 1,000,000; a row grouped into 1,000
//! groups and into 1,000,000; a row sorted, against the same rows read
//! unsorted; each of those past a work area of 1 MiB, its rows spilled;
//! a row of a conjunct judged; and a part run on eight threads, of two
//! sizes, for what starting its workers costs. Each weight is then
//! printed as the planner prices it, in rows of a scan of the row store,
//! so a weight's source is this run (`physical/cost.rs`,
//! `physical/cost/spill.rs`, `physical/cost/parallel.rs`). Run with
//! `cargo xtask bench`, on the harness (RULES.md R-BENCH-1); with
//! `ONLY_VAR` of `analytics.rs` set to `costs`, these alone.

use super::harness::{Bench, Case, note, scatter};
use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;

/// Rows of CR and of CC.
const ROWS: u64 = 1_000_000;
/// Rows of CS and of CSC, the small tables a part of eight threads reads:
/// past EXECUTOR.PARALLEL_MIN_ROWS, so they run in parallel.
const SMALL: u64 = 131_072;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 1_000;
/// The work area past which the spilled cases run, 1 MiB.
const SPILLED_WORK: u64 = 1 << 20;
/// The threads of the parallel cases.
const THREADS: usize = 8;

/// Schema B: CR and CC, `ROWS` rows each, K their key in order, G one of
/// 1,000, H a scattered key of its own, A one of 97, P a filler of 24
/// characters; CS and CSC, the first `SMALL` of their rows.
fn database() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let table = |name: &str, kept: &str| format!("CREATE TABLE {name} (K INTEGER, G INTEGER, H INTEGER, A INTEGER, P VARCHAR(24)){kept}");
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION B {} {} {} {}",
        table("CR", ""),
        table("CC", " USING columnstore"),
        table("CS", ""),
        table("CSC", " USING columnstore")
    ))
    .unwrap();
    let s = db.session("B");
    for first in (0..ROWS).step_by(PER_INSERT as usize) {
        let rows: Vec<String> = (first..first + PER_INSERT).map(row).collect();
        let mut tables = vec!["CR", "CC"];
        if first < SMALL {
            tables.extend(["CS", "CSC"]);
        }
        for table in tables {
            s.run(&format!("INSERT INTO {table} VALUES {}", rows.join(", "))).unwrap();
        }
        s.commit().unwrap();
    }
    db.maintain().unwrap();
    (db, s)
}

/// The values of row `k`.
fn row(k: u64) -> String {
    format!("({k}, {}, {}, {}, 'p{:023}')", k % 1_000, scatter(k, ROWS), k % 97, k)
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

/// The cases of a table, `{t}` in each query standing for it: each name,
/// query and the settings it runs under.
const CASES: [(&str, &str, &str); 14] = [
    ("scan of one column", "SELECT SUM(A) FROM {t}", ""),
    ("scan of one column, a row at a time", "SELECT SUM(A) FROM {t}", "EXECUTOR.BATCHES = OFF"),
    ("scan of all columns", "SELECT SUM(K), SUM(G), SUM(H), SUM(A), MAX(P) FROM {t}", ""),
    ("conjunct judged", "SELECT SUM(A) FROM {t} WHERE G >= 0", ""),
    ("conjunct judged, a row at a time", "SELECT SUM(A) FROM {t} WHERE G >= 0", "EXECUTOR.BATCHES = OFF"),
    ("hash join", "SELECT COUNT(*), SUM(Y.A) FROM {t} X JOIN {t} Y ON X.H = Y.K", ""),
    ("hash join, a row at a time", "SELECT COUNT(*), SUM(Y.A) FROM {t} X JOIN {t} Y ON X.H = Y.K", "EXECUTOR.BATCHES = OFF"),
    ("hash join, spilled", "SELECT COUNT(*), SUM(Y.A) FROM {t} X JOIN {t} Y ON X.H = Y.K", "MEMORY.WORK_BYTES = {w}"),
    ("grouping of 1,000 groups", "SELECT COUNT(*) FROM (SELECT G, SUM(A) FROM {t} GROUP BY G) AS Q", ""),
    ("grouping of 1,000,000 groups", "SELECT COUNT(*) FROM (SELECT H, SUM(A) FROM {t} GROUP BY H) AS Q", ""),
    (
        "grouping of 1,000,000 groups, spilled",
        "SELECT COUNT(*) FROM (SELECT H, SUM(A) FROM {t} GROUP BY H) AS Q",
        "MEMORY.WORK_BYTES = {w}",
    ),
    ("rows read", "SELECT K, A FROM {t}", ""),
    ("rows sorted", "SELECT K, A FROM {t} ORDER BY H", ""),
    ("rows sorted, spilled", "SELECT K, A FROM {t} ORDER BY H", "MEMORY.WORK_BYTES = {w}"),
];

/// Measures `sql` of `rows` rows under `setting`, if one, as `name`, on
/// `threads` threads: the seconds a row takes.
fn timed(bench: &Bench, s: &Session, (name, sql, setting): (&str, &str, &str), (rows, threads): (u64, usize)) -> f64 {
    s.run(&format!("SET EXECUTOR.DEGREE = {threads}")).unwrap();
    let setting = setting.replace("{w}", &SPILLED_WORK.to_string());
    if !setting.is_empty() {
        s.run(&format!("SET {setting}")).unwrap();
    }
    bench.measure(Case::new(name, "rows").threads(threads), || {
        fetched(s, sql);
        rows
    });
    if let Some((key, _)) = setting.split_once(" = ") {
        s.run(&format!("SET {key} TO DEFAULT")).unwrap();
    }
    s.run("SET EXECUTOR.DEGREE TO DEFAULT").unwrap();
    1.0 / bench.rate(name, threads).unwrap()
}

/// The workloads, then the weights they give.
pub fn workloads(bench: &Bench) {
    let (_db, s) = database();
    let mut unit = None;
    for (table, small, label) in [("CR", "CS", ""), ("CC", "CSC", " (columnstore)")] {
        let mut secs = Vec::new();
        for (name, sql, setting) in CASES {
            let case = (format!("{name}{label}"), sql.replace("{t}", table));
            secs.push(timed(bench, &s, (&case.0, &case.1, setting), (ROWS, 1)));
        }
        let scan = secs[0];
        let unit = *unit.get_or_insert(scan);
        let big = timed(bench, &s, (&format!("scan, {THREADS} threads{label}"), &CASES[0].1.replace("{t}", table), ""), (ROWS, THREADS));
        let little = timed(
            bench,
            &s,
            (&format!("scan of {SMALL} rows, {THREADS} threads{label}"), &CASES[0].1.replace("{t}", small), ""),
            (SMALL, THREADS),
        );
        weights(&secs, (unit, scan), (big, little), label);
    }
}

/// Prints the weights the times `secs` of `CASES` give, in rows of the
/// row store's scan `unit`, of a table whose own scan is `scan`, and the
/// start of a part of `THREADS` threads, of the seconds a row of a part
/// of `ROWS` and of `SMALL` rows took.
fn weights(secs: &[f64], (unit, scan): (f64, f64), (big, little): (f64, f64), label: &str) {
    let of = |s: f64| s / unit;
    let n = ROWS as f64;
    note(&format!("weights{label}, in rows of the row store's scan:"));
    note(&format!("  scan row {:.3}, a row at a time {:.3}, all columns {:.3}", of(scan), of(secs[1]), of(secs[2])));
    note(&format!("  conjunct {:.3}, a row at a time {:.3}", of(secs[3] - scan), of(secs[4] - secs[1])));
    // A join's time is of two scans and each row of both hashed.
    let hashed = |join: f64, scanned: f64| of(join - 2.0 * scanned) / 2.0;
    note(&format!("  HASH_ROW {:.3}, a row at a time {:.3}", hashed(secs[5], scan), hashed(secs[6], secs[1])));
    note(&format!("  spilled hash row {:.3}", of(secs[7] - secs[5]) / 2.0));
    note(&format!("  GROUP_ROW few groups {:.3}, a group each {:.3}", of(secs[8] - scan), of(secs[9] - scan)));
    note(&format!("  spilled grouped row {:.3}", of(secs[10] - secs[9])));
    note(&format!("  SORT_COMPARE {:.4}", of(secs[12] - secs[11]) / n.log2()));
    note(&format!("  spilled sorted row {:.3}", of(secs[13] - secs[12])));
    // A part's seconds are its start and its rows' share of each thread:
    // s = start / rows + per_row, of two sizes.
    let per_row = (big * n - little * SMALL as f64) / (n - SMALL as f64);
    note(&format!("  part start of {THREADS} threads {:.0}, a row on them {:.3}", of((little - per_row) * SMALL as f64), of(per_row)));
}
