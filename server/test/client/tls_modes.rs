//! TLS by mode and channel binding (RULES.md D204), the password file
//! (D203) and the SCRAM key cache (D205), as settings connect to a server.

use std::num::NonZeroU32;
use std::sync::Arc;

use turborel::db::Database;
use turborel_client::settings::{Options, Settings};
use turborel_client::target::Target;
use turborel_server::auth::Authenticator;
use turborel_server::password::Pbkdf2;
use turborel_server::scram::Scram as Scrammed;

use crate::common::{Pki, Running, pki, rows, secured, start, start_with};

/// A server of `pki` whose TLS is `tls`, and whose user ANN proves the
/// password "ann's" by SCRAM-SHA-256, or SCRAM-SHA-256-PLUS, beside
/// trust.
fn served(pki: &Pki, tls: &str) -> (Database, Running) {
    let db = Database::new();
    secured(&db, pki, tls);
    let made = Pbkdf2::new(NonZeroU32::new(10_000).unwrap());
    db.create_user("ANN", made.make("ann's").unwrap().as_bytes()).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let scram = Scrammed::new(db.clone(), &made).unwrap();
    let mechanisms: Vec<Arc<dyn Authenticator>> = vec![Arc::new(scram.plus()), Arc::new(scram)];
    let server = start_with(&db, &[], mechanisms);
    (db, server)
}

/// The settings of the URI `uri`, read with no environment.
fn settings(uri: &str) -> Settings {
    Settings::resolve_in(&Options::from_uri(uri).unwrap(), |_| None).unwrap()
}

/// Whether TLS secures the connection `settings` make.
fn secure(settings: &Settings) -> bool {
    settings.connect().unwrap().target().is_some_and(Target::is_tls)
}

/// Verifies: [DESIGN] RULES.md D204: to a server offering TLS, `prefer`
/// and `require` connect over it, its certificate unchecked; `verify-full`
/// over it once the certificate is of an authority trusted and of the
/// name asked, refusing another name; `verify-ca` takes another name, but
/// not no authority; `disable` connects in the clear.
#[test]
fn each_tls_mode_checks_as_it_says() {
    let pki = pki("client-tls-modes");
    let (_db, server) = served(&pki, "ON");
    let roots = pki.authority.display().to_string();
    let at = server.address;
    assert!(secure(&settings(&format!("turborel://X@{at}/?tls_mode=prefer"))));
    assert!(secure(&settings(&format!("turborel://X@{at}/?tls_mode=require"))));
    assert!(!secure(&settings(&format!("turborel://X@{at}/?tls_mode=disable"))));
    let full = format!("turborel://X@{at}/?tls_mode=verify-full&tls_roots={roots}");
    assert!(secure(&settings(&format!("{full}&server_name=localhost"))));
    let named = settings(&format!("{full}&server_name=elsewhere")).connect().unwrap_err();
    assert!(named.message.contains("handshake"), "{}", named.message);
    assert!(secure(&settings(&format!("turborel://X@{at}/?tls_mode=verify-ca&tls_roots={roots}&server_name=elsewhere"))));
    let none = settings(&format!("turborel://X@{at}/?tls_mode=verify-ca")).connect().unwrap_err();
    assert!(none.message.contains("no certificate authority is trusted"), "{}", none.message);
}

/// Verifies: [DESIGN] RULES.md D204: to a server offering no TLS,
/// `prefer` connects in the clear, as the handshake fails; `require`
/// does not connect.
#[test]
fn prefer_falls_back_and_require_does_not() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let server = start(&db, &[]);
    let mut plain = settings(&format!("turborel://X@{}/?tls_mode=prefer", server.address)).connect().unwrap();
    assert!(plain.target().is_some_and(Target::is_plain_tcp));
    assert_eq!(rows(&plain.query("SELECT COUNT(*) FROM T").unwrap()), ["0"]);
    assert!(settings(&format!("turborel://X@{}/?tls_mode=require", server.address)).connect().is_err());
}

/// Verifies: [DESIGN] RULES.md D203, D204, D205: a password from the
/// password file proves the user by SCRAM over TLS, its channel bound
/// where `channel_binding=require` asks; a reconnection by the same
/// settings takes its keys from their cache; binding required in the
/// clear, or of trust, is refused before any connection (SQLSTATE 28000).
#[test]
fn a_password_file_and_bound_scram_and_the_key_cache() {
    let pki = pki("client-scram-settings");
    let (_db, server) = served(&pki, "ON");
    let at = server.address;
    let passfile = pki.scratch.path("pass");
    std::fs::write(&passfile, format!("# servers\n{}:{}:ann:ann\\'s\n", at.ip(), at.port())).unwrap();
    let uri = format!(
        "turborel://ann@{at}/?tls_mode=verify-full&tls_roots={}&server_name=localhost&channel_binding=require&passfile={}",
        pki.authority.display(),
        passfile.display()
    );
    let s = settings(&uri);
    assert_eq!(s.connect().unwrap().authorization(), "ANN");
    assert_eq!(s.key_cache.lookups(), (0, 1));
    assert_eq!(s.connect().unwrap().authorization(), "ANN");
    assert_eq!(s.key_cache.lookups(), (1, 1), "the keys were the cache's");
    let clear = settings(&format!("turborel://ann:x@{at}/?tls_mode=disable&channel_binding=require")).connect().unwrap_err();
    assert_eq!(clear.sqlstate, "28000");
    let trusted = settings(&format!("turborel://X@{at}/?tls_mode=require&channel_binding=require")).connect().unwrap_err();
    assert_eq!(trusted.sqlstate, "28000");
}
