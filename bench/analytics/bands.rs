//! How fast joins on comparisons by order run (RULES.md D580, D581): a
//! band join, narrow and wide, of two tables of integers each row's own,
//! and the overlap of two tables' application-time periods, of a few days
//! each, scattered over centuries. The group `bands` joins tables of
//! `SMALL` rows, which a nested loop also joins in a few seconds; the
//! group `bands-large` joins tables of `LARGE` rows, which only a range
//! join does (RULES.md R-BENCH-1).

use super::harness::{Bench, Case, scatter};
use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;

/// Rows of each table of the group `bands`.
const SMALL: u64 = 4_000;
/// Rows of each table of the group `bands-large`.
const LARGE: u64 = 1_000_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 1_000;

/// Measures the group `bands`.
pub fn small(bench: &Bench) {
    workloads(bench, SMALL);
}

/// Measures the group `bands-large`.
pub fn large(bench: &Bench) {
    workloads(bench, LARGE);
}

/// Measures the joins of tables of `rows` rows each.
fn workloads(bench: &Bench, rows: u64) {
    let db = database(rows);
    let s = db.session("B");
    let queries = [
        ("band join, 3 rows wide", "SELECT COUNT(*) FROM BA A, BB B WHERE A.X BETWEEN B.Y - 1 AND B.Y + 1"),
        ("band join, 33 rows wide", "SELECT COUNT(*) FROM BA A, BB B WHERE A.X BETWEEN B.Y - 16 AND B.Y + 16"),
        ("join of A.X < B.Y < A.X + 8", "SELECT COUNT(*) FROM BA A, BB B WHERE B.Y > A.X AND B.Y < A.X + 8"),
        ("join of periods by OVERLAPS", "SELECT COUNT(*) FROM PA A, PB B WHERE A.P OVERLAPS B.P"),
        ("join of periods by CONTAINS", "SELECT COUNT(*) FROM PA A, PB B WHERE A.P CONTAINS B.P"),
    ];
    for (name, sql) in queries {
        bench.measure(Case::new(format!("{name}, {rows} x {rows} rows"), "rows"), || {
            fetched(&s, sql);
            rows
        });
    }
}

/// Schema B with BA and BB, of `rows` rows each, X and Y each row's own
/// value of `0..rows`, scattered; and PA and PB, of `rows` periods each,
/// of one to seven days, starting on days of `0..rows` from 1000-01-01,
/// scattered. Background work runs only when asked (JOBS.POLICY MANUAL),
/// so no freeze runs while a case is measured.
fn database(rows: u64) -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE BA (K INTEGER, X INTEGER)
           CREATE TABLE BB (K INTEGER, Y INTEGER)
           CREATE TABLE PA (K INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR P (S, E))
           CREATE TABLE PB (K INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR P (S, E))",
    )
    .unwrap();
    let s = db.session("B");
    for first in (0..rows).step_by(PER_INSERT as usize) {
        let keys = first..(first + PER_INSERT).min(rows);
        let of = |f: &dyn Fn(u64) -> String| keys.clone().map(f).collect::<Vec<_>>().join(", ");
        s.run(&format!("INSERT INTO BA VALUES {}", of(&|k| format!("({k}, {})", scatter(k, rows))))).unwrap();
        s.run(&format!("INSERT INTO BB VALUES {}", of(&|k| format!("({k}, {})", scatter(rows - 1 - k, rows))))).unwrap();
        s.run(&format!("INSERT INTO PA VALUES {}", of(&|k| period(k, scatter(k, rows), 1 + k % 7)))).unwrap();
        s.run(&format!("INSERT INTO PB VALUES {}", of(&|k| period(k, scatter(rows - 1 - k, rows), 1 + k % 5)))).unwrap();
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// The row of key `k` of a period of `days` days from day `start`.
fn period(k: u64, start: u64, days: u64) -> String {
    let (s, e) = (civil(start), civil(start + days));
    format!("({k}, DATE '{s}', DATE '{e}')")
}

/// Day `day` from 1000-01-01 as a date's text, by the days from the
/// civil calendar's epoch (Hinnant, "chrono-Compatible Low-Level Date
/// Algorithms", civil_from_days).
fn civil(day: u64) -> String {
    let days = i64::try_from(day).unwrap() + days_to_1000();
    let era = days.div_euclid(146_097);
    let of_era = days.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let from_march = (5 * of_year + 2) / 153;
    let of_month = of_year - (153 * from_march + 2) / 5 + 1;
    let month = if from_march < 10 { from_march + 3 } else { from_march - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{of_month:02}")
}

/// The days from 0000-03-01, the algorithm's epoch, to 1000-01-01: of
/// the year 999 counted from March, the 306 days from March 1 to January 1.
fn days_to_1000() -> i64 {
    let (year, era) = (999_i64, 2_i64);
    let year_of_era = year - era * 400;
    let of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + 306;
    era * 146_097 + of_era
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
