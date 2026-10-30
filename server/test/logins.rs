//! Logins: their checks run apart from the statements' workers, failures
//! slow their address, messages before a login are small, and one limit
//! of time bounds TLS and authentication together.

use std::net::TcpStream;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use turborel::db::Database;
use turborel_client::connection::{self, Connection, Password, Trust};
use turborel_client::tls;
use turborel_protocol::{Reply, Request};
use turborel_server::logins::FIRST_DELAY;
use turborel_server::native::LOGIN_MESSAGE_BYTES;
use turborel_server::password::{self as pbkdf2, Pbkdf2};
use turborel_server::settings::{BATCH_BYTES, LOGIN_DELAY_MS, LOGIN_TIMEOUT, MAX_LOGINS, PASSWORD_ITERATIONS, WORKERS};

use crate::common::{Pki, Running, pki, secured, set, start, start_with, state};

/// The workers the server shows busy, as its owner OWNER reads them in
/// INFORMATION_SCHEMA.SERVER through a session of its own, embedded.
fn busy(db: &Database) -> String {
    let owner = db.session("OWNER");
    let mut cursor = owner.open_cursor("SELECT WORKERS_BUSY FROM INFORMATION_SCHEMA.SERVER").unwrap();
    let row = cursor.fetch().unwrap().unwrap();
    drop(cursor);
    owner.rollback().unwrap();
    row[0].to_string()
}

/// Verifies: [DESIGN] RULES.md D42, D43: a login is checked on threads
/// apart from the statements' workers, SERVER.MAX_LOGINS at once: with the
/// one worker held by a statement whose client takes none of its rows, a
/// client still logs in. Regression test (R-TEST-7): logins were checked
/// on the workers, so clients that never logged in kept statements
/// waiting, and statements that ran long kept every client from logging
/// in.
#[test]
fn logins_never_wait_for_the_workers() {
    let db = Database::new();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER, S CHARACTER(1000))").unwrap();
    let filling = db.session("X");
    for k in 0..4000 {
        filling.run(&format!("INSERT INTO T VALUES ({k}, 'row')")).unwrap();
    }
    filling.commit().unwrap();
    let server = start(&db, &[(WORKERS, 1), (MAX_LOGINS, 1), (BATCH_BYTES, 1), (LOGIN_TIMEOUT, 5000)]);
    let mut waiting = server.raw();
    waiting.hello("X");
    // Four megabytes of rows, more than the sockets hold, not read yet.
    waiting.send(&Request::Query("SELECT K, S FROM T".into()));
    assert!(
        (0..1000).any(|_| {
            std::thread::sleep(Duration::from_millis(5));
            busy(&db) == "1"
        }),
        "the statement waits on the one worker"
    );
    let other = connection::connect(server.address, "Y", &mut Trust).unwrap();
    assert_eq!(other.authorization(), "Y", "logged in while the worker waits");
    while let Some(reply) = waiting.receive() {
        if matches!(reply, Reply::Complete(_)) {
            break;
        }
    }
    other.terminate().unwrap();
}

/// A server of the user ANN, with the password "ann's", over TLS of
/// `pki`; a login's wait after failed ones at most `delay` ms.
fn of_ann(pki: &Pki, delay: i128) -> Running {
    let db = Database::new();
    secured(&db, pki, "ON");
    set(&db, PASSWORD_ITERATIONS, 10_000);
    let made = Pbkdf2::new(NonZeroU32::new(10_000).unwrap());
    db.create_user("ANN", made.make("ann's").unwrap().as_bytes()).unwrap();
    let password = Arc::new(pbkdf2::Password::new(db.clone(), &made).unwrap());
    start_with(&db, &[(LOGIN_DELAY_MS, delay)], vec![password])
}

/// A login over TLS to `server` as ANN with `password`, and how long it
/// took.
fn timed(server: &Running, pki: &Pki, password: &str) -> (connection::Result<Connection<tls::TlsStream>>, Duration) {
    let config = tls::trusting(&pki.authority).unwrap();
    let began = Instant::now();
    let made = tls::connect(server.address, ("localhost", &config), "ann", &mut Password(password.to_owned()));
    (made, began.elapsed())
}

