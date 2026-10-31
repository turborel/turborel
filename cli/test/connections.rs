//! Connections: CONNECT, SET CONNECTION and DISCONNECT [ISO/IEC 9075:1992
//! §15], run by the client itself, an SQL-agent (RULES.md D46), over
//! servers it reaches by their addresses.

use std::io;

use turborel::db::Database;
use turborel_cli::{Credentials, Dialer, Server, Tls};
use turborel_client::agent::Agent;

use crate::common::{Running, command, options, pki, ran, start};
use crate::sessions::with_passwords;

/// A server of a database where X's table `table` holds `count` rows, and
/// PUBLIC may read it.
fn served(table: &str, count: i32) -> (Database, Running) {
    let db = Database::new();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION X CREATE TABLE {table} (K INTEGER) GRANT SELECT ON {table} TO PUBLIC")).unwrap();
    let s = db.session("X");
    for k in 0..count {
        s.run(&format!("INSERT INTO {table} VALUES ({k})")).unwrap();
    }
    s.commit().unwrap();
    let server = start(&db, &[]);
    (db, server)
}

/// The address of `server`, as CONNECT names it.
fn address(server: &Running) -> String {
    format!("127.0.0.1:{}", server.address.port())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §15.1 GR8, GR9; §15.2 GR3; §15.3 GR9];
