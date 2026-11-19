//! [DESIGN] RULES.md D46: statement time, shown only in a session typed
//! at a terminal whose output is a screen, with a supplied clock so no
//! test depends on elapsed real time (R-TEST-5).

use std::cell::Cell;

use turborel::db::Database;
use turborel_test_support::Scratch;
use turborel_test_support::server::start;

use super::*;
use crate::args::{Command, parse};

/// A screen tall enough that no output is paged.
const SCREEN: Screen = Screen { width: 80, height: 10_000 };

/// Options parsed as the program's are from `args`, as the user X.
fn options(args: &[&str]) -> Options {
    let line: Vec<String> = ["-U", "X"].iter().chain(args).map(|a| (*a).to_owned()).collect();
    let Command::Run(options) = parse(&line, &|_| None).unwrap() else { panic!("a run") };
    *options
}

/// Where a run reads and writes, of `shown`.
struct Seat<'a> {
    /// Standard input.
    standard: &'a str,
    /// Whether standard input is typed at a terminal.
    typed: bool,
    /// The screen standard output is, if it is one.
    screen: Option<Screen>,
}

/// A typed session on a screen, reading `standard`.
fn interactive(standard: &str) -> Seat<'_> {
    Seat { standard, typed: true, screen: Some(SCREEN) }
}

