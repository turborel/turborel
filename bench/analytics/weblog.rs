//! How fast the queries of a web analytics service run on its log of page
//! views (RULES.md R-BENCH-1). HITS is a column store table of 105 columns
//! (`weblog/schema.rs`). Its rows are made from a seed in the shape such a
//! log has (`weblog/rows.rs`), from figures measured of a reference log
//! and kept in `weblog/measured.rs` and `weblog/columns.rs`. They are
//! loaded as a bulk copy loads them (`weblog/load.rs`) and read by 43
//! queries (`weblog/queries.rs`). Each query runs once untimed and three
//! times timed. The harness reports its rate, and a table at the end
//! ranks the queries by the seconds of their fastest run, with the rows
//! each gave and a checksum of them, which two builds' runs are compared
//! by. It is a suite of the analytics benchmark, run alone as `cargo xtask
//! bench weblog`, whose options reach it as the variables below. It
//! stands in for runs on a reference log too large for a developer's
//! machine. Its use is to rank the queries, to profile one, to read a
//! plan and to tell whether a change moved one. It does not state how
//! fast the database is.

#[path = "weblog/columns.rs"]
mod columns;
#[path = "weblog/keys.rs"]
mod keys;
#[path = "weblog/layout.rs"]
mod layout;
#[path = "weblog/load.rs"]
mod load;
#[path = "weblog/measured.rs"]
mod measured;
#[path = "weblog/plain.rs"]
mod plain;
#[path = "weblog/queries.rs"]
mod queries;
#[path = "weblog/random.rs"]
mod random;
#[path = "weblog/rows.rs"]
mod rows;
#[path = "weblog/schema.rs"]
mod schema;
#[path = "weblog/texts.rs"]
mod texts;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::harness::{Bench, Case, note};
use turborel::db::Session;
use turborel::hash::xxhash::xxh3_64_with_seed;
use turborel::params::Params;

/// The rows of HITS, `ROWS` if not set.
const ROWS_VAR: &str = "TURBOREL_BENCH_WEBLOG_ROWS";
/// The seed the rows are made from, `SEED` if not set.
const SEED_VAR: &str = "TURBOREL_BENCH_WEBLOG_SEED";
/// The queries run, by commas, as `q02,q29`; all if not set.
const ONLY_VAR: &str = "TURBOREL_BENCH_WEBLOG_ONLY";
/// The threads a query runs on at most, the system's if not set.
const THREADS_VAR: &str = "TURBOREL_BENCH_WEBLOG_THREADS";
/// A query run in a loop and not measured, for a profiler to attach to.
const PROFILE_VAR: &str = "TURBOREL_BENCH_WEBLOG_PROFILE";
/// Queries run once and not measured, the plan of each printed with what
/// each step did.
const PLAN_VAR: &str = "TURBOREL_BENCH_WEBLOG_PLAN";
/// The seconds the loop of `PROFILE_VAR` lasts, `SECS` if not set.
const SECS_VAR: &str = "TURBOREL_BENCH_WEBLOG_SECS";
/// A directory HITS is kept in from one run to the next, so only the
/// first loads it; the harness's own, removed as the run ends, if not set.
const DIR_VAR: &str = "TURBOREL_BENCH_WEBLOG_DIR";

/// Rows of HITS by default, with which the suite runs in a few minutes.
const ROWS: u64 = 5_000_000;
/// The seed by default.
const SEED: u64 = 1;
/// Timed runs of each query, the fastest reported.
const RUNS: usize = 3;
/// Seconds a profiled query's loop lasts by default.
const SECS: u64 = 30;
/// Where the system says how many of its threads are busy, where it does.
const LOAD: &str = "/proc/loadavg";
/// The hundredths the load may move by between a run's start and its end
/// before the run's times are called disturbed, half a busy thread.
const MOVED: u64 = 50;
/// Seconds the process waits before a profiled query's loop, its number
/// printed, so a profiler can attach.
const WAIT: u64 = 5;

/// The value of the variable `name`, if set and not empty.
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// The number the variable `name` holds, `default` if it holds none.
fn number(name: &str, default: u64) -> u64 {
    var(name).and_then(|value| value.parse().ok()).unwrap_or(default)
}

/// The system's load average over the last minute, in hundredths, where
/// it tells one.
fn load() -> Option<u64> {
    let text = std::fs::read_to_string(LOAD).ok()?;
    let (whole, part) = text.split_whitespace().next()?.split_once('.')?;
    Some(whole.parse::<u64>().ok()? * 100 + part.parse::<u64>().ok()?)
}

/// Prints the load average `at` this point of the run, `load`, written
/// with its hundredths.
fn loaded(at: &str, load: u64) {
    note(&format!("load average {}.{:02} at the {at}", load.div_euclid(100), load % 100));
}

/// What a query gave, its rows and a checksum of their values in order.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Gave {
    rows: u64,
    checksum: u64,
}

/// Reads every row of `sql` and gives how many there were, with XXH3 of
/// each row's values as text, seeded with the checksum of the rows before
/// it.
fn fetched(s: &Session, sql: &str) -> Gave {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut gave = Gave { rows: 0, checksum: 0 };
    let mut text = String::new();
    while let Some(row) = cursor.fetch().unwrap() {
        text.clear();
        for value in &row {
            text.push_str(&value.to_string());
            text.push('\t');
        }
        gave = Gave { rows: gave.rows + 1, checksum: xxh3_64_with_seed(text.as_bytes(), gave.checksum) };
    }
    drop(cursor);
    s.commit().unwrap();
    gave
}

