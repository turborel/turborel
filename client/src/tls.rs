//! \[DESIGN\] TLS for a client (RULES.md D26, D42), with the feature `tls`:
//! rustls, its cryptography from aws-lc-rs. The client trusts the
//! certificates it is given, and the platform's certificate authorities
//! only when asked, with the feature `system-roots` (`roots`); it checks
//! the server's certificate against the name it connects to; the handshake is done before Hello, so a server it does not
//! trust is a connection failure (SQLSTATE 08006) that says why.

use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use crate::Failure;
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
    config(authorities(roots)?)
}

/// A client's TLS that trusts the certificates in the PEM file `roots`,
/// and presents to the server the certificate chain in the PEM file
/// `chain`, its own first, its private key in the PEM file `key`: for the
/// server's mechanism `certificate` (`connection::Certificate`).
///
/// # Errors
///
/// A connection failure if a file cannot be read, holds no certificate or
/// key, or the key does not fit the certificate.
pub fn presenting(roots: &Path, identity: (&Path, &Path)) -> Result<Arc<ClientConfig>> {
    presenting_to(authorities(roots)?, identity)
}

/// A client's TLS that trusts the certificates of `store`, and presents
/// to the server the certificate chain in the PEM file `chain`, its own
/// first, its private key in the PEM file `key`, as `presenting` does.
///
/// # Errors
///
/// As `presenting`'s.
pub fn presenting_to(store: RootCertStore, (chain, key): (&Path, &Path)) -> Result<Arc<ClientConfig>> {
    let certificates = CertificateDer::pem_file_iter(chain).map_err(unreadable(chain))?;
    let certificates = certificates.collect::<std::result::Result<Vec<_>, _>>().map_err(unreadable(chain))?;
    let private = PrivateKeyDer::from_pem_file(key).map_err(unreadable(key))?;
    let presented = builder()?.with_root_certificates(store).with_client_auth_cert(certificates, private);
    Ok(Arc::new(presented.map_err(|e| failed(&format!("[DESIGN] cannot present {}: {e}", chain.display())))?))
}

/// The failure of the PEM file `path` that cannot be read.
fn unreadable(path: &Path) -> impl Fn(rustls::pki_types::pem::Error) -> Failure + '_ {
    move |e| failed(&format!("[DESIGN] cannot read {}: {e}", path.display()))
}

/// The certificates of the PEM file `roots`, to trust: one at least.
fn authorities(roots: &Path) -> Result<RootCertStore> {
    let mut store = RootCertStore::empty();
    for certificate in CertificateDer::pem_file_iter(roots).map_err(unreadable(roots))? {
        store
            .add(certificate.map_err(unreadable(roots))?)
            .map_err(|e| failed(&format!("[DESIGN] cannot trust {}: {e}", roots.display())))?;
    }
    if store.is_empty() {
        return Err(failed(&format!("[DESIGN] {} holds no certificate", roots.display())));
    }
    Ok(store)
}

/// The certificate authorities a client trusts: those of the PEM file
/// `file`, if one is named, and the platform's if `platform` says so; at
/// least one.
///
/// The platform's are its store's (rustls-native-certs): the keychain's on
/// macOS, the system store's on Windows, the system's bundle elsewhere; or
/// instead those of the files the environment's SSL_CERT_FILE and
/// SSL_CERT_DIR name, if either is set. Those of its certificates rustls
/// cannot parse are left out; none it can is a failure. Taking them trusts
/// every authority the platform does, many more than a server's own
/// usually needs: so only when asked.
///
/// # Errors
///
/// A connection failure if `file` cannot be read or holds no certificate
/// it can trust; if the platform's are asked for and none can be read, or
/// the client was built without the feature `system-roots`; or if neither
/// is asked for.
pub fn roots(file: Option<&Path>, platform: bool) -> Result<RootCertStore> {
    let mut store = match file {
        Some(path) => authorities(path)?,
        None => RootCertStore::empty(),
    };
    if platform {
        platform_authorities(&mut store)?;
    }
    if store.is_empty() {
        return Err(failed("[DESIGN] no certificate authority is trusted: name a file of them, or the platform's"));
    }
    Ok(store)
}

/// `store`, the platform's certificate authorities added: one at least.
#[cfg(feature = "system-roots")]
fn platform_authorities(store: &mut RootCertStore) -> Result<()> {
    let loaded = rustls_native_certs::load_native_certs();
    let (added, _) = store.add_parsable_certificates(loaded.certs);
    if added == 0 {
        let errors: Vec<String> = loaded.errors.iter().map(ToString::to_string).collect();
        let why = if errors.is_empty() { "it holds none".to_owned() } else { errors.join("; ") };
        return Err(failed(&format!("[DESIGN] cannot trust the platform's certificate authorities: {why}")));
    }
    Ok(())
}

/// Refused: the client was built without the feature `system-roots`.
#[cfg(not(feature = "system-roots"))]
fn platform_authorities(_: &mut RootCertStore) -> Result<()> {
    Err(failed("[DESIGN] the platform's certificate authorities are trusted only with the feature `system-roots`"))
}

/// A client's TLS, its protocol versions chosen, its trust not yet.
fn builder() -> Result<rustls::ConfigBuilder<ClientConfig, rustls::WantsVerifier>> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let versions = ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions();
    versions.map_err(|e| failed(&format!("[DESIGN] cannot make TLS: {e}")))
}

/// A client's TLS that trusts the certificates of `store`.
///
/// # Errors
///
/// A connection failure if rustls cannot make it.
pub fn config(store: RootCertStore) -> Result<Arc<ClientConfig>> {
    Ok(Arc::new(builder()?.with_root_certificates(store).with_no_client_auth()))
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
