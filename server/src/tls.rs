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
//! TLS 1.2 and 1.3, rustls's safe defaults; the client is not asked for a
//! certificate.

use std::io;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio_rustls::TlsAcceptor;

use turborel::files::Noted;

use crate::transport::Transport;

/// \[DESIGN\] The first byte of a TLS record of a handshake: its content
/// type, handshake(22) [RFC 8446 §5.1].
const HANDSHAKE: u8 = 22;

/// What a connection is, once its first byte is read.
pub enum Secured {
    /// Secured by TLS.
    Tls(Box<dyn Transport>),
    /// In the clear.
    Plain(Box<dyn Transport>),
}

/// \[DESIGN\] TLS for a listener's connections.
pub struct Tls {
    acceptor: TlsAcceptor,
    required: bool,
    /// Its files, shown while it is taken (RULES.md D56).
    _notes: Vec<Noted>,
}

/// The configuration of a server whose certificate chain is `chain`, the
/// first its own, and whose private key is `key`.
///
/// # Errors
///
/// rustls's, if the key does not fit the certificate or its kind is not
/// one aws-lc-rs signs with.
pub fn server_config(chain: Vec<CertificateDer<'static>>, key: PrivateKeyDer<'static>) -> Result<Arc<ServerConfig>, rustls::Error> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = ServerConfig::builder_with_provider(provider).with_safe_default_protocol_versions()?.with_no_client_auth();
    Ok(Arc::new(config.with_single_cert(chain, key)?))
}

fn invalid(what: &Path, e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", what.display()))
}

impl Tls {
    /// TLS by `config`; `required` if a client in the clear is refused.
    pub fn new(config: Arc<ServerConfig>, required: bool) -> Tls {
        Tls { acceptor: TlsAcceptor::from(config), required, _notes: Vec::new() }
    }

    /// It, its files shown as `notes` say while it is taken.
    #[must_use]
    pub fn noting(self, notes: Vec<Noted>) -> Tls {
        Tls { _notes: notes, ..self }
    }

    /// TLS with the certificate chain in the PEM file `chain` and the
    /// private key in the PEM file `key`.
    ///
    /// # Errors
    ///
    /// `InvalidData` if a file cannot be read, holds no certificate or
    /// key, or the key does not fit the certificate.
    pub fn from_files(chain: &Path, key: &Path, required: bool) -> io::Result<Tls> {
        let certificates = CertificateDer::pem_file_iter(chain).map_err(|e| invalid(chain, e))?;
        let certificates = certificates.collect::<Result<Vec<_>, _>>().map_err(|e| invalid(chain, e))?;
        if certificates.is_empty() {
            return Err(invalid(chain, "no certificate"));
        }
        let private = PrivateKeyDer::from_pem_file(key).map_err(|e| invalid(key, e))?;
        Ok(Tls::new(server_config(certificates, private).map_err(|e| invalid(chain, e))?, required))
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
        let secured = self.acceptor.accept(prefixed).await?;
        Ok(Secured::Tls(Box::new(secured)))
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