/// A run's success, results and diagnostics, measured with `times`.
fn shown(given: &Options, seat: &Seat<'_>, times: &[Duration]) -> (bool, String, String) {
    let server = start(&Database::new(), &[]);
    let mut connection = server.connect("X");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let terminal = Terminal { screen: seat.screen, ..Terminal::default() };
    let io = Io { input: &mut seat.standard.as_bytes(), typed: seat.typed, out: &mut out, err: &mut err, interrupts: None, terminal };
    let mut ticks = times.iter().copied();
    let succeeded =
        run_with_clock(&mut connection, given, io, || ticks.next().expect("one start and end per statement")) == Outcome::Succeeded;
    assert_eq!(ticks.next(), None, "every attempted statement was timed");
    (succeeded, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

/// Verifies: [DESIGN] RULES.md D46: each statement typed has its own time
/// on stderr, in whole nanoseconds, while stdout keeps its results. A
/// zero duration and a duration whose nanoseconds exceed u64 are shown
/// whole. Time between statements is excluded.
#[test]
fn each_statement_reports_its_own_nanoseconds() {
    let times = [
        Duration::ZERO,
        Duration::ZERO,
        Duration::from_secs(1),
        Duration::from_nanos(1_000_000_123),
        Duration::from_secs(2),
        Duration::MAX,
    ];
    let (ok, out, err) = shown(&options(&[]), &interactive("COMMIT;\nVALUES 1;\nVALUES 2;\n"), &times);
    assert!(ok, "{err}");
    assert!(out.contains("OK\n"), "{out}");
    assert_eq!(out.matches("(1 row)").count(), 2);
    let longest = Duration::MAX.saturating_sub(Duration::from_secs(2)).as_nanos();
    assert_eq!(err, format!("Time: 0 ns\nTime: 123 ns\nTime: {longest} ns\n"));
}

/// Verifies: [DESIGN] RULES.md D46: no statement is timed, and the clock
/// is never read, unless it is typed at a terminal and output is a
/// screen. Scripts, `-c`, `-f`, piped input and output not on a screen
/// show no time, and `--timing` is an unknown option.
#[test]
fn only_an_interactive_session_is_timed() {
    let sql = "COMMIT;\nVALUES 1;\n";
    let scratch = Scratch::new("cli-timing");
    let file = scratch.path("statements.sql");
    std::fs::write(&file, sql).unwrap();
    let path = file.display().to_string();
    let file_given = options(&["-f", &path]);
    let command_given = options(&["-c", "COMMIT; VALUES 1"]);
    let seats = [
        (&command_given, Seat { standard: "", typed: true, screen: Some(SCREEN) }),
        (&file_given, Seat { standard: "", typed: true, screen: Some(SCREEN) }),
        (&options(&[]), Seat { standard: sql, typed: false, screen: Some(SCREEN) }),
        (&options(&[]), Seat { standard: sql, typed: true, screen: None }),
        (&options(&[]), Seat { standard: &format!("\\i {path}\n"), typed: true, screen: Some(SCREEN) }),
    ];
    for (given, seat) in &seats {
        let (ok, out, err) = shown(given, seat, &[]);
        assert!(ok, "{err}");
        assert!(out.contains("(1 row)"), "{out}");
        assert!(!err.contains("Time:"), "{err}");
    }
    assert!(parse(&["--timing".to_owned()], &|_| None).is_err(), "--timing is no option");
}

/// Verifies: [DESIGN] RULES.md D46: warnings, empty results and no-data
/// completions retain their output and each get a timing line.
#[test]
fn warnings_and_empty_results_are_timed() {
    let typed =
        "CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (N INTEGER);\nDELETE FROM T;\nSELECT N FROM T;\nVALUES CAST('abc' AS CHAR(1));\n";
    let (ok, out, err) = shown(&options(&[]), &interactive(typed), &[Duration::ZERO; 8]);
    assert!(ok, "{err}");
    assert!(out.contains("OK\n"), "{out}");
    assert!(out.contains("no data\n"), "{out}");
    assert!(out.contains("(0 rows)"), "{out}");
    assert!(out.contains("(1 row, with a warning: SQLSTATE 01004)"), "{out}");
    assert_eq!(err, "Time: 0 ns\n".repeat(4));
}

/// Verifies: [DESIGN] RULES.md D46: a failed SQL statement gets its time
/// after its diagnostic, and the session goes on. SQLCODE -301 is
/// retained. Blank lines and comments produce no timing line.
#[test]
fn failures_are_timed_and_the_session_goes_on() {
    let (ok, out, err) = shown(&options(&[]), &interactive("VALUES 1 / 0;\n\n-- a comment\nVALUES 2;\n"), &[Duration::ZERO; 4]);
    assert!(!ok);
    assert!(out.contains("(1 row)"));
    assert!(err.starts_with("turborel: SQLCODE -301 (SQLSTATE 22012):"), "{err}");
    assert!(err.ends_with("\nTime: 0 ns\nTime: 0 ns\n"), "{err}");
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
    let given = options(&[]);
    let now = Cell::new(Duration::ZERO);
    let mut out = Output { now: &now, bytes: Vec::new() };
    let mut err = Output { now: &now, bytes: Vec::new() };
    let terminal = Terminal { screen: Some(SCREEN), ..Terminal::default() };
    let io = Io { input: &mut "VALUES (1), (2);\n".as_bytes(), typed: true, out: &mut out, err: &mut err, interrupts: None, terminal };
    assert_eq!(run_with_clock(&mut connection, &given, io, || now.get()), Outcome::Succeeded);
    let shown = String::from_utf8(out.bytes).unwrap();
    let result = shown.strip_prefix(PROMPT).and_then(|s| s.strip_suffix(&format!("{PROMPT}\n"))).unwrap_or_else(|| panic!("{shown}"));
    assert!(result.ends_with("(2 rows)\n"), "{shown}");
    assert_eq!(String::from_utf8(err.bytes).unwrap(), format!("Time: {} ns\n", result.len()));
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
        let given = options(&[]);
        let mut out = Vec::new();
        let mut err = Broken { on_flush };
        let terminal = Terminal { screen: Some(SCREEN), ..Terminal::default() };
        let io = Io { input: &mut "COMMIT;\nCOMMIT;\n".as_bytes(), typed: true, out: &mut out, err: &mut err, interrupts: None, terminal };
        let calls = Cell::new(0_u32);
        let clock = || {
            calls.set(calls.get() + 1);
            Duration::ZERO
        };
        assert_eq!(run_with_clock(&mut connection, &given, io, clock), Outcome::Succeeded);
        assert_eq!(String::from_utf8(out).unwrap(), format!("{PROMPT}OK\n{PROMPT}OK\n{PROMPT}\n"));
        assert_eq!(calls.get(), 4, "a start and an end per statement");
    }
}
