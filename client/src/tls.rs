//! \[DESIGN\] TLS for a client (RULES.md D26, D42), with the feature `tls`:
//! rustls, its cryptography from aws-lc-rs. The client trusts the
//! certificates it is given, and checks the server's against the name it
//! connects to; the handshake is done before Hello, so a server it does not
//! trust is a connection failure (SQLSTATE 08006) that says why.

use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use crate::connection::{Connection, Mechanism, Result, failed};
use crate::target::Target;
use crate::timeouts::{Socket, Timeouts, tcp_to};

/// A connection's stream, secured.
pub type TlsStream = StreamOwned<ClientConnection, TcpStream>;

/// A client's TLS that trusts the certificates in the PEM file `roots`,
/// and only them.
///
/// # Errors
///
/// A connection failure if the file cannot be read or holds no
/// certificate it can trust.
pub fn trusting(roots: &Path) -> Result<Arc<ClientConfig>> {
    let unreadable = |e: rustls::pki_types::pem::Error| failed(&format!("[DESIGN] cannot read {}: {e}", roots.display()));
    let mut store = RootCertStore::empty();
    for certificate in CertificateDer::pem_file_iter(roots).map_err(unreadable)? {
        store.add(certificate.map_err(unreadable)?).map_err(|e| failed(&format!("[DESIGN] cannot trust {}: {e}", roots.display())))?;
    }
    if store.is_empty() {
        return Err(failed(&format!("[DESIGN] {} holds no certificate", roots.display())));
    }
    config(store)
}

/// A client's TLS that trusts the certificates of `store`.
///
/// # Errors
///
/// A connection failure if rustls cannot make it.
pub fn config(store: RootCertStore) -> Result<Arc<ClientConfig>> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let versions = ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions();
    let builder = versions.map_err(|e| failed(&format!("[DESIGN] cannot make TLS: {e}")))?;
    Ok(Arc::new(builder.with_root_certificates(store).with_no_client_auth()))
}

/// Connects over TLS, by `config`, to the server named `name` at
/// `address`, as `user`, authenticated by `mechanism`; waiting as
/// `Timeouts::default` says, its handshake as a step of its login.
///
/// # Errors
///
/// As `Connection::open`; a connection failure if none can be made, in
/// time, or the handshake fails: the server's certificate is not trusted,
/// or not of `name`.
pub fn connect(
    address: impl ToSocketAddrs,
    (name, config): (&str, &Arc<ClientConfig>),
    user: &str,
    mechanism: &mut dyn Mechanism,
) -> Result<Connection<TlsStream>> {
    let timeouts = Timeouts::default();
    let tcp = tcp_to(address, timeouts.connect)?;
    // Where it reached, for a connection made again to cancel (`Target`).
    let server = tcp.peer_addr().map_err(|e| failed(&format!("[DESIGN] cannot set the connection up: {e}")))?;
    let socket = Socket::tcp(&tcp)?;
    socket.limit(timeouts.connect)?;
    let stream = secured(tcp, (name, config))?;
    let reach = Target::Tls(server, name.to_owned(), Arc::clone(config));
    Ok(Connection::open_timed(stream, socket, (user, mechanism), timeouts)?.reached_by(reach))
}

/// The connection `tcp` to the server of the name `name`, secured by TLS
/// as `config` says, its handshake done.
///
/// # Errors
///
/// As `connect`'s.
pub(crate) fn secured(mut tcp: TcpStream, (name, config): (&str, &Arc<ClientConfig>)) -> Result<TlsStream> {
    let server = ServerName::try_from(name.to_owned()).map_err(|e| failed(&format!("[DESIGN] {name} is no server's name: {e}")))?;
    let mut session = ClientConnection::new(Arc::clone(config), server).map_err(|e| failed(&format!("[DESIGN] cannot begin TLS: {e}")))?;
    tcp.set_nodelay(true).map_err(|e| failed(&format!("[DESIGN] cannot set the connection up: {e}")))?;
    while session.is_handshaking() {
        session.complete_io(&mut tcp).map_err(|e| failed(&format!("[DESIGN] the TLS handshake failed: {e}")))?;
    }
    Ok(StreamOwned::new(session, tcp))
}