/// A measured query, with its name, its fastest run's seconds and what it
/// gave.
struct Line {
    name: &'static str,
    secs: f64,
    gave: Gave,
    /// Whether every run gave the same.
    same: bool,
}

/// Runs the suite. HITS is made or opened, then the queries are measured
/// and ranked, or their plans printed, or one run in a loop.
pub fn workloads(bench: &Bench) {
    let system = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let threads = number(THREADS_VAR, system as u64) as usize;
    let maker = rows::Maker::new(number(ROWS_VAR, ROWS), number(SEED_VAR, SEED));
    let (dir, kept) = var(DIR_VAR).map_or_else(|| (bench.path("weblog"), false), |dir| (PathBuf::from(dir), true));
    let started = Instant::now();
    let db = load::database(&dir, kept, &maker, system);
    note(&format!("{} rows of HITS ready in {:.1} s", maker.rows(), started.elapsed().as_secs_f64()));
    let s = db.session("B");
    s.run(&format!("SET EXECUTOR.DEGREE = {threads}")).unwrap();
    let constants = maker.constants();
    let only = var(PLAN_VAR).or_else(|| var(PROFILE_VAR)).or_else(|| var(ONLY_VAR));
    let chosen = |name: &str| only.as_ref().is_none_or(|names| names.split(',').any(|n| n.trim() == name));
    let texts: Vec<(&'static str, String)> =
        (0..queries::count()).filter(|&n| chosen(queries::name(n))).map(|n| (queries::name(n), queries::text(n, &constants))).collect();
    let before = load();
    if let Some(load) = before {
        loaded("start", load);
    }
    if var(PLAN_VAR).is_some() {
        planned(&s, &texts);
    } else if var(PROFILE_VAR).is_some() {
        profiled(&s, &texts);
    } else {
        ranked(bench, &s, &texts, threads);
    }
    if let (Some(before), Some(after)) = (before, load()) {
        loaded("end", after);
        if before.abs_diff(after) > MOVED {
            note("WARNING: the load moved while the queries ran, so other work disturbed their times; run them again");
        }
    }
    drop(s);
    // Closed, so a table kept for the next run is whole in its files.
    db.close().unwrap();
}

/// Measures each query of `texts` and prints them ranked by the seconds of
/// their fastest run, the slowest first, with the rows each gave, a
/// checksum of them, and the sum of the seconds.
fn ranked(bench: &Bench, s: &Session, texts: &[(&'static str, String)], threads: usize) {
    let mut lines: Vec<Line> = texts.iter().map(|(name, sql)| measured_query(bench, s, name, sql, threads)).collect();
    lines.sort_by(|a, b| b.secs.total_cmp(&a.secs));
    note(&format!("{:<6} {:>10} {:>8} {:>18}", "query", "seconds", "rows", "checksum"));
    for line in &lines {
        let differs = if line.same { "" } else { "  its runs differ" };
        note(&format!("{:<6} {:>10.4} {:>8} {:>18x}{differs}", line.name, line.secs, line.gave.rows, line.gave.checksum));
    }
    note(&format!("{:<6} {:>10.4}", "all", lines.iter().map(|line| line.secs).sum::<f64>()));
}

/// Measures the query `name`, of the text `sql`, and keeps what it gave.
fn measured_query(bench: &Bench, s: &Session, name: &'static str, sql: &str, threads: usize) -> Line {
    let timed = bench.measure_timed(Case::new(name, "queries").threads(threads), RUNS, || fetched(s, sql));
    let gave = timed.gave[0];
    let same = timed.gave.iter().all(|other| *other == gave);
    let differs = if same { "" } else { ", its runs differ" };
    bench.keep_result(name, threads, &format!("{} rows, checksum {:x}{differs}", gave.rows, gave.checksum));
    Line { name, secs: timed.least(), gave, same }
}

/// Runs each query of `texts` once and prints its plan with what each
/// step did, as EXPLAIN ANALYZE gives it.
fn planned(s: &Session, texts: &[(&str, String)]) {
    for (name, sql) in texts {
        note(&format!("{name}: {sql}"));
        s.explain_analyze(sql, &Params::new()).unwrap().lines().for_each(note);
        s.commit().unwrap();
    }
}

/// Runs the queries of `texts` in a loop for the seconds asked, after a
/// wait in which a profiler attaches to the process, whose number it
/// prints.
fn profiled(s: &Session, texts: &[(&str, String)]) {
    let secs = number(SECS_VAR, SECS);
    let names: Vec<&str> = texts.iter().map(|(name, _)| *name).collect();
    note(&format!("process {}: {} in a loop for {secs} s, starting in {WAIT} s", std::process::id(), names.join(", ")));
    std::thread::sleep(Duration::from_secs(WAIT));
    let started = Instant::now();
    let mut runs = 0_u64;
    while started.elapsed() < Duration::from_secs(secs) {
        for (_, sql) in texts {
            fetched(s, sql);
        }
        runs += 1;
    }
    note(&format!("{runs} runs in {:.1} s", started.elapsed().as_secs_f64()));
}
