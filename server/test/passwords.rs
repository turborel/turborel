//! Passwords: a user proves itself by its password, over a secure
//! connection only; the owner manages users over the wire, in SQL.

use std::num::NonZeroU32;
use std::sync::Arc;

use turborel::db::Database;
use turborel_client::connection::{self, Password, Trust};
use turborel_client::target::{Target, connect_to};
use turborel_client::tls;
use turborel_protocol::Request;
use turborel_server::password::{self as pbkdf2, Pbkdf2};
use turborel_server::settings::PASSWORD_ITERATIONS;

use crate::common::{Pki, Running, done, pki, rows, secured, set, start_with, state};

/// \[DESIGN\] The iterations of the tests' verifiers: the fewest the
/// setting takes, so the tests run quickly.
const FEW: i128 = 10_000;

/// A server of a database whose users are ANN, the owner, with the
/// password "ann's", and BOB, with "bob's"; over TLS, offered, of `pki`;
/// taking passwords beside trust.
fn served(pki: &Pki) -> (Database, Running) {
    let db = Database::new();
    secured(&db, pki, "ON");
    set(&db, PASSWORD_ITERATIONS, FEW);
    let made = Pbkdf2::new(NonZeroU32::new(10_000).unwrap());
    for (user, password) in [("ANN", "ann's"), ("BOB", "bob's")] {
        db.create_user(user, made.make(password).unwrap().as_bytes()).unwrap();
    }
    db.set_owner("ANN").unwrap();
    let password = Arc::new(pbkdf2::Password::new(db.clone(), &made).unwrap());
    #[cfg(unix)]
    db.set_setting(turborel_server::settings::SOCKET, &turborel::value::Value::Char(pki.scratch.path("s").display().to_string())).unwrap();
    let server = start_with(&db, &[], vec![password]);
    (db, server)
}

/// A connection over TLS to `server`, as `user` with `password`.
fn as_user(server: &Running, pki: &Pki, (user, password): (&str, &str)) -> connection::Result<connection::Connection<tls::TlsStream>> {
    let config = tls::trusting(&pki.authority).unwrap();
    tls::connect(server.address, ("localhost", &config), user, &mut Password(password.to_owned()))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4]; [DESIGN] RULES.md D26,
/// D43: a user proves itself by its password, its name an identifier, so
/// `ann` is ANN; over TLS, or on Unix over a socket. A wrong password, and
/// a user not there, are refused alike (SQLSTATE 28000); so is a password
/// sent in the clear, saying why; and a name not an identifier.
#[test]
fn a_user_proves_itself_by_its_password() {
    let pki = pki("passwords");
    let (_db, server) = served(&pki);
    assert_eq!(as_user(&server, &pki, ("ann", "ann's")).unwrap().authorization(), "ANN");
    let wrong = as_user(&server, &pki, ("ann", "bob's")).err().unwrap();
    let nobody = as_user(&server, &pki, ("carol", "ann's")).err().unwrap();
    assert_eq!((state(&wrong), state(&nobody)), (("28000", -919), ("28000", -919)));
    assert_eq!(wrong.message, nobody.message, "alike");
    let clear = connection::connect(server.address, "ann", &mut Password("ann's".into())).err().unwrap();
    assert_eq!(state(&clear).0, "28000");
    assert!(clear.message.contains("TLS"), "{clear}");
    let mut raw = server.raw();
    raw.send(&Request::Hello { version: 1, user: "ann bob".into(), mechanism: "password".into(), initial: vec![], capabilities: vec![] });
    assert_eq!(state(&raw.refusal()).0, "28000", "not an identifier");
    #[cfg(unix)]
    {
        let local = connection::connect_local(&pki.scratch.path("s"), "bob", &mut Password("bob's".into())).unwrap();
        assert_eq!(local.authorization(), "BOB");
    }
}

