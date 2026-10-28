//! The program, run as a process.

use std::process::{Command, Output};

use turborel::db::Database;

use crate::common::start;

/// `turborel` run with `args`.
fn program(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_turborel")).args(args).env_remove(turborel_cli::PASSWORD_VARIABLE).output().unwrap()
}

/// Verifies: [DESIGN] RULES.md D46: `turborel --help` prints its usage and
/// succeeds; a wrong command line fails, saying why; so does a server not
/// reached.
#[test]
fn the_program_prints_its_usage_or_what_is_wrong() {
    let help = program(&["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout).unwrap().starts_with("usage: turborel"));
    let wrong = program(&["--verbose"]);
    assert!(!wrong.status.success());
    assert!(String::from_utf8(wrong.stderr).unwrap().starts_with("turborel: --verbose is not an option"));
    let unreached = program(&["-p", "1", "-U", "X", "-c", "SELECT 1"]);
    assert!(!unreached.status.success());
    assert!(String::from_utf8(unreached.stderr).unwrap().starts_with("turborel: "));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1]; [DESIGN] RULES.md D46: the program
/// runs its statements over a connection, shows their results, and exits
/// successfully only if every one succeeded.
#[test]
fn the_program_runs_statements() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let server = start(&db, &[]);
    let port = server.address.port().to_string();
    let run = program(&["-p", &port, "-U", "X", "-c", "INSERT INTO T VALUES (7)", "-c", "SELECT K FROM T"]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert!(String::from_utf8(run.stdout).unwrap().ends_with(" 7\n(1 row)\n"));
    let failed = program(&["-p", &port, "-U", "X", "-c", "SELECT K FROM NOWHERE"]);
    assert!(!failed.status.success());
    assert!(String::from_utf8(failed.stderr).unwrap().starts_with("turborel: SQLCODE -"));
}

/// Verifies: [DESIGN] RULES.md D44, D46: Ctrl-C (SIGINT) cancels the
/// statement running; the run, not typed, ends there, and fails.
#[cfg(unix)]
#[test]
fn ctrl_c_cancels_the_statement_running() {
    use std::io::{BufRead, BufReader};
    use std::time::{Duration, Instant};
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let server = start(&db, &[]);
    let port = server.address.port().to_string();
    let mut args = vec!["-p", &port, "-U", "X", "-c", "INSERT INTO T VALUES (1)"];
    // 1024 rows: their cube is too many to count before Ctrl-C.
    args.extend(std::iter::repeat_n(["-c", "INSERT INTO T SELECT K FROM T"], 10).flatten());
    args.extend(["-c", "SELECT COUNT(*) FROM T A, T B, T C", "-c", "SELECT 1 FROM T"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_turborel"))
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    for _ in 0..11 {
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert_eq!(line, "OK\n");
    }
    // Ctrl-C until the count is cancelled: one sent before it begins
    // cancels nothing.
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        assert!(Command::new("kill").args(["-INT", &child.id().to_string()]).status().unwrap().success());
        std::thread::sleep(Duration::from_millis(200));
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "not cancelled");
    };
    assert_eq!(status.code(), Some(1));
    let mut err = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err).unwrap();
    assert!(err.starts_with("turborel: SQLCODE -920"), "{err}");
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
    assert_eq!(rest, "", "the run ended there");
}
