//! \[DESIGN\] TLS (RULES.md D26, D42): rustls, its cryptography from
//! aws-lc-rs, securing a listener's connections.
//!
//! A client may speak TLS or not on the same port: a TLS record of a
//! handshake begins with the byte 22 (RFC 8446 §5.1), and no message of
//! the native protocol does, so the first byte tells them apart. With TLS
//! offered, a client without it is served in the clear; with TLS required,
//! it is refused (SQLSTATE 08004), and told why. The handshake runs in the
//! connection's own task, so a slow client never holds up the others.
//!
//! TLS 1.2 and 1.3, rustls's safe defaults. With SERVER.TLS_CLIENT_CA, a
//! client may present a certificate, verified against those authorities
//! (rustls's `WebPkiClientVerifier`), its subject's common name then known
//! to the authenticators (`Peer::certificate`, `certificate.rs`); one
//! presenting none is served as before, one presenting another's fails its
//! handshake.
//!
//! Its files are taken again without a restart (`Tls::reload`): when the
//! settings naming them change, as the next connection is secured, and
//! when the program asks, as `turboreld` does on SIGHUP. The connections
//! after are secured by the new ones; those secured go on as they are. A
//! reload that fails keeps the files it had, and is told
//! (`Event::TlsReloadFailed`).

mod subject;

use std::io;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::task::{Context, Poll};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio_rustls::TlsAcceptor;

use turborel::db::Database;
use turborel::diagnostics::Event;
use turborel::files::Noted;

use crate::settings::{self, TLS_CERTIFICATE, TLS_CLIENT_CA, TLS_KEY};
use crate::transport::Transport;

pub use subject::common_name;

/// \[DESIGN\] The first byte of a TLS record of a handshake: its content
/// type, handshake(22) [RFC 8446 §5.1].
const HANDSHAKE: u8 = 22;

/// What a connection is, once its first byte is read.
pub enum Secured {
    /// Secured by TLS; with the common name of the certificate its client
    /// presented, verified, if any (`Peer::certificate`).
    Tls(Box<dyn Transport>, Option<String>),
    /// In the clear.
    Plain(Box<dyn Transport>),
}

/// The files TLS is taken from, as the settings name them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Files {
    /// The PEM file of the server's certificate chain, its own first.
    pub chain: String,
    /// The PEM file of its private key.
    pub key: String,
    /// The PEM file of the authorities a client's certificate is verified
    /// against; empty, none asked for.
    pub authorities: String,
}

impl Files {
    /// The files `db`'s settings name: SERVER.TLS_CERTIFICATE,
    /// SERVER.TLS_KEY and SERVER.TLS_CLIENT_CA.
    pub fn of(db: &Database) -> Files {
        Files {
            chain: settings::text(db, TLS_CERTIFICATE),
            key: settings::text(db, TLS_KEY),
            authorities: settings::text(db, TLS_CLIENT_CA),
        }
    }

    /// Each file named.
    fn named(&self) -> Vec<&str> {
        [&self.chain, &self.key, &self.authorities].into_iter().map(String::as_str).filter(|f| !f.is_empty()).collect()
    }
}

/// Where a TLS's files are named: the database whose settings name them,
/// and the files it last tried to take.
struct Source {
    db: Database,
    tried: Mutex<Files>,
}

/// \[DESIGN\] TLS for a listener's connections.
pub struct Tls {
    config: RwLock<Arc<ServerConfig>>,
    required: bool,
    /// Its files, shown while they are taken (RULES.md D56).
    notes: Mutex<Vec<Noted>>,
    /// Where its files are named, if it takes them again as they change.
    source: Option<Source>,
}

/// What a lock guards, though a panic poisoned it: it is replaced whole.
fn whole<T>(locked: std::sync::LockResult<T>) -> T {
    locked.unwrap_or_else(PoisonError::into_inner)
}

/// The configuration of a server whose certificate chain is `chain`, the
/// first its own, and whose private key is `key`; asking a client for a
/// certificate of `authorities`, if given, and serving one that presents
/// none.
///
/// # Errors
///
/// rustls's, if the key does not fit the certificate or its kind is not
/// one aws-lc-rs signs with, or the authorities cannot verify.
pub fn server_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    authorities: Option<RootCertStore>,
) -> Result<Arc<ServerConfig>, rustls::Error> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let versions = ServerConfig::builder_with_provider(Arc::clone(&provider)).with_safe_default_protocol_versions()?;
    let config = match authorities {
        None => versions.with_no_client_auth(),
        Some(roots) => {
            let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider).allow_unauthenticated().build();
            versions.with_client_cert_verifier(verifier.map_err(|e| rustls::Error::General(e.to_string()))?)
        }
    };
    Ok(Arc::new(config.with_single_cert(chain, key)?))
}

fn invalid(what: &str, e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{what}: {e}"))
}

/// The certificates of the PEM file `path`: one at least.
fn certificates(path: &str) -> io::Result<Vec<CertificateDer<'static>>> {
    let found = CertificateDer::pem_file_iter(Path::new(path)).map_err(|e| invalid(path, e))?;
    let found = found.collect::<Result<Vec<_>, _>>().map_err(|e| invalid(path, e))?;
    if found.is_empty() {
        return Err(invalid(path, "no certificate"));
    }
    Ok(found)
}

