//! The program, run as a process.

use std::path::Path;
use std::process::{Command, Output};

use turborel::db::Database;

use crate::common::{pki, secured, start};

/// `turborel` run with `args`.
fn program(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_turborel")).args(args).env_remove(turborel_cli::PASSWORD_VARIABLE).output().unwrap()
}

/// Verifies: [DESIGN] RULES.md D46: `turborel --help` prints its usage and
/// succeeds, `--version` the release; a wrong command line fails, saying
/// why; so does a server not reached, at each of its host's addresses.
#[test]
fn the_program_prints_its_usage_or_what_is_wrong() {
    let help = program(&["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout).unwrap().starts_with("usage: turborel"));
    let version = program(&["--version"]);
    assert!(version.status.success());
    assert_eq!(String::from_utf8(version.stdout).unwrap().trim_end(), turborel::product());
    let wrong = program(&["--verbose"]);
    assert!(!wrong.status.success());
    assert!(String::from_utf8(wrong.stderr).unwrap().starts_with("turborel: --verbose is not an option"));
    let unreached = program(&["-p", "1", "-U", "X", "-c", "SELECT 1"]);
    assert!(!unreached.status.success());
    assert!(String::from_utf8(unreached.stderr).unwrap().starts_with("turborel: "));
    // Of a host of more than one address, each address's failure is told.
    let addresses = std::net::ToSocketAddrs::to_socket_addrs(&("localhost", 1)).map_or(0, Iterator::count);
    let each = String::from_utf8(program(&["-h", "localhost", "-p", "1", "-U", "X", "-c", "SELECT 1"]).stderr).unwrap();
    assert_eq!(each.matches("cannot connect").count(), addresses, "{each}");
}

/// `turborel` run with `args`, the platform's certificate authorities those
/// of the file `roots`, as the environment's SSL_CERT_FILE names it: so
/// what the platform trusts is the test's, whatever the machine's.
fn trusting(roots: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_turborel"));
    command.args(args).env_remove(turborel_cli::PASSWORD_VARIABLE).env_remove("SSL_CERT_DIR").env("SSL_CERT_FILE", roots);
    command.output().unwrap()
}

/// Verifies: [DESIGN] RULES.md D26, D46: `--tls-system-roots` trusts the
/// platform's certificate authorities, alone or beside the file `--tls`
/// names: a server whose authority the platform trusts is reached without
/// the file; one whose authority it does not is reached only with it. A
/// platform's store that cannot be read, or holds no certificate, is a
/// failure that says so.
#[test]
fn the_platforms_authorities_are_trusted_when_asked() {
    let (pki, other) = (pki("cli-system-roots"), pki("cli-system-roots-other"));
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    secured(&db, &pki, "REQUIRED");
    let server = start(&db, &[]);
    let port = server.address.port().to_string();
    let line = ["-h", "localhost", "-p", &port, "-U", "X", "--tls-system-roots", "-c", "SELECT K FROM T"];
    let platform = trusting(&pki.authority, &line);
    assert!(platform.status.success(), "{}", String::from_utf8_lossy(&platform.stderr));
    let elsewhere = trusting(&other.authority, &line);
    assert!(!elsewhere.status.success());
    let told = String::from_utf8(elsewhere.stderr).unwrap();
    assert!(told.contains("the TLS handshake failed"), "{told}");
    let authority = pki.authority.display().to_string();
    let beside = trusting(&other.authority, &[&line[..], &["--tls", &authority][..]].concat());
    assert!(beside.status.success(), "{}", String::from_utf8_lossy(&beside.stderr));
    let empty = other.scratch.path("empty.pem");
    std::fs::write(&empty, "").unwrap();
    for (roots, why) in [(other.scratch.path("none.pem"), "cannot trust the platform's"), (empty, "it holds none")] {
        let refused = trusting(&roots, &line);
        assert!(!refused.status.success());
        let told = String::from_utf8(refused.stderr).unwrap();
        assert!(told.contains("cannot trust the platform's certificate authorities") && told.contains(why), "{told}");
    }
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

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §14.3 GR5a, §20.1 GR2a];
/// [DESIGN] RULES.md D46: a run whose statements succeed but whose commit, as the program
/// ends the session, fails, a deferred constraint not satisfied, fails,
/// saying why (SQLSTATE 40002); nothing is committed. Regression test
/// (R-TEST-7): the program exited successfully, the commit's failure lost.
#[test]
fn a_commit_that_fails_at_the_end_fails_the_run() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE U (K INTEGER UNIQUE DEFERRABLE INITIALLY DEFERRED)").unwrap();
    let server = start(&db, &[]);
    let port = server.address.port().to_string();
    let run = program(&["-p", &port, "-U", "X", "-c", "INSERT INTO U VALUES (1)", "-c", "INSERT INTO U VALUES (1)"]);
    assert!(!run.status.success());
    let told = String::from_utf8(run.stderr).unwrap();
    assert!(told.starts_with("turborel: ") && told.contains("40002"), "{told}");
    let session = db.session("X");
    assert!(session.open_cursor("SELECT K FROM U").unwrap().fetch().unwrap().is_none(), "nothing committed");
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
