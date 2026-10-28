//! Sessions: statements run over a connection to a server, their results
//! shown, their failures told.

use std::io::{self, Write};
use std::num::NonZeroU32;
use std::sync::Arc;

use turborel::db::Database;
use turborel_cli::{Credentials, Input, Io, MORE, PROMPT, Server, Tls};
use turborel_client::connection::Password;
use turborel_server::password::{self as pbkdf2, Pbkdf2};

use crate::common::{Running, Scratch, command, options, pki, ran, secured, start, start_with};

/// A server of a database where X has table T, holding 1 and 2.
fn served() -> (Database, Running) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER, S CHARACTER(5))").unwrap();
    let server = start(&db, &[]);
    (db, server)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1]; [DESIGN] RULES.md D46: each
/// statement given runs, in order: a query's rows shown as a table, its
/// columns as wide as their types or names, a number's aligned right with
/// its name; a null shown as asked; another statement's completion said:
/// OK, OK with a warning's SQLSTATE [ISO/IEC 9075:1992 §11.37 GR8], or no
/// data.
#[test]
fn statements_run_and_show_their_results() {
    let (_db, server) = served();
    let mut given = options(
        &server,
        vec![
            command("INSERT INTO T VALUES (1, 'a'); INSERT INTO T VALUES (2, NULL)"),
            command("DELETE FROM T WHERE K = 9;SELECT K, S FROM T ORDER BY K"),
            command("REVOKE SELECT ON T FROM NOBODY CASCADE"),
        ],
    );
    given.null = "-".into();
    let run = ran(&given, "", false);
    assert!(run.succeeded, "{}", run.err);
    assert_eq!(
        run.out,
        "OK\nOK\nno data\n            K | S\n--------------+-------\n            1 | a\n            2 | -\n(2 rows)\nOK, with a warning: SQLSTATE 01006\n"
    );
    assert_eq!(run.err, "");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1]; [DESIGN] RULES.md D46: a statement
/// that fails is told, and the run goes on, then fails; with
/// `--stop-on-error`, it stops there.
#[test]
fn a_failure_is_told_and_the_run_goes_on() {
    let (_db, server) = served();
    let mut given = options(&server, vec![command("SELECT K FROM NOWHERE; SELECT COUNT(*) FROM T")]);
    let run = ran(&given, "", false);
    assert!(!run.succeeded);
    assert!(run.err.starts_with("turborel: SQLCODE -"), "{}", run.err);
    assert!(run.out.contains("(1 row)"), "went on: {}", run.out);
    given.stop_on_error = true;
    let stopped = ran(&given, "", false);
    assert_eq!((stopped.succeeded, stopped.out.as_str()), (false, ""), "stopped");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.2]; [DESIGN] RULES.md D46: a file's
/// statements run, its last needing no semicolon, a comment's ignored; a
/// file that cannot be read ends the run.
#[test]
fn a_file_is_run() {
    let (_db, server) = served();
    let scratch = Scratch::new("cli-file");
    let file = scratch.path("a.sql");
    std::fs::write(
        &file,
        "-- two rows; then a count\nINSERT INTO T VALUES (1, 'a');\nINSERT INTO T\n  VALUES (2, 'b');\nSELECT COUNT(*) FROM T",
    )
    .unwrap();
    let run = ran(&options(&server, vec![Input::File(file)]), "", false);
    assert!(run.succeeded, "{}", run.err);
    assert!(run.out.starts_with("OK\nOK\n") && run.out.ends_with("2\n(1 row)\n"), "{}", run.out);
    let missing = ran(&options(&server, vec![Input::File(scratch.path("none.sql")), command("SELECT K FROM T")]), "", false);
    assert!(!missing.succeeded);
    assert!(missing.err.starts_with("turborel: cannot read "), "{}", missing.err);
    assert_eq!(missing.out, "", "ended the run");
}

/// Verifies: [DESIGN] RULES.md D46: typed statements are prompted for, a
/// statement's rest by a prompt of its own; a failure does not end what is
/// typed, nor does a statement cancelled. Standard input not typed is read
/// without prompts.
#[test]
fn typed_statements_are_prompted_for() {
    let (_db, server) = served();
    let mut given = options(&server, vec![Input::Standard]);
    given.stop_on_error = true;
    let run = ran(&given, "SELECT K FROM NOWHERE;\nINSERT INTO T\nVALUES (1, 'a');\n", true);
    assert!(!run.succeeded);
    assert_eq!(run.out, format!("{PROMPT}{PROMPT}{MORE}OK\n{PROMPT}\n"));
    let piped = ran(&given, "INSERT INTO T VALUES (2, 'b')", false);
    assert_eq!((piped.succeeded, piped.out.as_str()), (true, "OK\n"));
}

/// Output that cannot be written.
struct Closed;

