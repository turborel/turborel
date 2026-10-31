//! Certificates: a client proving who it is by the certificate it
//! presents over TLS; and the server's certificate, key and client
//! authorities taken again without a restart.

use std::sync::Arc;

use turborel::db::Database;
use turborel::value::Value;
use turborel_client::connection::{Certificate, Trust};
use turborel_client::tls;
use turborel_server::certificate::{Certificate as Certified, MECHANISM};
use turborel_server::listening;
use turborel_server::settings::{TLS_CERTIFICATE, TLS_CLIENT_CA, TLS_KEY};
use turborel_server::tls::{Files, Tls};

use crate::common::{Kept, Pki, Running, client_certificate, over_tls, pki, secured, start_with, state};

/// Sets `db`'s text setting `name` to `value`.
fn text(db: &Database, name: &str, value: &str) {
    db.set_setting(name, &Value::Char(value.to_owned())).unwrap();
}

/// A server of `pki`, its TLS offered, asking clients for certificates of
/// `pki`'s authority, whose users are ANN and BOB, taken by certificate
/// beside trust.
fn served(pki: &Pki) -> (Database, Running) {
    let db = Database::new();
    secured(&db, pki, "ON");
    text(&db, TLS_CLIENT_CA, &pki.authority.display().to_string());
    db.create_user("ANN", b"v").unwrap();
    db.create_user("BOB", b"v").unwrap();
    let server = start_with(&db, &[], vec![Arc::new(Certified::new(db.clone()))]);
    (db, server)
}

/// A connection over TLS to `server` as `user`, by the certificate of the
/// common name `name` `issuer`'s authority issued, trusting `pki`'s.
fn presenting(server: &Running, (pki, issuer): (&Pki, &Pki), (name, user): (&str, &str)) -> turborel_client::connection::Result<String> {
    let identity = client_certificate(issuer, name);
    let config = tls::presenting(&pki.authority, (&identity.chain, &identity.key))?;
    Ok(tls::connect(server.address, ("localhost", &config), user, &mut Certificate)?.authorization().to_owned())
}

/// Verifies: [DESIGN] RULES.md D42, D43: with SERVER.TLS_CLIENT_CA, a
/// client presenting a certificate its authorities issued logs in as the
/// user the certificate's common name names, read as an identifier, so
/// `ann` is ANN; as another user, or of a name no user's, it is refused
/// (SQLSTATE 28000), and so is one presenting none; one of other
/// authorities fails its TLS (SQLSTATE 08006). A client presenting none is
/// served by other mechanisms. With SERVER.TLS_CLIENT_CA emptied, no
/// certificate is asked for, so none proves a user.
#[test]
fn a_certificate_proves_its_user() {
    let pki = pki("certificates");
    let other = self::pki("certificates-other");
    let (db, server) = served(&pki);
    assert_eq!(presenting(&server, (&pki, &pki), ("ann", "ann")).unwrap(), "ANN");
    let refused = presenting(&server, (&pki, &pki), ("ann", "bob")).unwrap_err();
    assert_eq!((state(&refused), refused.message.as_str()), (("28000", -919), "the certificate is not of the user named"));
    assert_eq!(state(&presenting(&server, (&pki, &pki), ("carol", "carol")).unwrap_err()).0, "28000", "no user");
    let config = tls::trusting(&pki.authority).unwrap();
    let none = tls::connect(server.address, ("localhost", &config), "ann", &mut Certificate).err().unwrap();
    assert!(none.message.contains("no certificate"), "{none}");
    assert_eq!(state(&presenting(&server, (&pki, &other), ("ann", "ann")).unwrap_err()).0, "08006", "another authority's");
    assert_eq!(tls::connect(server.address, ("localhost", &config), "x", &mut Trust).unwrap().authorization(), "X");
    text(&db, TLS_CLIENT_CA, "");
    let unasked = presenting(&server, (&pki, &pki), ("ann", "ann")).unwrap_err();
    assert!(unasked.message.contains("no certificate"), "{unasked}");
    assert_eq!(MECHANISM, "certificate");
}

