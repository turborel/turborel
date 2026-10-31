//! \[DESIGN\] The client's side of `scram-sha-256` and
//! `scram-sha-256-plus` (RULES.md D43; RFC 5802, RFC 7677), with the
//! feature `scram`: the client proves it knows the password, never sending
//! it, and takes the server only once it has proved it knows the user's
//! verifier. Over TLS it binds the exchange to the connection: over TLS
//! 1.3 by `tls-exporter` (RFC 9266), the keying material the connection
//! exports, else by `tls-server-end-point` (RFC 5929 §4), the hash of the
//! certificate the server presented; so a server in the middle cannot
//! relay it. The
//! messages and keys are the protocol's (`turborel_protocol::scram`), as
//! the server's are.

use turborel_protocol::Failure;
use turborel_protocol::scram::{self, Channel, Client};

use crate::connection::{Mechanism, Result, TlsChannel};

/// \[DESIGN\] The client's side of `scram-sha-256`, of a user's password:
/// what a client proves a password by, unless the server takes only
/// `password`.
///
/// Over TLS (`Mechanism::bind`) it is `scram-sha-256-plus`: over TLS 1.3
/// bound by `tls-exporter`, which RFC 9266 §3 makes the binding of TLS
/// 1.3; else by `tls-server-end-point`, to the certificate the server
/// presented. Over TLS 1.2, of a certificate whose signature defines no
/// binding [RFC 5929 §4.1], it is `scram-sha-256` with the header `y` [RFC
/// 5802 §6], which a server that does bind refuses, as a server in the
/// middle would make it. Over a Unix socket or a named pipe, or plain
/// TCP, it binds nothing.
///
/// Its exchange overheard lets the eavesdropper guess the password at
/// leisure, so over plain TCP it is refused before anything is sent,
/// unless the program says otherwise (`in_the_clear`), as a server does
/// whose SERVER.SCRAM_WITHOUT_TLS is ON.
#[derive(Clone)]
pub struct Scram {
    password: String,
    exchange: Option<Client>,
    clear: bool,
    /// The channel binding of the connection about to carry it, as
    /// `bind` set it; taken by `initial`, so the next connection binds
    /// afresh.
    channel: Channel,
}

impl std::fmt::Debug for Scram {
    /// Never the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Scram(..)")
    }
}

impl Scram {
    /// The proof of `password`.
    pub fn new(password: &str) -> Scram {
        Scram { password: password.to_owned(), exchange: None, clear: false, channel: Channel::Unbound }
    }

    /// It, sent over plain TCP too: for a server whose
    /// SERVER.SCRAM_WITHOUT_TLS is ON, on a network the program trusts.
    #[must_use]
    pub fn in_the_clear(self) -> Scram {
        Scram { clear: true, ..self }
    }
}

impl Mechanism for Scram {
    /// `scram-sha-256-plus` once bound to a connection or a certificate's
    /// hash, else `scram-sha-256`.
    fn name(&self) -> &str {
        match self.channel {
            Channel::EndPoint(_) | Channel::Exporter(_) => scram::MECHANISM_PLUS,
            Channel::Unbound | Channel::Unoffered => scram::MECHANISM,
        }
    }

    /// client-first, of a new nonce, bound as `bind` said. The user is the
    /// one Hello names, so none is given here.
    fn initial(&mut self) -> Vec<u8> {
        let channel = std::mem::replace(&mut self.channel, Channel::Unbound);
        let mut client = Client::new("", &self.password).bound(channel);
        // With no randomness, no nonce: the empty one sent is refused by
        // the server as malformed.
        let first = client.first(&scram::nonce().unwrap_or_default());
        self.exchange = Some(client);
        first
    }

    /// client-final to server-first; then, to server-final, nothing, once
    /// the server's signature is right.
    fn answer(&mut self, challenge: &[u8]) -> Result<Vec<u8>> {
        let Some(client) = &mut self.exchange else { return Err(crate::connection::failed("[DESIGN] SCRAM was not begun")) };
        let refused = |e: turborel::error::SqlError| Failure::from(&e);
        if challenge.starts_with(b"v=") || challenge.starts_with(b"e=") {
            client.verify(challenge).map_err(refused)?;
            return Ok(Vec::new());
        }
        client.answer(challenge).map_err(refused)
    }

    fn secret(&self) -> bool {
        !self.clear
    }

    /// Bound to `channel`'s `tls-exporter`, over TLS 1.3; else to its
    /// certificate's `tls-server-end-point`; else, of a certificate that
    /// defines none, able to bind but offered none. Both sides name the
    /// type by the GS2 header, which the proof covers [RFC 5802 §6, §7].
    fn bind(&mut self, channel: &TlsChannel) {
        let end_point = || channel.certificate.as_deref().and_then(scram::end_point).map(Channel::EndPoint);
        self.channel = channel.exporter.clone().map(Channel::Exporter).or_else(end_point).unwrap_or(Channel::Unoffered);
    }

    fn ready(&self) -> Result<()> {
        if self.exchange.as_ref().is_some_and(Client::verified) {
            return Ok(());
        }
        Err(crate::connection::failed("[DESIGN] the server did not prove it knows the user's verifier"))
    }
}