/// The authorities of the PEM file `path`, if it is named.
fn authorities(path: &str) -> io::Result<Option<RootCertStore>> {
    if path.is_empty() {
        return Ok(None);
    }
    let mut roots = RootCertStore::empty();
    for certificate in certificates(path)? {
        roots.add(certificate).map_err(|e| invalid(path, e))?;
    }
    Ok(Some(roots))
}

/// The configuration `files` make.
///
/// # Errors
///
/// `InvalidData` if the chain or key is not named, a file cannot be read,
/// holds no certificate or key, or the key does not fit the certificate.
fn load(files: &Files) -> io::Result<Arc<ServerConfig>> {
    if files.chain.is_empty() || files.key.is_empty() {
        return Err(invalid("TLS", "SERVER.TLS_CERTIFICATE and SERVER.TLS_KEY name its files"));
    }
    let chain = certificates(&files.chain)?;
    let private = PrivateKeyDer::from_pem_file(Path::new(&files.key)).map_err(|e| invalid(&files.key, e))?;
    server_config(chain, private, authorities(&files.authorities)?).map_err(|e| invalid(&files.chain, e))
}

/// `files`, shown among `db`'s files (RULES.md D56), each read once.
///
/// # Errors
///
/// `Other` if the database is closed.
fn noted(db: &Database, files: &Files) -> io::Result<Vec<Noted>> {
    let notes = files.named().into_iter().map(|f| db.note_file(Path::new(f), false)).collect::<turborel::error::Result<Vec<_>>>();
    let notes = notes.map_err(|e| io::Error::other(e.msg))?;
    notes.iter().for_each(Noted::read);
    Ok(notes)
}

impl Tls {
    /// TLS by `config`; `required` if a client in the clear is refused.
    pub fn new(config: Arc<ServerConfig>, required: bool) -> Tls {
        Tls { config: RwLock::new(config), required, notes: Mutex::new(Vec::new()), source: None }
    }

    /// TLS of `files`.
    ///
    /// # Errors
    ///
    /// `InvalidData` if the chain or key is not named, a file cannot be
    /// read, holds no certificate or key, or the key does not fit the
    /// certificate.
    pub fn from_files(files: &Files, required: bool) -> io::Result<Tls> {
        Ok(Tls::new(load(files)?, required))
    }

    /// It, its files as `db`'s settings name them, shown among `db`'s
    /// files, taken again when the settings change or `reload` asks.
    ///
    /// # Errors
    ///
    /// `Other` if the database is closed.
    pub fn of(self, db: &Database) -> io::Result<Tls> {
        let files = Files::of(db);
        let notes = noted(db, &files)?;
        Ok(Tls { notes: Mutex::new(notes), source: Some(Source { db: db.clone(), tried: Mutex::new(files) }), ..self })
    }

    /// Takes its files again, as its database's settings name them, for
    /// the connections after; told either way, and, failing, keeping those
    /// it had. Whether it took them; false of a TLS not of a database's
    /// settings.
    pub fn reload(&self) -> bool {
        let Some(source) = &self.source else { return false };
        let files = Files::of(&source.db);
        whole(source.tried.lock()).clone_from(&files);
        match load(&files).and_then(|config| Ok((config, noted(&source.db, &files)?))) {
            Ok((config, notes)) => {
                *whole(self.config.write()) = config;
                *whole(self.notes.lock()) = notes;
                source.db.tell(&Event::TlsReloaded { certificate: files.chain });
                true
            }
            Err(e) => {
                source.db.tell(&Event::TlsReloadFailed { message: e.to_string() });
                false
            }
        }
    }

    /// Takes its files again if the settings name others than it last
    /// tried to take.
    fn refresh(&self) {
        let Some(source) = &self.source else { return };
        let changed = Files::of(&source.db) != *whole(source.tried.lock());
        if changed {
            self.reload();
        }
    }

    /// Whether a client in the clear is refused.
    pub fn required(&self) -> bool {
        self.required
    }

    /// `transport`, secured by TLS if its client begins a handshake, else
    /// as it is; the byte read to tell is read again.
    ///
    /// # Errors
    ///
    /// The connection's, if it ends before a byte, or the handshake fails.
    pub async fn secure(&self, mut transport: Box<dyn Transport>) -> io::Result<Secured> {
        let first = transport.read_u8().await?;
        let prefixed = Prefixed { first: Some(first), inner: transport };
        if first != HANDSHAKE {
            return Ok(Secured::Plain(Box::new(prefixed)));
        }
        self.refresh();
        let config = Arc::clone(&*whole(self.config.read()));
        let acceptor = TlsAcceptor::from(config);
        let secured = acceptor.accept(prefixed).await?;
        let presented = secured.get_ref().1.peer_certificates().and_then(<[_]>::first).and_then(|c| common_name(c));
        Ok(Secured::Tls(Box::new(secured), presented))
    }
}

/// A stream with a byte read from it put back before it.
struct Prefixed {
    first: Option<u8>,
    inner: Box<dyn Transport>,
}

impl AsyncRead for Prefixed {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if buf.remaining() > 0
            && let Some(first) = self.first.take()
        {
            buf.put_slice(&[first]);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Prefixed {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
