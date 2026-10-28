//! Certificates for a server's TLS, made for each test run so none expires
//! in the repository; and a database's settings that secure its server
//! with them. With the feature `tls`.

use std::path::PathBuf;

use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use turborel::db::Database;
use turborel::value::Value;
use turborel_server::settings::{self, TLS, TLS_CERTIFICATE, TLS_KEY};

use crate::Scratch;

/// A certificate authority's certificate, and a server's chain and key
/// for "localhost" it issued, as PEM files.
pub struct Pki {
    /// Where the files are.
    pub scratch: Scratch,
    /// The authority's certificate.
    pub authority: PathBuf,
    /// The server's certificate chain.
    pub chain: PathBuf,
    /// The server's private key.
    pub key: PathBuf,
}

/// The certificates of `Pki`, for the test `test`.
pub fn pki(test: &str) -> Pki {
    let scratch = Scratch::new(test);
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let issuer = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
    let key = KeyPair::generate().unwrap();
    let certificate = CertificateParams::new(vec!["localhost".to_owned()]).unwrap().signed_by(&key, &issuer).unwrap();
    let (authority, chain, key_file) = (scratch.path("ca.pem"), scratch.path("chain.pem"), scratch.path("key.pem"));
    std::fs::write(&authority, issuer.pem()).unwrap();
    std::fs::write(&chain, certificate.pem()).unwrap();
    std::fs::write(&key_file, key.serialize_pem()).unwrap();
    Pki { scratch, authority, chain, key: key_file }
}

/// `db`'s server settings declared, and TLS set to `mode` with `pki`'s
/// files.
pub fn secured(db: &Database, pki: &Pki, mode: &str) {
    settings::declare(db).unwrap();
    let text = |s: &str| Value::Char(s.to_owned());
    db.set_setting(TLS, &text(mode)).unwrap();
    db.set_setting(TLS_CERTIFICATE, &text(&pki.chain.display().to_string())).unwrap();
    db.set_setting(TLS_KEY, &text(&pki.key.display().to_string())).unwrap();
}