/// Verifies: [DESIGN] RULES.md D42, R-ERR-1: a client is not made to
/// present a certificate whose chain or key cannot be read, or whose key
/// is another's (SQLSTATE 08006, saying why); `certificate` answers no
/// challenge.
#[test]
fn a_client_presents_only_what_it_can() {
    let pki = pki("certificates-files");
    let ann = client_certificate(&pki, "ann");
    let bob = client_certificate(&pki, "bob");
    let none = pki.scratch.path("none.pem");
    let refused = |chain: &std::path::Path, key: &std::path::Path| tls::presenting(&pki.authority, (chain, key)).err().unwrap();
    assert!(refused(&none, &ann.key).message.contains("cannot read"));
    assert!(refused(&ann.chain, &none).message.contains("cannot read"));
    let mismatched = refused(&ann.chain, &bob.key);
    assert_eq!((state(&mismatched).0, mismatched.message.contains("cannot present")), ("08006", true), "{mismatched}");
    assert!(turborel_client::connection::Mechanism::answer(&mut Certificate, b"").is_err());
}

/// Verifies: [DESIGN] RULES.md D40, D42: a change of SERVER.TLS_CERTIFICATE
/// and SERVER.TLS_KEY is taken as the next connection is secured, which
/// the new certificate secures, told once; a change naming files that
/// cannot be taken keeps the certificate there was, told once, however
/// many connections come after.
#[test]
fn tls_files_are_taken_again_as_their_settings_change() {
    let (old, new) = (pki("reload-old"), pki("reload-new"));
    let kept = Kept::default();
    let db = Database::new();
    db.set_diagnostics(Box::new(kept.clone()));
    secured(&db, &old, "ON");
    let server = start_with(&db, &[], vec![]);
    over_tls(&server, &old, "localhost").unwrap().terminate().unwrap();
    text(&db, TLS_CERTIFICATE, &new.chain.display().to_string());
    text(&db, TLS_KEY, &new.key.display().to_string());
    over_tls(&server, &new, "localhost").unwrap().terminate().unwrap();
    assert_eq!(state(&over_tls(&server, &old, "localhost").err().unwrap()).0, "08006", "the old one gone");
    assert_eq!(kept.of("TlsReloaded").len(), 1, "{:?}", kept.all());
    text(&db, TLS_CERTIFICATE, &new.scratch.path("none.pem").display().to_string());
    over_tls(&server, &new, "localhost").unwrap().terminate().unwrap();
    over_tls(&server, &new, "localhost").unwrap().terminate().unwrap();
    let failed = kept.of("TlsReloadFailed");
    assert!(failed.len() == 1 && failed[0].contains("none.pem"), "{failed:?}");
}

/// Verifies: [DESIGN] RULES.md D42, R-ERR-1: a server's TLS takes its
/// files again when asked, as `turboreld` asks on SIGHUP: their contents
/// changed in place are taken; files that cannot be read, or a key not of
/// the certificate, are refused, the TLS keeping what it had. A TLS not
/// of a database's settings has nothing to take again.
#[test]
fn a_tls_takes_its_files_again_when_asked() {
    let (old, new) = (pki("reload-asked"), pki("reload-asked-new"));
    let db = Database::new();
    secured(&db, &old, "ON");
    let taken = listening::tls(&db).unwrap().unwrap();
    std::fs::copy(&new.chain, &old.chain).unwrap();
    assert!(!taken.reload(), "a key not of the certificate");
    std::fs::copy(&new.key, &old.key).unwrap();
    assert!(taken.reload());
    std::fs::write(&old.chain, b"").unwrap();
    assert!(!taken.reload(), "no certificate");
    text(&db, TLS_KEY, "");
    assert!(!taken.reload(), "no key named");
    let files = Files { chain: new.chain.display().to_string(), key: new.key.display().to_string(), authorities: String::new() };
    assert!(!Tls::from_files(&files, false).unwrap().reload(), "of no settings");
    let unreadable = Files { authorities: old.scratch.path("none.pem").display().to_string(), ..files };
    assert!(Tls::from_files(&unreadable, false).is_err(), "authorities not there");
}