/// Verifies: [DESIGN] RULES.md D26, D42, D43: a client refuses to send a
/// password over plain TCP, by `connect` or `connect_to`, before it makes
/// a connection (SQLSTATE 28000, saying to use TLS or a Unix socket): a
/// listener is never reached. Regression test (R-TEST-7): the password
/// crossed the network in Hello, and only the server refused it.
#[test]
fn a_password_never_leaves_over_plain_tcp() {
    let pki = pki("passwords-clear");
    let (_db, server) = served(&pki);
    let told = connection::connect(server.address, "ann", &mut Password("ann's".into())).err().unwrap();
    assert_eq!(state(&told), ("28000", -919));
    assert!(told.message.contains("TLS or a Unix socket, never over plain TCP"), "the client's own refusal: {told}");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let direct = connection::connect(address, "ann", &mut Password("ann's".into())).err().unwrap();
    let targeted = connect_to(&Target::Tcp(address), "ann", &mut Password("ann's".into())).err().unwrap();
    assert_eq!((state(&direct), state(&targeted)), (("28000", -919), ("28000", -919)));
    let reached = listener.accept().err().unwrap();
    assert_eq!(reached.kind(), std::io::ErrorKind::WouldBlock, "no connection made");
    let config = tls::trusting(&pki.authority).unwrap();
    let secured = Target::Tls(server.address, "localhost".into(), config);
    assert_eq!(connect_to(&secured, "ann", &mut Password("ann's".into())).unwrap().authorization(), "ANN", "over TLS");
    #[cfg(unix)]
    {
        let local = Target::Local(pki.scratch.path("s"));
        assert_eq!(connect_to(&local, "bob", &mut Password("bob's".into())).unwrap().authorization(), "BOB", "over a socket");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4]; [DESIGN] RULES.md D43:
/// over the wire, the owner creates a user with a password, as the
/// server's PBKDF2 and SCRAM make its verifier, who then proves itself by it; gives
/// it another, the old one refused then; and drops it. Another user
/// manages none (SQLCODE -104), and sees itself alone in
/// INFORMATION_SCHEMA.USERS. A user statement waits for no transaction:
/// in one it is refused (SQLSTATE 25000).
#[test]
fn the_owner_manages_users_over_the_wire() {
    let pki = pki("passwords-sql");
    let (db, server) = served(&pki);
    let mut ann = as_user(&server, &pki, ("ann", "ann's")).unwrap();
    assert_eq!(done(&ann.query("CREATE USER carol PASSWORD 'carol''s'").unwrap()), 0);
    let kept = db.verifier("CAROL").unwrap().unwrap();
    assert!(pbkdf2::check(&kept, "carol's") && kept.starts_with(b"$scram-sha-256$i=10000$"), "{}", String::from_utf8_lossy(&kept));
    as_user(&server, &pki, ("carol", "carol's")).unwrap().terminate().unwrap();
    assert_eq!(done(&ann.query("ALTER USER carol PASSWORD 'new'").unwrap()), 0);
    assert_eq!(state(&as_user(&server, &pki, ("carol", "carol's")).err().unwrap()).0, "28000", "the old one");
    as_user(&server, &pki, ("carol", "new")).unwrap().terminate().unwrap();
    let mut bob = as_user(&server, &pki, ("bob", "bob's")).unwrap();
    assert_eq!(state(&bob.query("DROP USER carol").unwrap_err()), ("2A000", -104));
    let users = "SELECT USER_NAME FROM INFORMATION_SCHEMA.USERS ORDER BY USER_NAME";
    assert_eq!(rows(&bob.query(users).unwrap()).iter().map(|r| r.trim().to_owned()).collect::<Vec<_>>(), ["BOB"]);
    assert_eq!(rows(&ann.query(users).unwrap()).len(), 3);
    assert_eq!(state(&ann.query("DROP USER carol").unwrap_err()).0, "25000", "in the query's transaction");
    ann.query("COMMIT").unwrap();
    assert_eq!(done(&ann.query("DROP USER carol").unwrap()), 0);
    assert_eq!(state(&as_user(&server, &pki, ("carol", "new")).err().unwrap()).0, "28000", "dropped");
    assert_eq!(connection::connect(server.address, "carol", &mut Trust).unwrap().authorization(), "CAROL", "trust is another mechanism");
}
