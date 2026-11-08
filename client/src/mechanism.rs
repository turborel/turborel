//! \[DESIGN\] A client's side of the mechanisms of authentication
//! (RULES.md D43). `Mechanism` is what a connection proves its user by,
//! and `trust`, `password` and `certificate` have their sides here;
//! SCRAM's is in `scram`, with the feature `scram`.

use crate::connection::{Result, failed};

/// \[DESIGN\] A client's side of a mechanism of authentication: its name,
/// its first bytes, and its answer to each challenge. The server's
/// `Authenticator` of the same name is the other side.
pub trait Mechanism {
    /// The mechanism's name, as the server knows it.
    fn name(&self) -> &str;
    /// The bytes sent with Hello.
    fn initial(&mut self) -> Vec<u8>;
    /// The answer to the server's challenge `challenge`.
    ///
    /// # Errors
    ///
    /// If it has none: the connection is then given up.
    fn answer(&mut self, challenge: &[u8]) -> Result<Vec<u8>>;
    /// Whether what it sends gives away what proves the user, as a
    /// password does, or lets one who overhears it guess that at leisure,
    /// as SCRAM's proof does: if so, a connection over plain TCP refuses it
    /// before anything is sent (`connect`, `target::connect_to`). None does
    /// by default.
    fn secret(&self) -> bool {
        false
    }
    /// Whether the exchange may end as the server sends Ready: a mechanism
    /// by which the server proves itself too, as SCRAM's, refuses one that
    /// did not.
    ///
    /// # Errors
    ///
    /// If the server has not proved itself: the connection is then given
    /// up. None does by default.
    fn ready(&self) -> Result<()> {
        Ok(())
    }
    /// The connection about to carry it is secured by TLS, of `channel`:
    /// a mechanism that binds itself to its channel, as SCRAM's PLUS does,
    /// takes it, for this connection alone. Called before `name` and
    /// `initial` by `tls::connect` and `target::connect_to`; a program
    /// opening a TLS stream of its own calls it, of `tls::channel`, before
    /// `Connection::open`. None takes it by default.
    fn bind(&mut self, _channel: &TlsChannel) {}
}

/// \[DESIGN\] What a TLS connection's handshake gives a mechanism to bind
/// itself to (`Mechanism::bind`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsChannel {
    /// The certificate the server presented, its chain's first, as DER.
    pub certificate: Option<Vec<u8>>,
    /// The connection's channel binding `tls-exporter` [RFC 9266 §2]: the
    /// keying material its session exports, over TLS 1.3 alone.
    pub exporter: Option<Vec<u8>>,
}

/// \[DESIGN\] The client's side of `trust`: nothing to prove, so a server
/// takes it only from a client on its own machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct Trust;

/// The name of the mechanism `Trust` is a side of.
pub const TRUST: &str = "trust";

impl Mechanism for Trust {
    fn name(&self) -> &str {
        TRUST
    }
    fn initial(&mut self) -> Vec<u8> {
        Vec::new()
    }
    fn answer(&mut self, _: &[u8]) -> Result<Vec<u8>> {
        Err(failed("[DESIGN] trust has no answer to a challenge"))
    }
}

/// \[DESIGN\] The client's side of `password`: the password, sent whole,
/// so only over TLS, a Unix socket or a named pipe. A connection over
/// plain TCP refuses it before it sends anything, as the server would
/// after.
#[derive(Clone)]
pub struct Password(pub String);

/// The name of the mechanism `Password` is a side of.
pub const PASSWORD: &str = "password";

impl std::fmt::Debug for Password {
    /// Never the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Password(..)")
    }
}

impl Mechanism for Password {
    fn name(&self) -> &str {
        PASSWORD
    }
    fn initial(&mut self) -> Vec<u8> {
        self.0.as_bytes().to_vec()
    }
    fn answer(&mut self, _: &[u8]) -> Result<Vec<u8>> {
        Err(failed("[DESIGN] password has no answer to a challenge"))
    }
    fn secret(&self) -> bool {
        true
    }
}

/// \[DESIGN\] The client's side of `certificate`: nothing sent, the
/// certificate presented over TLS (`tls::presenting`, with the feature
/// `tls`) proving the user its common name names.
#[derive(Debug, Clone, Copy, Default)]
pub struct Certificate;

/// The name of the mechanism `Certificate` is a side of.
pub const CERTIFICATE: &str = "certificate";

impl Mechanism for Certificate {
    fn name(&self) -> &str {
        CERTIFICATE
    }
    fn initial(&mut self) -> Vec<u8> {
        Vec::new()
    }
    fn answer(&mut self, _: &[u8]) -> Result<Vec<u8>> {
        Err(failed("[DESIGN] certificate has no answer to a challenge"))
    }
}
