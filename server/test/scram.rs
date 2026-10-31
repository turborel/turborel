//! SCRAM-SHA-256: a user proves its password without sending it, the
//! server proving it knows the verifier; over TLS, a Unix socket or a
//! named pipe, over plain TCP only if the server allows it; proofs wrong, tampered with or
//! replayed refused, and a server that does not prove itself refused by
//! the client. Over TLS, SCRAM-SHA-256-PLUS binds the exchange to the
//! connection, over TLS 1.3 by `tls-exporter`, over TLS 1.2 by the
//! certificate the server presented: relayed or downgraded, refused.

use std::net::TcpStream;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, StreamOwned};

use turborel::db::Database;
use turborel::value::Value;
use turborel_client::connection::{self, Connection, Mechanism, TlsChannel};
use turborel_client::scram::Scram;
use turborel_client::tls;
use turborel_protocol::scram::{self, Client};
use turborel_protocol::{Reply, Request};
use turborel_server::auth::{Authenticator, Exchange, Peer, Step};
use turborel_server::password::{Pbkdf2, Verifier};
use turborel_server::scram::{MECHANISM, MECHANISM_PLUS, Scram as Scrammed};
use turborel_server::settings::{SCRAM_WITHOUT_TLS, TLS_CERTIFICATE, TLS_KEY};
use turborel_test_support::server::Raw;

use crate::common::{Kept, Pki, Running, ed25519_certificate, pki, secured, start_with, state};

/// A server of `pki`, its TLS offered, whose user ANN's password is
/// "ann's", proved by SCRAM-SHA-256 or SCRAM-SHA-256-PLUS beside trust; at
/// a Unix socket or, on Windows, a named pipe too.
fn served(pki: &Pki) -> (Database, Running) {
    served_by(pki, false)
}

