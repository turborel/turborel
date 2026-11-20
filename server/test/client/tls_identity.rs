//! A client certificate presented by settings, and the files of TLS that
//! cannot be read or do not go together (RULES.md D204).

use std::sync::Arc;

use turborel::db::Database;
use turborel::value::Value;
use turborel_client::settings::{Options, Settings};
use turborel_server::certificate::Certificate;
use turborel_server::settings::TLS_CLIENT_CA;

use crate::common::{client_certificate, pki, rows, secured, start_with};

/// The settings of the URI `uri`, read with no environment.
fn settings(uri: &str) -> Settings {
    Settings::resolve_in(&Options::from_uri(uri).unwrap(), |_| None).unwrap()
}

/// Verifies: [DESIGN] RULES.md D204: settings naming a certificate and
/// its key present them, and a server that takes certificates proves the
/// user its common name names; a certificate named without its key, or
/// a file that cannot be read, is a connection failure that says so;
/// `verify-ca` refuses a server whose authority is not trusted, whatever
/// its name.
#[test]
fn a_client_certificate_is_presented_by_settings() {
    let (pki, other) = (pki("client-tls-identity"), pki("client-tls-identity-other"));
    let db = Database::new();
    secured(&db, &pki, "ON");
    db.set_setting(TLS_CLIENT_CA, &Value::Char(pki.authority.display().to_string())).unwrap();
    db.create_user("ANN", b"unused").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE T (K INTEGER)").unwrap();
    let server = start_with(&db, &[], vec![Arc::new(Certificate::new(db.clone()))]);
    let ann = client_certificate(&pki, "ANN");
    let at = server.address;
    let (chain, key) = (ann.chain.display(), ann.key.display());
    let base = format!("turborel://ANN@{at}/?tls_mode=require&tls_certificate={chain}");
    let mut proved = settings(&format!("{base}&tls_key={key}")).connect().unwrap();
    assert_eq!(proved.authorization(), "ANN");
    assert_eq!(rows(&proved.query("SELECT COUNT(*) FROM T").unwrap()), ["0"]);
    proved.terminate().unwrap();
    let keyless = settings(&base).connect().unwrap_err();
    assert!(keyless.message.contains("presented with its key, and none is named"), "{}", keyless.message);
    let missing = pki.scratch.path("none.pem").display().to_string();
    let unread = settings(&format!("turborel://ANN@{at}/?tls_mode=require&tls_certificate={missing}&tls_key={key}")).connect();
    assert!(unread.is_err());
    let unread_key = settings(&format!("{base}&tls_key={missing}")).connect();
    assert!(unread_key.is_err());
    let roots = other.authority.display();
    let untrusted = settings(&format!("turborel://ANN@{at}/?tls_mode=verify-ca&tls_roots={roots}&server_name=localhost")).connect();
    assert!(untrusted.unwrap_err().message.contains("handshake"));
}
