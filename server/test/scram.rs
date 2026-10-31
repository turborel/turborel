//! SCRAM-SHA-256: a user proves its password without sending it, the
//! server proving it knows the verifier; over TLS, a Unix socket or a
//! named pipe, over plain TCP only if the server allows it; proofs wrong, tampered with or
//! replayed refused, and a server that does not prove itself refused by
//! the client.

use std::num::NonZeroU32;
use std::sync::Arc;

use turborel::db::Database;
use turborel::value::Value;
use turborel_client::connection;
use turborel_client::scram::Scram;
use turborel_client::tls;
use turborel_protocol::scram::{self, Client};
use turborel_protocol::{Reply, Request};
use turborel_server::auth::{Authenticator, Exchange, Peer, Step};
use turborel_server::password::{Pbkdf2, Verifier};
use turborel_server::scram::{MECHANISM, Scram as Scrammed};
use turborel_server::settings::SCRAM_WITHOUT_TLS;
use turborel_test_support::server::Raw;

use crate::common::{Kept, Pki, Running, pki, secured, start_with, state};

/// A server of `pki`, its TLS offered, whose user ANN's password is
/// "ann's", proved by SCRAM-SHA-256 beside trust; on Unix at a socket too.
fn served(pki: &Pki) -> (Database, Running) {
    let db = Database::new();
    secured(&db, pki, "ON");
    let made = Pbkdf2::new(NonZeroU32::new(10_000).unwrap());
    db.create_user("ANN", made.make("ann's").unwrap().as_bytes()).unwrap();
    #[cfg(unix)]
    db.set_setting(turborel_server::settings::SOCKET, &Value::Char(pki.scratch.path("s").display().to_string())).unwrap();
    let scram = Arc::new(Scrammed::new(db.clone(), &made).unwrap());
    let server = start_with(&db, &[], vec![scram]);
    (db, server)
}

/// A connection over TLS to `server` as `user`, proving `password` by
/// SCRAM-SHA-256.
fn over_tls(server: &Running, pki: &Pki, (user, password): (&str, &str)) -> connection::Result<String> {
    let config = tls::trusting(&pki.authority).unwrap();
    let c = tls::connect(server.address, ("localhost", &config), user, &mut Scram::new(password))?;
    Ok(c.authorization().to_owned())
}

/// Verifies: [DESIGN] RULES.md D43; RFC 5802: a user proves its password
/// by SCRAM-SHA-256 over TLS, and on Unix over a socket; a wrong password
/// and a user not there are refused alike (SQLSTATE 28000), each told to
/// the diagnostics as a failed login; over plain TCP the client begins
/// nothing (SQLSTATE 28000). The mechanism is named `scram-sha-256`, and
/// the client's never shows the password.
#[test]
fn a_user_proves_its_password_by_scram() {
    let pki = pki("scram");
    let kept = Kept::default();
    let (db, server) = served(&pki);
    db.set_diagnostics(Box::new(kept.clone()));
    assert_eq!(over_tls(&server, &pki, ("ann", "ann's")).unwrap(), "ANN");
    let wrong = over_tls(&server, &pki, ("ann", "bob's")).unwrap_err();
    let nobody = over_tls(&server, &pki, ("carol", "ann's")).unwrap_err();
    assert_eq!((state(&wrong), wrong.message.as_str()), (("28000", -919), "the user or the password is wrong"));
    assert_eq!(wrong.message, nobody.message, "alike");
    assert_eq!(kept.of("LoginFailed").len(), 2, "{:?}", kept.all());
    let clear = connection::connect(server.address, "ann", &mut Scram::new("ann's")).err().unwrap();
    assert!(clear.message.contains("never over plain TCP"), "{clear}");
    #[cfg(unix)]
    {
        let local = connection::connect_local(&pki.scratch.path("s"), "ann", &mut Scram::new("ann's")).unwrap();
        assert_eq!(local.authorization(), "ANN");
    }
    assert_eq!((MECHANISM, format!("{:?}", Scram::new("ann's"))), ("scram-sha-256", "Scram(..)".to_owned()));
}

/// Verifies: [DESIGN] RULES.md D43: over plain TCP the server refuses
/// SCRAM-SHA-256, saying why, while SERVER.SCRAM_WITHOUT_TLS is OFF, its
/// default; ON, it takes it. The client sends it so only if its program
/// says (`Scram::in_the_clear`).
#[test]
fn scram_over_plain_tcp_waits_for_the_setting() {
    let pki = pki("scram-clear");
    let (db, server) = served(&pki);
    let refused = connection::connect(server.address, "ann", &mut Scram::new("ann's").in_the_clear()).err().unwrap();
    assert_eq!(state(&refused).0, "28000");
    assert!(refused.message.contains("SERVER.SCRAM_WITHOUT_TLS"), "{refused}");
    db.set_setting(SCRAM_WITHOUT_TLS, &Value::Char("ON".into())).unwrap();
    let taken = connection::connect(server.address, "ann", &mut Scram::new("ann's").in_the_clear()).unwrap();
    assert_eq!(taken.authorization(), "ANN");
}