/// A server as `served`'s, by SCRAM-SHA-256-PLUS alone if `plus_only`.
fn served_by(pki: &Pki, plus_only: bool) -> (Database, Running) {
    let db = Database::new();
    secured(&db, pki, "ON");
    let made = Pbkdf2::new(NonZeroU32::new(10_000).unwrap());
    db.create_user("ANN", made.make("ann's").unwrap().as_bytes()).unwrap();
    #[cfg(any(unix, windows))]
    db.set_setting(turborel_server::settings::SOCKET, &Value::Char(pki.scratch.socket("s").display().to_string())).unwrap();
    let scram = Scrammed::new(db.clone(), &made).unwrap();
    let mut mechanisms: Vec<Arc<dyn Authenticator>> = vec![Arc::new(scram.plus())];
    if !plus_only {
        mechanisms.push(Arc::new(scram));
    }
    let server = start_with(&db, &[], mechanisms);
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
/// by SCRAM-SHA-256 over TLS, and over a Unix socket or, on Windows, a
/// named pipe; a wrong password and a user not there are refused alike
/// (SQLSTATE 28000), each told to the diagnostics as a failed login; over
/// plain TCP the client begins nothing (SQLSTATE 28000). The mechanism is
/// named `scram-sha-256`, and the client's never shows the password.
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
    #[cfg(any(unix, windows))]
    {
        let local = connection::connect_local(&pki.scratch.socket("s"), "ann", &mut Scram::new("ann's")).unwrap();
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

/// An authenticator that takes any client as `scram-sha-256-plus`, as
/// clients over TLS ask, at once, proving nothing: an impostor. A
/// test-local plug-in (RULES.md D6).
struct Impostor;

impl Authenticator for Impostor {
    fn mechanism(&self) -> &'static str {
        MECHANISM_PLUS
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
    assert!(scram.answer(b"r=x,s=AA==,i=1").is_err(), "not begun");
    scram.initial();
    assert!(scram.answer(b"v=AA==").is_err(), "a signature out of turn");
}

/// A TLS stream to `server`, trusting `pki`'s authority, its handshake
/// done: for a mechanism bound by hand (`Mechanism::bind`).
fn handshaken(server: &Running, pki: &Pki) -> tls::TlsStream {
    handshaken_by(server, tls::trusting(&pki.authority).unwrap())
}

/// A client's TLS of version 1.2 alone, trusting `pki`'s authority.
fn tls12(pki: &Pki) -> Arc<ClientConfig> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let versions = ClientConfig::builder_with_provider(provider).with_protocol_versions(&[&rustls::version::TLS12]).unwrap();
    Arc::new(versions.with_root_certificates(tls::roots(Some(&pki.authority), false).unwrap()).with_no_client_auth())
}

/// The binding of a TLS connection whose server presented `certificate`,
/// as DER, of no `tls-exporter`: as over TLS 1.2.
fn by_certificate(certificate: Vec<u8>) -> TlsChannel {
    TlsChannel { certificate: Some(certificate), exporter: None }
}

/// A TLS stream to `server`, by `config`, its handshake done.
fn handshaken_by(server: &Running, config: Arc<ClientConfig>) -> tls::TlsStream {
    let mut tcp = TcpStream::connect(server.address).unwrap();
    let mut session = ClientConnection::new(config, ServerName::try_from("localhost").unwrap()).unwrap();
    while session.is_handshaking() {
        session.complete_io(&mut tcp).unwrap();
    }
    StreamOwned::new(session, tcp)
}

/// The DER of the first certificate of the PEM file `path`.
fn der_of(path: &Path) -> Vec<u8> {
    CertificateDer::from_pem_file(path).unwrap().as_ref().to_vec()
}

/// Verifies: [DESIGN] RULES.md D43; RFC 5802 §6; RFC 5929 §4: over TLS a
/// client proves its password by SCRAM-SHA-256-PLUS, bound to the
/// certificate the server presented, which a server of that mechanism
/// alone takes. A client bound to another certificate, as one is through
/// a server in the middle that presents its own and relays the exchange,
/// is refused (SQLSTATE 28000). A client of SCRAM-SHA-256 binding nothing
/// (`n`) is still taken over TLS; one that could bind but sees none
/// offered (`y`), as of a certificate signed by Ed25519, which defines
/// none, is refused where the server offers one: a downgrade.
#[test]
fn scram_over_tls_is_bound_to_its_connection() {
    let (alone, both) = (pki("scram-plus"), pki("scram-bound"));
    let (_db, plus) = served_by(&alone, true);
    assert_eq!(over_tls(&plus, &alone, ("ann", "ann's")).unwrap(), "ANN", "by scram-sha-256-plus");
    let mut relayed = Scram::new("ann's");
    relayed.bind(&by_certificate(der_of(&both.chain)));
    assert_eq!(relayed.name(), MECHANISM_PLUS);
    let refused = Connection::open(handshaken(&plus, &alone), "ann", &mut relayed).err().unwrap();
    assert_eq!(state(&refused), ("28000", -919), "{refused}");
    let (_db, server) = served(&both);
    let unbound = Connection::open(handshaken(&server, &both), "ann", &mut Scram::new("ann's")).unwrap();
    assert_eq!(unbound.authorization(), "ANN", "a client that binds nothing");
    let mut unoffered = Scram::new("ann's");
    unoffered.bind(&by_certificate(ed25519_certificate()));
    assert_eq!(unoffered.name(), MECHANISM);
    let downgraded = Connection::open(handshaken(&server, &both), "ann", &mut unoffered).err().unwrap();
    let why = "SCRAM: the client believes the server offers no channel binding, which it does: a downgrade";
    assert_eq!((state(&downgraded), downgraded.message.as_str()), (("28000", -919), why));
    let mut once = Scram::new("ann's");
    once.bind(&by_certificate(der_of(&both.chain)));
    once.initial();
    assert_eq!(once.name(), MECHANISM, "a binding is of one connection");
}

/// Verifies: [DESIGN] RULES.md D42, D43; RFC 5929 §4: a certificate taken
/// again, its settings changed, binds the connections after: a client
/// bound to the certificate the server now presents proves its password;
/// one bound to the certificate it presented before is refused (SQLSTATE
/// 28000).
#[test]
fn a_certificate_taken_again_binds_the_connections_after() {
    let (old, new) = (pki("scram-reload-old"), pki("scram-reload-new"));
    let (db, server) = served(&old);
    assert_eq!(over_tls(&server, &old, ("ann", "ann's")).unwrap(), "ANN");
    db.set_setting(TLS_CERTIFICATE, &Value::Char(new.chain.display().to_string())).unwrap();
    db.set_setting(TLS_KEY, &Value::Char(new.key.display().to_string())).unwrap();
    assert_eq!(over_tls(&server, &new, ("ann", "ann's")).unwrap(), "ANN", "bound to the new one");
    let mut stale = Scram::new("ann's");
    stale.bind(&by_certificate(der_of(&old.chain)));
    let refused = Connection::open(handshaken(&server, &new), "ann", &mut stale).err().unwrap();
    assert_eq!(state(&refused).0, "28000", "{refused}");
}

/// Verifies: [DESIGN] RULES.md D43; RFC 9266 §2, §3: over TLS 1.3 the
/// client binds by `tls-exporter`, 32 bytes the connection exports, which
/// the server takes though the certificate the client names is another's:
/// the connection, not the certificate, is bound. A client of another
/// connection's keying material, as one is through a server in the
/// middle relaying between two TLS connections to the same server, is
/// refused (SQLSTATE 28000), though the certificate is the server's own.
#[test]
fn over_tls_1_3_scram_is_bound_by_the_exporter() {
    let (pki, other) = (pki("scram-exporter"), pki("scram-exporter-other"));
    let (_db, server) = served_by(&pki, true);
    let stream = handshaken(&server, &pki);
    let own = tls::channel(&stream);
    assert_eq!((stream.conn.protocol_version(), own.exporter.as_ref().map(Vec::len)), (Some(rustls::ProtocolVersion::TLSv1_3), Some(32)));
    assert_eq!(own.certificate, Some(der_of(&pki.chain)));
    let mut bound = Scram::new("ann's");
    bound.bind(&TlsChannel { certificate: Some(der_of(&other.chain)), ..own.clone() });
    assert_eq!(bound.name(), MECHANISM_PLUS);
    assert_eq!(Connection::open(stream, "ann", &mut bound).unwrap().authorization(), "ANN", "by tls-exporter");
    let (relay, onward) = (handshaken(&server, &pki), handshaken(&server, &pki));
    let mut relayed = Scram::new("ann's");
    relayed.bind(&tls::channel(&relay));
    let refused = Connection::open(onward, "ann", &mut relayed).err().unwrap();
    assert_eq!(state(&refused), ("28000", -919), "{refused}");
    assert_eq!(over_tls(&server, &pki, ("ann", "ann's")).unwrap(), "ANN", "tls::connect binds by tls-exporter");
}

/// Verifies: [DESIGN] RULES.md D43; RFC 9266 §2; RFC 5929 §4; RFC 5802
/// §6: over TLS 1.2 the connection has no `tls-exporter`, so the client
/// binds by `tls-server-end-point`, through `tls::connect` too; one bound
/// to another certificate is refused, as is one naming `tls-exporter`
/// there, a binding type the connection does not offer (SQLSTATE 28000).
#[test]
fn over_tls_1_2_scram_is_bound_by_the_certificate() {
    let (pki, other) = (pki("scram-tls12"), pki("scram-tls12-other"));
    let (_db, server) = served_by(&pki, true);
    let config = tls12(&pki);
    let stream = handshaken_by(&server, Arc::clone(&config));
    let own = tls::channel(&stream);
    assert_eq!((stream.conn.protocol_version(), &own), (Some(rustls::ProtocolVersion::TLSv1_2), &by_certificate(der_of(&pki.chain))));
    let mut bound = Scram::new("ann's");
    bound.bind(&own);
    assert_eq!(bound.name(), MECHANISM_PLUS);
    assert_eq!(Connection::open(stream, "ann", &mut bound).unwrap().authorization(), "ANN", "by tls-server-end-point");
    let connected = tls::connect(server.address, ("localhost", &config), "ann", &mut Scram::new("ann's")).unwrap();
    assert_eq!(connected.authorization(), "ANN");
    let mut relayed = Scram::new("ann's");
    relayed.bind(&by_certificate(der_of(&other.chain)));
    let refused = Connection::open(handshaken_by(&server, Arc::clone(&config)), "ann", &mut relayed).err().unwrap();
    assert_eq!(state(&refused), ("28000", -919), "{refused}");
    let mut exporting = Scram::new("ann's");
    exporting.bind(&TlsChannel { exporter: Some(vec![0; 32]), ..by_certificate(der_of(&pki.chain)) });
    let unoffered = Connection::open(handshaken_by(&server, config), "ann", &mut exporting).err().unwrap();
    assert_eq!(unoffered.message, "SCRAM: this connection offers no channel binding tls-exporter: not TLS 1.3", "{unoffered}");
}
