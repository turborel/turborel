//! [DESIGN] RULES.md D46: optional statement timing, with a supplied
//! clock so no test depends on elapsed real time (R-TEST-5).

use std::cell::Cell;

use turborel::db::Database;
use turborel_test_support::Scratch;
use turborel_test_support::server::start;

use super::*;
use crate::args::{Command, parse};

/// Options to time `sql`, parsed as the program's are.
fn options(sql: &str) -> Options {
    let args = ["-U", "X", "--timing", "-c", sql].map(str::to_owned);
    let Command::Run(options) = parse(&args, &|_| None).unwrap() else { panic!("a run") };
    *options
}

/// A run's success, results and diagnostics, measured with `times`.
fn shown(given: &Options, (standard, typed): (&str, bool), times: &[Duration]) -> (bool, String, String) {
    let server = start(&Database::new(), &[]);
    let mut connection = server.connect("X");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let io = Io { input: &mut standard.as_bytes(), typed, out: &mut out, err: &mut err, interrupts: None, terminal: Terminal::default() };
    let mut ticks = times.iter().copied();
    let succeeded = run_with_clock(&mut connection, given, io, || ticks.next().expect("one start and end per statement")) == Outcome::Succeeded;
    assert_eq!(ticks.next(), None, "every attempted statement was timed");
    (succeeded, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

/// Verifies: [DESIGN] RULES.md D46: each statement has its own time on
/// stderr, in whole nanoseconds, while stdout keeps its results. A zero
/// duration and a duration whose nanoseconds exceed u64 are shown whole.
/// Time between statements is excluded.
#[test]
fn each_statement_reports_its_own_nanoseconds() {
    let given = options("COMMIT; VALUES 1; VALUES 2");
    let times = [
        Duration::ZERO,
        Duration::ZERO,
        Duration::from_secs(1),
        Duration::from_nanos(1_000_000_123),
        Duration::from_secs(2),
        Duration::MAX,
    ];
    let (ok, out, err) = shown(&given, ("", false), &times);
    assert!(ok, "{err}");
    assert!(out.starts_with("OK\n"));
    assert_eq!(out.matches("(1 row)").count(), 2);
    let longest = Duration::MAX.saturating_sub(Duration::from_secs(2)).as_nanos();
    assert_eq!(err, format!("Time: 0 ns\nTime: 123 ns\nTime: {longest} ns\n"));
    let mut untimed = given.clone();
    untimed.timing = false;
    let (without_ok, original, without_err) = shown(&untimed, ("", false), &[]);
    assert!(without_ok);
    assert_eq!(out, original, "timing leaves stdout unchanged");
    assert_eq!(without_err, "");
}

/// Verifies: [DESIGN] RULES.md D46: warnings, empty results and no-data
/// completions retain their output and each get a timing line.
#[test]
fn warnings_and_empty_results_are_timed() {
    let given =
        options("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (N INTEGER); DELETE FROM T; SELECT N FROM T; VALUES CAST('abc' AS CHAR(1))");
    let (ok, out, err) = shown(&given, ("", false), &[Duration::ZERO; 8]);
    assert!(ok, "{err}");
    assert!(out.starts_with("OK\nno data\n"), "{out}");
    assert!(out.contains("(0 rows)"), "{out}");
    assert!(out.contains("(1 row, with a warning: SQLSTATE 01004)"), "{out}");
    assert_eq!(err, "Time: 0 ns\n".repeat(4));
}

/// Verifies: [DESIGN] RULES.md D46: a failed SQL statement gets its time
/// after its diagnostic, and timing preserves continuation and
/// --stop-on-error. SQLCODE -301 is retained.
#[test]
fn failures_are_timed_before_continuing_or_stopping() {
    let mut given = options("VALUES 1 / 0; VALUES 2");
    let (ok, out, err) = shown(&given, ("", false), &[Duration::ZERO; 4]);
    assert!(!ok);
    assert!(out.contains("(1 row)"));
    assert!(err.starts_with("turborel: SQLCODE -301 (SQLSTATE 22012):"), "{err}");
    assert!(err.ends_with("\nTime: 0 ns\nTime: 0 ns\n"), "{err}");
    given.stop_on_error = true;
    let (stopped_ok, stopped_out, stopped_err) = shown(&given, ("", false), &[Duration::ZERO; 2]);
    assert!(!stopped_ok);
    assert!(!stopped_out.contains("(1 row)"));
    assert_eq!(stopped_err.matches("Time: 0 ns").count(), 1);
    given.timing = false;
    let (without_ok, _, without_err) = shown(&given, ("", false), &[]);
    assert!(!without_ok);
    assert!(!without_err.contains("Time:"));
}

/// Verifies: [DESIGN] RULES.md D46: files, piped input and typed input
/// time each SQL statement, including an unterminated last one. Blank
/// input and comments produce no timing line.
#[test]
fn every_sql_input_is_timed_without_timing_empty_input() {
    let sql = "-- a comment;\nCOMMIT;\nVALUES 1";
    let scratch = Scratch::new("cli-timing");
    let file = scratch.path("statements.sql");
    std::fs::write(&file, sql).unwrap();
    let mut given = options("");
    given.inputs = vec![Input::File(file)];
    let (ok, _, err) = shown(&given, ("", false), &[Duration::ZERO; 4]);
    assert!(ok);
    assert_eq!(err, "Time: 0 ns\nTime: 0 ns\n");
    given.inputs = vec![Input::Standard];
    for typed in [false, true] {
        let (standard_ok, _, standard_err) = shown(&given, (sql, typed), &[Duration::ZERO; 4]);
        assert!(standard_ok);
        assert_eq!(standard_err, "Time: 0 ns\nTime: 0 ns\n");
        assert_eq!(shown(&given, ("\n-- only a comment\n", typed), &[]).2, "");
    }
}

/// Output that advances a supplied clock by a nanosecond per byte.
struct Output<'a> {
    now: &'a Cell<Duration>,
    bytes: Vec<u8>,
}

impl Write for Output<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        let nanos = u64::try_from(bytes.len()).map_err(io::Error::other)?;
        self.now.set(self.now.get().saturating_add(Duration::from_nanos(nanos)));
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Verifies: [DESIGN] RULES.md D46: timing includes writing streamed rows
/// and the final result count; it ends before writing the timing itself.
#[test]
fn streamed_output_is_inside_the_measurement() {
    let server = start(&Database::new(), &[]);
    let mut connection = server.connect("X");
    let given = options("VALUES (1), (2)");
    let now = Cell::new(Duration::ZERO);
    let mut out = Output { now: &now, bytes: Vec::new() };
    let mut err = Output { now: &now, bytes: Vec::new() };
    let io = Io { input: &mut "".as_bytes(), typed: false, out: &mut out, err: &mut err, interrupts: None, terminal: Terminal::default() };
    assert_eq!(run_with_clock(&mut connection, &given, io, || now.get()), Outcome::Succeeded);
    let shown = String::from_utf8(out.bytes).unwrap();
    assert!(shown.ends_with("(2 rows)\n"), "{shown}");
    assert_eq!(String::from_utf8(err.bytes).unwrap(), format!("Time: {} ns\n", shown.len()));
}

/// A timing destination that refuses writes, or only its flush.
struct Broken {
    on_flush: bool,
}

impl Write for Broken {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.on_flush { Ok(bytes.len()) } else { Err(io::Error::other("closed")) }
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("closed"))
    }
}

/// Verifies: [DESIGN] RULES.md D46: a failure to write or flush timing
/// is ignored, as one to write a diagnostic is; the run goes on and
/// succeeds, each statement still timed.
#[test]
fn unwritten_timing_is_ignored() {
    for on_flush in [false, true] {
        let server = start(&Database::new(), &[]);
        let mut connection = server.connect("X");
        let given = options("COMMIT; COMMIT");
        let mut out = Vec::new();
        let mut err = Broken { on_flush };
        let io = Io { input: &mut "".as_bytes(), typed: false, out: &mut out, err: &mut err, interrupts: None, terminal: Terminal::default() };
        let calls = Cell::new(0_u32);
        let clock = || {
            calls.set(calls.get() + 1);
            Duration::ZERO
        };
        assert_eq!(run_with_clock(&mut connection, &given, io, clock), Outcome::Succeeded);
        assert_eq!(out, b"OK\nOK\n");
        assert_eq!(calls.get(), 4, "a start and an end per statement");
    }
}