/// Verifies: [DESIGN] RULES.md D43: after a failed login, the next from
/// the same address waits at least 100 ms before it is checked, a right
/// password's too; accepted, it forgets the failure. The user is never
/// locked out. SERVER.LOGIN_DELAY_MS 0 slows none.
#[test]
fn a_failed_login_slows_its_address() {
    let pki = pki("logins-delay");
    let server = of_ann(&pki, 10_000);
    let (wrong, _) = timed(&server, &pki, "bob's");
    assert_eq!(state(&wrong.err().unwrap()), ("28000", -919));
    let (right, took) = timed(&server, &pki, "ann's");
    assert_eq!(right.unwrap().authorization(), "ANN", "never locked out");
    assert!(took >= FIRST_DELAY, "{took:?}");
    let undelayed = of_ann(&pki, 0);
    for _ in 0..3 {
        assert!(timed(&undelayed, &pki, "bob's").0.is_err());
    }
    assert_eq!(timed(&undelayed, &pki, "ann's").0.unwrap().authorization(), "ANN");
}

/// Verifies: [DESIGN] RULES.md D42: before a client is authenticated, a
/// message longer than `LOGIN_MESSAGE_BYTES` is refused (SQLSTATE 08006),
/// however long SERVER.MAX_MESSAGE_BYTES allows; once it is, a message as
/// long as that is taken. A body cut short as the connection closes is
/// refused, saying so. Regression test (R-TEST-7): a client not yet
/// authenticated could make the server reserve SERVER.MAX_MESSAGE_BYTES
/// at once, by a header alone.
#[test]
fn messages_before_a_login_are_small() {
    let db = Database::new();
    let server = start(&db, &[]);
    let long = usize::try_from(LOGIN_MESSAGE_BYTES).unwrap() + 1;
    let mut raw = server.raw();
    raw.send(&Request::Hello { version: 1, user: "X".into(), mechanism: "trust".into(), initial: vec![0; long], capabilities: vec![] });
    let refused = raw.refusal();
    assert_eq!(state(&refused), ("08006", -919));
    assert!(refused.message.contains(&format!("longer than the {LOGIN_MESSAGE_BYTES} taken")), "{}", refused.message);
    let mut raw = server.raw();
    raw.hello("X");
    raw.send(&Request::Query(format!("VALUES 1{}", " ".repeat(long))));
    assert!(matches!(raw.receive(), Some(Reply::Description(_))), "taken once authenticated");
    let mut cut = server.raw();
    let mut hello = Vec::new();
    Request::Hello { version: 1, user: "X".into(), mechanism: "trust".into(), initial: vec![0; 100], capabilities: vec![] }
        .encode(&mut hello);
    cut.send_bytes(&hello[..50]);
    cut.0.shutdown(std::net::Shutdown::Write).unwrap();
    let refused = cut.refusal();
    assert!(refused.message.contains("within a message"), "{}", refused.message);
}

/// Verifies: [DESIGN] RULES.md D42: SERVER.LOGIN_TIMEOUT bounds TLS's
/// handshake and authentication together, from when the connection is
/// taken: a client slow to begin TLS, then slow to say Hello, each within
/// the limit but not the two, is closed (SQLSTATE 08006). Regression test
/// (R-TEST-7): each was given the whole limit, so a login took twice it.
#[test]
fn one_limit_bounds_tls_and_authentication() {
    let pki = pki("logins-limit");
    let db = Database::new();
    secured(&db, &pki, "ON");
    let server = start(&db, &[(LOGIN_TIMEOUT, 1000)]);
    let mut tcp = TcpStream::connect(server.address).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    let config = tls::trusting(&pki.authority).unwrap();
    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut session = rustls::ClientConnection::new(config, name).unwrap();
    while session.is_handshaking() {
        session.complete_io(&mut tcp).unwrap();
    }
    let stream = rustls::StreamOwned::new(session, tcp);
    std::thread::sleep(Duration::from_millis(600));
    // Closed by then: told why, or gone as Hello is sent.
    let refused = Connection::open(stream, "X", &mut Trust).err().unwrap();
    assert_eq!(state(&refused).0, "08006", "{refused}");
}
