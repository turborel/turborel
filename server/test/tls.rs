//! TLS: offered or required by the settings, with certificates made for
//! each test by a certificate authority of its own; what a client trusts;
//! and a peer known to be secure.

use std::sync::Arc;

use turborel::db::Database;
use turborel::error::Code;
use turborel::value::Value;
use turborel_client::connection::{self, Trust};
use turborel_client::tls;
use turborel_server::auth::{Authenticator, Exchange, Peer, Step};
use turborel_server::listening;
use turborel_server::settings::{self, TLS, TLS_CERTIFICATE, TLS_KEY};

use crate::common::{over_tls, pki, rows, secured, start, start_with, state};

/// Verifies: [DESIGN] RULES.md D26, D42: with SERVER.TLS ON, a client may
/// connect over TLS, checking the server's certificate against its
/// authority and the name it connects to, and run statements; one in the
/// clear is served too, on the same port.
#[test]
fn tls_offered_serves_clients_with_it_or_without() {
    let pki = pki("tls-offered");
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    secured(&db, &pki, "on");
    let server = start(&db, &[]);
    let mut c = over_tls(&server, &pki, "localhost").unwrap();
    c.query("INSERT INTO T VALUES (1)").unwrap();
    assert_eq!(rows(&c.query("SELECT K FROM T").unwrap()), ["1"]);
    c.terminate().unwrap();
    let mut plain = server.connect("X");
    assert_eq!(rows(&plain.query("SELECT K FROM T").unwrap()), ["1"], "committed as the TLS session ended");
}

/// Verifies: [DESIGN] RULES.md D42: with SERVER.TLS REQUIRED, a TCP client
/// in the clear is refused, and told why (SQLSTATE 08004); one over TLS is
/// served. A Unix socket, or on Windows a named pipe, which only this
/// machine reaches, is served in the clear.
#[test]
fn tls_required_refuses_clients_in_the_clear() {
    let pki = pki("tls-required");
    let db = Database::new();
    secured(&db, &pki, "REQUIRED");
    #[cfg(any(unix, windows))]
    let socket = pki.scratch.socket("s");
    #[cfg(any(unix, windows))]
    db.set_setting(settings::SOCKET, &Value::Char(socket.display().to_string())).unwrap();
    let server = start(&db, &[]);
    let refused = connection::connect(server.address, "X", &mut Trust).err().unwrap();
    assert_eq!(state(&refused), ("08004", -919));
    assert!(refused.message.contains("TLS"), "{refused}");
    over_tls(&server, &pki, "localhost").unwrap().terminate().unwrap();
    #[cfg(any(unix, windows))]
    {
        assert!(server.names.contains(&socket.display().to_string()), "{:?}", server.names);
        connection::connect_local(&socket, "X", &mut Trust).unwrap().terminate().unwrap();
    }
}

/// Verifies: [DESIGN] RULES.md D26, D42: a client does not connect to a
/// server whose certificate its authorities did not issue, nor one not of
/// the name it connects to: the handshake fails (SQLSTATE 08006); the
/// server goes on serving others. With no server there, it cannot
/// connect at all.
#[test]
fn a_client_trusts_only_its_authorities() {
    let pki = pki("tls-trust");
    let other = self::pki("tls-other");
    let db = Database::new();
    secured(&db, &pki, "ON");
    let server = start(&db, &[]);
    let config = tls::trusting(&other.authority).unwrap();
    let untrusted = tls::connect(server.address, ("localhost", &config), "X", &mut Trust).err().unwrap();
    assert_eq!(state(&untrusted).0, "08006");
    assert!(untrusted.message.contains("handshake"), "{untrusted}");
    let misnamed = over_tls(&server, &pki, "db.example").err().unwrap();
    assert_eq!(state(&misnamed).0, "08006", "{misnamed}");
    assert!(over_tls(&server, &pki, "no name").is_err(), "not a name");
    over_tls(&server, &pki, "localhost").unwrap().terminate().unwrap();
    let gone = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let nobody = tls::connect(gone, ("localhost", &config), "X", &mut Trust).err().unwrap();
    assert!(nobody.message.contains("cannot connect"), "{nobody}");
}

/// Verifies: [DESIGN] RULES.md D42: a client connecting over TLS opens one
/// connection to its server, the one it secures, and no other.
#[test]
fn a_client_connects_once_to_secure_it() {
    let pki = pki("tls-once");
    let config = tls::trusting(&pki.authority).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    // Each connection taken in turn, and dropped at once: no handshake is
    // answered, so the client's ends once its connections were taken.
    let taken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counting = Arc::clone(&taken);
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            counting.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            drop(connection);
        }
    });
    let refused = tls::connect(address, ("localhost", &config), "X", &mut Trust).err().unwrap();
    assert_eq!(state(&refused).0, "08006", "{refused}");
    assert_eq!(taken.load(std::sync::atomic::Ordering::SeqCst), 1, "one connection opened");
}

