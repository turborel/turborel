//! \[DESIGN\] Authentication (RULES.md D42): an `Authenticator` plug-in
//! turns a user and what the client proves into the authorization
//! identifier of a session, by an exchange of as many steps as its
//! mechanism needs, so challenge and response fit as well as one step.
//! A step may be slow, a password hashed slowly on purpose (D26), so the
//! server runs each on a blocking thread of its logins (`logins.rs`), never
//! on a thread doing I/O, nor on a worker statements need.

use std::net::SocketAddr;

/// Where a client connected from, as its transport says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A TCP client at this address.
    Tcp(SocketAddr),
    /// A client of a Unix socket, or a named pipe on Windows: on this
    /// machine.
    Local,
}

/// Who connected: where from, whether what it sends is kept from others,
/// so an authenticator may refuse a secret sent in the clear, the
/// certificate it presented, if any, and its connection's channel binding,
/// if it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// Where from.
    pub origin: Origin,
    /// Whether the connection is secure: over TLS, or a Unix socket or a
    /// named pipe, which only this machine reaches.
    pub secure: bool,
    /// The common name of the subject of the certificate the client
    /// presented over TLS, once TLS verified it against SERVER.TLS_CLIENT_CA
    /// (`certificate.rs`); none if it presented none, or one without a
    /// single common name.
    pub certificate: Option<String>,
    /// The channel binding `tls-server-end-point` of the connection [RFC
    /// 5929 §4]: the hash of the certificate the server presented in its
    /// TLS handshake (`turborel_protocol::scram::end_point`), which SCRAM's
    /// PLUS mechanism binds to; none off TLS, or of a certificate that
    /// defines none.
    pub end_point: Option<Vec<u8>>,
    /// The channel binding `tls-exporter` of the connection [RFC 9266 §2]:
    /// the keying material its TLS 1.3 session exports
    /// (`turborel_protocol::scram::EXPORTER_LABEL`), which SCRAM's PLUS
    /// mechanism binds to; none off TLS, or over TLS 1.2.
    pub exporter: Option<Vec<u8>>,
}

impl Peer {
    /// A TCP client at `address`, not secured yet.
    pub fn tcp(address: SocketAddr) -> Peer {
        Peer { origin: Origin::Tcp(address), secure: false, certificate: None, end_point: None, exporter: None }
    }

    /// A client of a Unix socket or a named pipe: secure.
    pub fn local() -> Peer {
        Peer { origin: Origin::Local, secure: true, certificate: None, end_point: None, exporter: None }
    }

    /// Where from, as the server shows and tells it: its address, or
    /// `local`.
    pub fn name(&self) -> String {
        match self.origin {
            Origin::Tcp(address) => address.to_string(),
            Origin::Local => "local".to_owned(),
        }
    }

    /// Whether the client is on the server's own machine: a loopback
    /// address, an IPv4 one written as IPv6 too, or a Unix socket or a
    /// named pipe.
    pub fn is_local(&self) -> bool {
        match self.origin {
            Origin::Tcp(address) => address.ip().to_canonical().is_loopback(),
            Origin::Local => true,
        }
    }
}

/// The outcome of a step of an exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Bytes to send the client, whose answer is the next step's input.
    Challenge(Vec<u8>),
    /// Proven: the session's authorization identifier.
    Accept(String),
    /// Not proven: why, for the client (SQLSTATE 28000). It should not say
    /// which part failed, so as not to help a guesser.
    Refuse(String),
}

/// \[DESIGN\] One client's exchange with an `Authenticator`.
///
/// Contract (RULES.md R-SAFE-3): `step` is given the client's first bytes,
/// then its answer to each `Challenge`; it returns `Accept` or `Refuse` in
/// a bounded number of steps.
pub trait Exchange: Send {
    /// The next step, given the client's bytes `input`.
    fn step(&mut self, input: &[u8]) -> Step;
}

/// \[DESIGN\] A mechanism of authentication: an extension point (RULES.md
/// D42).
///
/// Contract (RULES.md R-SAFE-3): `mechanism` is its name, which clients
/// ask for, unique in a server; `begin` starts the exchange of `user`
/// connecting from `peer`, whatever the user, so a refusal comes only at
/// a step, as for a wrong proof, and does not tell who exists.
pub trait Authenticator: Send + Sync {
    /// The mechanism's name.
    fn mechanism(&self) -> &'static str;
    /// Begins the exchange of `user`, connected from `peer`.
    fn begin(&self, user: &str, peer: &Peer) -> Box<dyn Exchange>;
}

/// \[DESIGN\] The mechanism `trust`: a client on the server's own machine
/// is who it says it is, as on a machine its operator controls; from any
/// other it is refused. It has nothing to prove, so it suits a server
/// reached only by its own machine's programs.
#[derive(Debug, Clone, Copy, Default)]
pub struct Trust;

/// The name of `Trust`'s mechanism.
pub const TRUST: &str = "trust";

impl Authenticator for Trust {
    fn mechanism(&self) -> &'static str {
        TRUST
    }

    fn begin(&self, user: &str, peer: &Peer) -> Box<dyn Exchange> {
        Box::new(Trusted { user: user.to_owned(), local: peer.is_local() })
    }
}

/// An exchange of `Trust`.
struct Trusted {
    user: String,
    local: bool,
}

impl Exchange for Trusted {
    fn step(&mut self, _: &[u8]) -> Step {
        if !self.local {
            return Step::Refuse("[DESIGN] trust is taken only from this machine".into());
        }
        if self.user.is_empty() {
            return Step::Refuse("[DESIGN] no user is named".into());
        }
        Step::Accept(self.user.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D42: a peer is local at a loopback
    /// address, IPv4 or IPv6, an IPv4 one written as IPv6 too, and on a
    /// Unix socket or a named pipe; not at any other address. A Unix socket
    /// or a named pipe is secure; a TCP connection is not until TLS secures
    /// it. A peer is named by its
    /// address, or `local`.
    #[test]
    fn a_peer_is_local_on_loopback_a_unix_socket_or_a_named_pipe() {
        for (address, local) in [
            ("127.0.0.1:1", true),
            ("[::1]:1", true),
            ("[::ffff:127.0.0.1]:1", true),
            ("10.0.0.1:1", false),
            ("[::ffff:10.0.0.1]:1", false),
        ] {
            assert_eq!(Peer::tcp(address.parse().unwrap()).is_local(), local, "{address}");
        }
        assert!(Peer::local().is_local() && Peer::local().secure);
        assert!(!Peer::tcp("127.0.0.1:1".parse().unwrap()).secure, "not secured yet");
        assert_eq!((Peer::tcp("[::1]:9".parse().unwrap()).name(), Peer::local().name()), ("[::1]:9".to_owned(), "local".to_owned()));
    }

    /// Verifies: [DESIGN] RULES.md D42: trust accepts a named user from
    /// this machine as who it says, in one step; refuses one from another,
    /// and an unnamed user.
    #[test]
    fn trust_takes_only_named_users_of_this_machine() {
        let here = Peer::tcp("127.0.0.1:9".parse().unwrap());
        assert_eq!(Trust.begin("X", &here).step(b""), Step::Accept("X".into()));
        assert_eq!(Trust.begin("X", &Peer::local()).step(b"ignored"), Step::Accept("X".into()));
        let away = Peer::tcp("192.0.2.1:9".parse().unwrap());
        assert!(matches!(Trust.begin("X", &away).step(b""), Step::Refuse(_)));
        assert!(matches!(Trust.begin("", &here).step(b""), Step::Refuse(_)));
        assert_eq!(Trust.mechanism(), TRUST);
    }
}