/// The server's first message to `raw`, of a Hello as `user` whose
/// client-first `client` makes of `nonce`.
fn begin(raw: &mut Raw, user: &str, (client, nonce): (&mut Client, &str)) -> Vec<u8> {
    let initial = client.first(nonce);
    raw.send(&Request::Hello { version: 1, user: user.into(), mechanism: MECHANISM.into(), initial, capabilities: vec![] });
    let Some(Reply::Challenge(server_first)) = raw.receive() else { panic!("no server-first") };
    server_first
}

/// Verifies: [DESIGN] RULES.md D43; RFC 5802 §5, §9: the exchange by hand:
/// server-first extends the client's nonce with the server's, and gives
/// the verifier's salt and iterations; server-final is the server's
/// signature, which the client checks, and an empty answer then is Ready.
/// A client-final replayed into another exchange is refused, its nonce
/// not the new one's, as is a proof tampered with (SQLSTATE 28000). A user
/// not there is given a salt, the same each time.
#[test]
fn proofs_replayed_or_tampered_with_are_refused() {
    let pki = pki("scram-raw");
    let (db, server) = served(&pki);
    db.set_setting(SCRAM_WITHOUT_TLS, &Value::Char("ON".into())).unwrap();
    let mut client = Client::new("", "ann's");
    let mut raw = server.raw();
    let server_first = begin(&mut raw, "ann", (&mut client, "abcdefgh"));
    let kept = Verifier::read(&db.verifier("ANN").unwrap().unwrap()).unwrap();
    let text = String::from_utf8(server_first.clone()).unwrap();
    assert!(text.starts_with("r=abcdefgh") && text.ends_with(&format!(",s={},i=10000", scram::encode(&kept.salt))), "{text}");
    let client_final = client.answer(&server_first).unwrap();
    raw.send(&Request::Answer(client_final.clone()));
    let Some(Reply::Challenge(server_final)) = raw.receive() else { panic!("no server-final") };
    client.verify(&server_final).unwrap();
    raw.send(&Request::Answer(vec![]));
    assert!(matches!(raw.receive(), Some(Reply::Ready { .. })));
    let mut again = Client::new("", "ann's");
    let mut replay = server.raw();
    begin(&mut replay, "ann", (&mut again, "abcdefgh"));
    replay.send(&Request::Answer(client_final.clone()));
    assert_eq!(state(&replay.refusal()).0, "28000", "replayed");
    let mut tampered = server.raw();
    let mut third = Client::new("", "ann's");
    let first = begin(&mut tampered, "ann", (&mut third, "xyz"));
    let mut proof = third.answer(&first).unwrap();
    let at = proof.len() - 2;
    proof[at] ^= 1;
    tampered.send(&Request::Answer(proof));
    assert_eq!(state(&tampered.refusal()).0, "28000", "tampered");
    let salt = |nonce: &str| {
        let mut raw = server.raw();
        String::from_utf8(begin(&mut raw, "nobody", (&mut Client::new("", "x"), nonce))).unwrap().split(',').nth(1).unwrap().to_owned()
    };
    assert_eq!(salt("a"), salt("b"), "the same salt");
}

/// An authenticator that takes any client as `scram-sha-256` at once,
/// proving nothing: an impostor. A test-local plug-in (RULES.md D6).
struct Impostor;

impl Authenticator for Impostor {
    fn mechanism(&self) -> &'static str {
        MECHANISM
    }
    fn begin(&self, user: &str, _: &Peer) -> Box<dyn Exchange> {
        Box::new(Accepting(user.to_owned()))
    }
}

struct Accepting(String);

impl Exchange for Accepting {
    fn step(&mut self, _: &[u8]) -> Step {
        Step::Accept(self.0.clone())
    }
}

/// Verifies: [DESIGN] RULES.md D43; RFC 5802 §3: a client does not take a
/// server that sends Ready without proving it knows the user's verifier
/// (SQLSTATE 08006), nor a signature before its proof, nor a server-first
/// before its own first message.
#[test]
fn a_client_refuses_a_server_that_does_not_prove_itself() {
    let pki = pki("scram-impostor");
    let db = Database::new();
    secured(&db, &pki, "ON");
    let server = start_with(&db, &[], vec![Arc::new(Impostor)]);
    let config = tls::trusting(&pki.authority).unwrap();
    let refused = tls::connect(server.address, ("localhost", &config), "ann", &mut Scram::new("ann's")).err().unwrap();
    assert_eq!(state(&refused).0, "08006");
    assert!(refused.message.contains("did not prove"), "{refused}");
    let mut scram = Scram::new("ann's");
    assert!(connection::Mechanism::answer(&mut scram, b"r=x,s=AA==,i=1").is_err(), "not begun");
    connection::Mechanism::initial(&mut scram);
    assert!(connection::Mechanism::answer(&mut scram, b"v=AA==").is_err(), "a signature out of turn");
}