/// An authenticator, `sealed`, that takes a user only over a secure
/// connection: as one taking a password must. A test-local plug-in
/// (RULES.md D6).
struct Sealed;

struct Sealing(bool, String);

impl Authenticator for Sealed {
    fn mechanism(&self) -> &'static str {
        "sealed"
    }
    fn begin(&self, user: &str, peer: &Peer) -> Box<dyn Exchange> {
        Box::new(Sealing(peer.secure, user.to_owned()))
    }
}

impl Exchange for Sealing {
    fn step(&mut self, _: &[u8]) -> Step {
        if self.0 { Step::Accept(self.1.clone()) } else { Step::Refuse("only over TLS".into()) }
    }
}

/// The client's side of `sealed`.
struct Seal;

impl connection::Mechanism for Seal {
    fn name(&self) -> &'static str {
        "sealed"
    }
    fn initial(&mut self) -> Vec<u8> {
        Vec::new()
    }
    fn answer(&mut self, _: &[u8]) -> connection::Result<Vec<u8>> {
        Ok(Vec::new())
    }
}

/// Verifies: [DESIGN] RULES.md D42: an authenticator knows whether its
/// peer is secure: over TLS it is; in the clear on the same port it is
/// not, so a mechanism may refuse a secret sent in the clear (SQLSTATE
/// 28000).
#[test]
fn an_authenticator_knows_a_secure_peer() {
    let pki = pki("tls-peer");
    let db = Database::new();
    secured(&db, &pki, "ON");
    let server = start_with(&db, &[], vec![Arc::new(Sealed)]);
    let config = tls::trusting(&pki.authority).unwrap();
    let c = tls::connect(server.address, ("localhost", &config), "X", &mut Seal).unwrap();
    assert_eq!(c.authorization(), "X");
    let clear = connection::connect(server.address, "X", &mut Seal).err().unwrap();
    assert_eq!(state(&clear).0, "28000");
}

/// Verifies: [DESIGN] RULES.md D42, R-ERR-1: TLS is not taken, saying why
/// (SQLCODE -915), with its files not named, not there, holding no
/// certificate, or a key that does not fit the certificate; with SERVER.TLS
/// OFF there is none to take. A client is not made to trust a file not
/// there, one holding no certificate, or one holding a malformed one.
#[test]
fn tls_says_why_it_cannot_be_taken() {
    let pki = pki("tls-files");
    let other = self::pki("tls-files-other");
    let db = Database::new();
    settings::declare(&db).unwrap();
    assert!(listening::tls(&db).unwrap().is_none(), "OFF");
    db.set_setting(TLS, &Value::Char("ON".into())).unwrap();
    let refused = |db: &Database| listening::tls(db).err().map(|e| (e.code, e.msg)).unwrap();
    assert!(refused(&db).1.contains("name its files"));
    secured(&db, &pki, "ON");
    db.set_setting(TLS_KEY, &Value::Char(other.key.display().to_string())).unwrap();
    assert_eq!(refused(&db).0, Code::Io, "a key of another");
    db.set_setting(TLS_CERTIFICATE, &Value::Char(pki.scratch.path("none.pem").display().to_string())).unwrap();
    assert!(refused(&db).1.contains("cannot take TLS"));
    std::fs::write(pki.scratch.path("empty.pem"), b"").unwrap();
    db.set_setting(TLS_CERTIFICATE, &Value::Char(pki.scratch.path("empty.pem").display().to_string())).unwrap();
    assert!(refused(&db).1.contains("no certificate"));
    assert_eq!(state(&tls::trusting(&pki.scratch.path("none.pem")).err().unwrap()).0, "08006");
    assert!(tls::trusting(&pki.scratch.path("empty.pem")).err().unwrap().message.contains("no certificate"));
    std::fs::write(pki.scratch.path("bad.pem"), b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n").unwrap();
    assert!(tls::trusting(&pki.scratch.path("bad.pem")).err().unwrap().message.contains("cannot trust"));
}

/// Times a client over TLS ends its connection and connects again at once.
const RECONNECTS_OVER_TLS: u32 = 50;

/// Verifies: [DESIGN] RULES.md D42: over TLS, a connection's place under
/// SERVER.MAX_CONNECTIONS is given back before close_notify is sent: at
/// the limit, a client that ends its connection and connects again at
/// once is never refused.
///
/// Regression test (R-TEST-7): the place was given back only once the
/// connection's task ended, after it closed.
#[test]
fn a_client_over_tls_that_saw_its_connection_closed_is_not_counted() {
    let pki = pki("tls-reconnects");
    let db = Database::new();
    secured(&db, &pki, "REQUIRED");
    let server = start(&db, &[(settings::MAX_CONNECTIONS, 1)]);
    for _ in 0..RECONNECTS_OVER_TLS {
        over_tls(&server, &pki, "localhost").unwrap().terminate().unwrap();
    }
}
