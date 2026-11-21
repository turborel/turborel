//! The analytic queries the kernels run in, A/B (RULES.md D570): two
//! databases of the same column store rows, one on `Simd::detect()`, one
//! on the portable kernels, each query run on one then the other in turn,
//! on one thread; nanoseconds a row read, the median of the rounds, and
//! the speed of the detected level against the portable one. Run with
//! `cargo run --release -p turborel-simd --example queries`; `ROUNDS` sets
//! the rounds (default 7), `ONLY` a substring of the queries' names.
#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::print_stdout,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

use std::time::Instant;

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;
use turborel_simd::Simd;

/// Rows of the table.
const ROWS: u64 = 1_000_000;

/// Rows a statement inserts.
const PER_INSERT: u64 = 2_000;

/// Times each query is run a round.
const REPEATS: u64 = 5;

/// A value of `k` scattered over `n`, a multiply by an odd constant.
fn scatter(k: u64, n: u64) -> u64 {
    k.wrapping_mul(0x9E37_79B9_7F4A_7C15) % n
}

/// A database of the table DC, on the SIMD kernels if `simd`.
fn database(simd: bool) -> Database {
    let db = Database::new();
    if simd {
        db.register_kernels(Box::new(Simd::detect())).unwrap();
    }
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE DC (K INTEGER, P DOUBLE PRECISION, F INTEGER, D BIGINT, N INTEGER, T VARCHAR(20), Q DECIMAL(12,2))
             USING columnstore",
    )
    .unwrap();
    let s = db.session("B");
    for first in (0..ROWS).step_by(PER_INSERT as usize) {
        let rows: Vec<String> = (first..first + PER_INSERT)
            .map(|k| {
                let c = scatter(k, ROWS);
                let n = if k % 10 == 0 { "NULL".to_owned() } else { (c % 1_000).to_string() };
                format!(
                    "({k}, {}.{:02}, {}, {}, {n}, 'name {}', {}.{:02})",
                    c % 5_000,
                    c % 100,
                    c % 2_000_000,
                    (c % 1_024) * 1_000_000_007,
                    c % 300,
                    c % 100_000,
                    k % 100
                )
            })
            .collect();
        s.run(&format!("INSERT INTO DC VALUES {}", rows.join(", "))).unwrap();
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    s.run("SET EXECUTOR.DEGREE = 1").unwrap();
    db
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

/// Nanoseconds a row read of `sql` run `REPEATS` times on `s`.
fn timed(s: &Session, sql: &str) -> f64 {
    let start = Instant::now();
    for _ in 0..REPEATS {
        fetched(s, sql);
    }
    start.elapsed().as_nanos() as f64 / (ROWS * REPEATS) as f64
}

/// The median of `times`.
fn median(mut times: Vec<f64>) -> f64 {
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

fn main() {
    let rounds: usize = std::env::var("ROUNDS").ok().and_then(|r| r.parse().ok()).unwrap_or(7);
    let only = std::env::var("ONLY").unwrap_or_default();
    println!("detected level: {:?}", Simd::detect().level());
    let (simd, portable) = (database(true), database(false));
    let (a, b) = (simd.session("B"), portable.session("B"));
    a.run("SET EXECUTOR.DEGREE = 1").unwrap();
    b.run("SET EXECUTOR.DEGREE = 1").unwrap();
    println!("{:<52} {:>10} {:>10} {:>7}", "query", "portable", "simd", "speed");
    for (name, sql) in [
        ("COUNT by an integer, 1%", "SELECT COUNT(*) FROM DC WHERE F < 20000"),
        ("COUNT by an integer, 50%", "SELECT COUNT(*) FROM DC WHERE F < 1000000"),
        ("COUNT by an integer with nulls, 50%", "SELECT COUNT(*) FROM DC WHERE N < 500"),
        ("COUNT by a double, 10%", "SELECT COUNT(*) FROM DC WHERE P < 500"),
        ("COUNT by a dictionary of texts", "SELECT COUNT(*) FROM DC WHERE T = 'name 7'"),
        ("SUM of a frame of 21 bits", "SELECT SUM(F) FROM DC"),
        ("SUM of an integer with nulls", "SELECT SUM(N) FROM DC"),
        ("SUM of prices as decimals (ALP)", "SELECT SUM(P) FROM DC"),
        ("SUM of a dictionary of 1,024 integers", "SELECT SUM(D) FROM DC"),
        ("SUM of an exact decimal", "SELECT SUM(Q) FROM DC"),
        ("MIN and MAX of an integer", "SELECT MIN(F), MAX(F) FROM DC"),
        ("MIN and MAX with nulls", "SELECT MIN(N), MAX(N) FROM DC"),
        ("SUM by an integer, 50%", "SELECT SUM(D) FROM DC WHERE F < 1000000"),
        ("ORDER BY an integer, top 10", "SELECT K FROM DC ORDER BY F DESC FETCH FIRST 10 ROWS ONLY"),
        ("ORDER BY an integer, 10% fetched", "SELECT F FROM DC WHERE F < 200000 ORDER BY F"),
        ("GROUP BY 300 texts", "SELECT T, COUNT(*) FROM DC GROUP BY T"),
        ("GROUP BY 1,024 integers, SUM", "SELECT D, SUM(F) FROM DC GROUP BY D"),
    ] {
        if !name.contains(only.as_str()) {
            continue;
        }
        assert_eq!(fetched(&a, sql), fetched(&b, sql), "{name}");
        let (mut slow, mut fast) = (Vec::new(), Vec::new());
        for round in 0..rounds {
            if round % 2 == 0 {
                slow.push(timed(&b, sql));
                fast.push(timed(&a, sql));
            } else {
                fast.push(timed(&a, sql));
                slow.push(timed(&b, sql));
            }
        }
        let (p, s) = (median(slow), median(fast));
        println!("{name:<52} {p:>10.3} {s:>10.3} {:>6.2}x", p / s);
    }
}