/// [DESIGN] RULES.md D46: CONNECT TO an address AS a name makes a
/// connection to that server, as the user named, current; SET CONNECTION
/// makes another current, DEFAULT the one the command line made; DISCONNECT
/// ends one, and with the current one ended, none is current until one is
/// made or chosen.
#[test]
fn connections_are_made_chosen_and_ended() {
    let (_one, first) = served("T", 1);
    let (_two, second) = served("U", 2);
    let to = address(&second);
    let given = options(
        &first,
        vec![command(&format!(
            "CONNECT TO '{to}' AS 'other' USER 'y'; SELECT COUNT(*), CURRENT_USER FROM X.U; COMMIT;
             SET CONNECTION DEFAULT; SELECT COUNT(*), CURRENT_USER FROM T; COMMIT;
             SET CONNECTION ' OTHER '; DISCONNECT CURRENT; SELECT COUNT(*) FROM X.U;
             SET CONNECTION DEFAULT; SELECT COUNT(*) FROM T; COMMIT; DISCONNECT ALL;
             CONNECT TO DEFAULT; SELECT COUNT(*) FROM T"
        ))],
    );
    let run = ran(&given, "", false);
    assert!(!run.succeeded);
    let told: Vec<&str> = run.err.lines().collect();
    assert_eq!(told.len(), 1, "{}", run.err);
    assert!(told[0].contains("SQLSTATE 08003"), "none current: {}", told[0]);
    let rows = |count: &str, user: &str| format!("{:>22}|\n{}+---\n{count:>21} | {user}\n(1 row)\n", "", "-".repeat(22));
    let count = format!("\n{}\n{:>21}\n(1 row)\n", "-".repeat(22), 1);
    let ok = |n: usize| "OK\n".repeat(n);
    let shown = [ok(1), rows("2", "Y"), ok(2), rows("1", "X"), ok(4), count.clone(), ok(3), count].concat();
    assert_eq!(run.out, shown, "the second server's rows as Y, then the first's as X");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §15.1 GR1, GR3, GR6, GR7, GR10; §15.2
/// GR1, GR2; §15.3 GR1, GR6]: each refusal by its SQLSTATE: a name in use
/// (08002), none such (08003), a name or user not an identifier (2E000,
/// 28000), a server not reached (08001); CONNECT or SET CONNECTION in a
/// transaction (0A001), DISCONNECT of a connection whose transaction is
/// active (25000). The connections held go on, as they do past a
/// statement's 28000, SET SESSION AUTHORIZATION refused [§16.4 GR5].
#[test]
fn connection_statements_are_refused_saying_why() {
    let (_db, server) = served("T", 1);
    let to = address(&server);
    let statements = [
        ("SET CONNECTION 'NOPE'", "08003"),
        ("DISCONNECT 'NOPE'", "08003"),
        ("CONNECT TO DEFAULT", "08002"),
        (&*format!("CONNECT TO '{to}'"), "2E000"),
        (&*format!("CONNECT TO '{to}' AS '1x'"), "2E000"),
        (&*format!("CONNECT TO '{to}' AS 'C' USER 'a b'"), "28000"),
        ("CONNECT TO '127.0.0.1:1' AS 'C'", "08001"),
        ("CONNECT TO 'no-such-host.invalid' AS 'C'", "08001"),
        ("CONNECT TO '[::1' AS 'C'", "08001"),
        ("SET SESSION AUTHORIZATION 'Z'", "28000"),
        ("SELECT COUNT(*) FROM T", ""),
        (&*format!("CONNECT TO '{to}' AS 'C'"), "0A001"),
        ("SET CONNECTION DEFAULT", "0A001"),
        ("DISCONNECT DEFAULT", "25000"),
        ("DISCONNECT ALL", "25000"),
        ("COMMIT", ""),
        (&*format!("CONNECT TO '{to}' AS 'C'"), ""),
        (&*format!("CONNECT TO '{to}' AS 'c'"), "08002"),
        ("DISCONNECT 'C'", ""),
        ("DISCONNECT DEFAULT", ""),
        ("DISCONNECT CURRENT", "08003"),
    ];
    let sql: Vec<&str> = statements.iter().map(|(s, _)| *s).collect();
    let run = ran(&options(&server, vec![command(&sql.join(";"))]), "", false);
    let told: Vec<&str> = run.err.lines().collect();
    let expected: Vec<&str> = statements.iter().map(|(_, state)| *state).filter(|s| !s.is_empty()).collect();
    assert_eq!(told.len(), expected.len(), "{}", run.err);
    for (line, state) in told.iter().zip(expected) {
        assert!(line.contains(&format!("SQLSTATE {state}")), "{state}: {line}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §15.1 GR8b, GR11]; [DESIGN]
/// RULES.md D43, D46: a connection CONNECT makes is secured as the
/// command line's is, over TLS trusting its certificates, the server's
/// checked against the host named; on Unix a path beginning '/' is a
/// socket's. A password typed is asked for again for each: a wrong one is
/// refused (08004), and one that cannot be read leaves the server
/// unreached (08001).
#[test]
fn a_connection_made_is_proved_and_secured_as_the_first() {
    let pki = pki("cli-connect");
    let (_db, server) = with_passwords(&pki);
    let mut given = options(&server, vec![]);
    given.user = "ann".into();
    given.credentials = Credentials::Ask;
    let tls = Tls { roots: pki.authority.clone(), name: Some("localhost".into()), identity: None };
    given.server = Server::Tcp { host: "127.0.0.1".into(), port: server.address.port(), tls: Some(tls) };
    let connection = turborel_cli::connect(&given, || Ok("ann's".into())).unwrap();
    let mut typed = vec![Err(io::Error::other("no terminal")), Ok("bob's".to_owned()), Ok("ann's".to_owned()), Ok("ann's".to_owned())];
    let ask = Box::new(move || typed.pop().unwrap());
    let mut agent = Agent::connected(Dialer::new(given, ask), connection);
    let port = server.address.port();
    agent.query(&format!("CONNECT TO 'localhost:{port}' AS 'TLS'")).unwrap();
    assert_eq!(agent.current().unwrap().authorization(), "ANN");
    #[cfg(unix)]
    {
        agent.query(&format!("CONNECT TO '{}' AS 'SOCK'", pki.scratch.path("s").display())).unwrap();
        assert_eq!(agent.current_name(), Some("SOCK"));
    }
    #[cfg(not(unix))]
    agent.query(&format!("CONNECT TO 'localhost:{port}' AS 'SOCK'")).unwrap();
    assert_eq!(agent.query(&format!("CONNECT TO 'localhost:{port}' AS 'WRONG'")).unwrap_err().sqlstate, "08004");
    assert_eq!(agent.query(&format!("CONNECT TO 'localhost:{port}' AS 'UNREAD'")).unwrap_err().sqlstate, "08001");
    agent.terminate().unwrap();
}