impl Write for Closed {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("closed"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Verifies: [DESIGN] RULES.md D42, D46: once the connection is lost, the
/// run ends, saying why; results that cannot be written end it too.
#[test]
fn a_run_ends_when_it_cannot_go_on() {
    let (_db, server) = served();
    let given = options(&server, vec![command("SELECT K FROM T; SELECT K FROM T")]);
    let mut connection = turborel_cli::connect(&given, || Ok(String::new())).unwrap();
    let mut err = Vec::new();
    let io = Io { input: &mut io::empty(), typed: false, out: &mut Closed, err: &mut err };
    assert!(!turborel_cli::run(&mut connection, &given, io));
    assert_eq!(String::from_utf8(err).unwrap(), "turborel: cannot write: closed\n", "once");
    let mut typed = Vec::new();
    let io = Io { input: &mut "SELECT K FROM T;".as_bytes(), typed: true, out: &mut Closed, err: &mut typed };
    assert!(!turborel_cli::run(&mut connection, &options(&server, vec![Input::Standard]), io));
    assert_eq!(String::from_utf8(typed).unwrap(), "turborel: cannot write: closed\n", "not even prompted");
    server.stop();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let io = Io { input: &mut io::empty(), typed: false, out: &mut out, err: &mut err };
    assert!(!turborel_cli::run(&mut connection, &given, io));
    let told = String::from_utf8(err).unwrap();
    assert_eq!(told.lines().count(), 1, "ended: {told}");
    assert!(told.contains("SQLSTATE 08006"), "{told}");
}

/// A server of `pki`, its TLS offered, where ANN's password is "ann's",
/// taken beside trust; on Unix, at a socket too.
fn with_passwords(pki: &crate::common::Pki) -> (Database, Running) {
    let db = Database::new();
    secured(&db, pki, "ON");
    let made = Pbkdf2::new(NonZeroU32::new(10_000).unwrap());
    db.create_user("ANN", made.make("ann's").unwrap().as_bytes()).unwrap();
    #[cfg(unix)]
    db.set_setting(turborel_server::settings::SOCKET, &turborel::value::Value::Char(pki.scratch.path("s").display().to_string())).unwrap();
    let password = Arc::new(pbkdf2::Password::new(db.clone(), &made).unwrap());
    let server = start_with(&db, &[], vec![password]);
    (db, server)
}

/// Verifies: [DESIGN] RULES.md D26, D43, D46: a user proves who they are by
/// a password, typed or from the environment, over TLS, the server's
/// certificate checked against its name, or on Unix over a socket; a wrong
/// one is refused, saying why.
#[test]
fn a_password_is_sent_securely() {
    let pki = pki("cli-passwords");
    let (_db, server) = with_passwords(&pki);
    let mut given = options(&server, vec![command("SELECT 1 FROM T")]);
    given.user = "ann".into();
    given.credentials = Credentials::Ask;
    let tls = Tls { roots: pki.authority.clone(), name: Some("localhost".into()) };
    given.server = Server::Tcp { host: "127.0.0.1".into(), port: server.address.port(), tls: Some(tls) };
    let connection = turborel_cli::connect(&given, || Ok("ann's".into())).unwrap();
    assert_eq!(connection.authorization(), "ANN");
    connection.terminate().unwrap();
    let wrong = turborel_cli::connect(&given, || Ok("bob's".into())).err().unwrap();
    assert!(wrong.contains("SQLSTATE 28000"), "{wrong}");
    let untyped = turborel_cli::connect(&given, || Err(io::Error::other("no terminal"))).err().unwrap();
    assert_eq!(untyped, "cannot read the password: no terminal");
    given.credentials = Credentials::Given(Password("ann's".into()));
    #[cfg(unix)]
    {
        given.server = Server::Socket(pki.scratch.path("s"));
        assert_eq!(turborel_cli::connect(&given, || panic!("not typed")).unwrap().authorization(), "ANN");
    }
    let tls = Tls { roots: pki.authority.clone(), name: None };
    given.server = Server::Tcp { host: "localhost".into(), port: server.address.port(), tls: Some(tls) };
    assert_eq!(turborel_cli::connect(&given, || panic!("not typed")).unwrap().authorization(), "ANN", "named as the host");
}

/// Verifies: [DESIGN] RULES.md D46: a server that cannot be reached, a
/// host that cannot be found, and certificates that cannot be read are
/// each told.
#[test]
fn a_server_not_reached_is_told() {
    let (_db, server) = served();
    let mut given = options(&server, vec![]);
    // Port 1, where nothing listens: a stopped server's port may be taken
    // by another test's.
    given.server = Server::Tcp { host: "127.0.0.1".into(), port: 1, tls: None };
    let unreached = turborel_cli::connect(&given, || Ok(String::new())).err().unwrap();
    assert!(unreached.contains("cannot connect"), "{unreached}");
    given.server = Server::Tcp { host: "no-such-host.invalid".into(), port: 1, tls: None };
    assert!(turborel_cli::connect(&given, || Ok(String::new())).err().unwrap().starts_with("cannot find no-such-host.invalid"));
    let scratch = Scratch::new("cli-roots");
    let tls = Tls { roots: scratch.path("none.pem"), name: None };
    given.server = Server::Tcp { host: "127.0.0.1".into(), port: 1, tls: Some(tls) };
    assert!(turborel_cli::connect(&given, || Ok(String::new())).err().unwrap().contains("cannot read"));
}
