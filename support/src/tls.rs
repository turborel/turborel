//! Certificates for a server's TLS, made for each test run so none expires
//! in the repository; certificates its clients present; and a database's
//! settings that secure its server with them. With the feature `tls`.

use std::path::PathBuf;

use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair};
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
    /// The authority, to issue more.
    issuer: CertifiedIssuer<'static, KeyPair>,
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
    Pki { scratch, authority, chain, key: key_file, issuer }
}

/// A client's certificate chain and private key, as PEM files.
pub struct Identity {
    /// The certificate chain.
    pub chain: PathBuf,
    /// The private key.
    pub key: PathBuf,
}

/// A certificate `pki`'s authority issued a client, its subject's common
/// name `name`, beside `pki`'s files.
pub fn client_certificate(pki: &Pki, name: &str) -> Identity {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.distinguished_name.push(DnType::CommonName, name);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let key = KeyPair::generate().unwrap();
    let certificate = params.signed_by(&key, &pki.issuer).unwrap();
    let identity = Identity { chain: pki.scratch.path(&format!("{name}.pem")), key: pki.scratch.path(&format!("{name}.key")) };
    std::fs::write(&identity.chain, certificate.pem()).unwrap();
    std::fs::write(&identity.key, key.serialize_pem()).unwrap();
    identity
}

/// A certificate for "localhost" signed by its own Ed25519 key, as DER: of
/// a signature algorithm of no separate hash, so one RFC 5929 §4.1 gives
/// no `tls-server-end-point` binding.
pub fn ed25519_certificate() -> Vec<u8> {
    let key = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    CertificateParams::new(vec!["localhost".to_owned()]).unwrap().self_signed(&key).unwrap().der().to_vec()
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
