//! \[DESIGN\] The client's side of `scram-sha-256` and
//! `scram-sha-256-plus` (RULES.md D43; RFC 5802, RFC 7677), with the
//! feature `scram`: the client proves it knows the password, never sending
//! it, and takes the server only once it has proved it knows the user's
//! verifier. Over TLS it binds the exchange to the connection: over TLS
//! 1.3 by `tls-exporter` (RFC 9266), the keying material the connection
//! exports, else by `tls-server-end-point` (RFC 5929 §4), the hash of the
//! certificate the server presented; so a server in the middle cannot
//! relay it. The
//! exchange is the protocol's own client (`turborel_protocol::scram::Client`),
//! proving from the keys a password derives, which a `KeyCache` may keep
//! (`cache.rs`, RULES.md D205), so a connection made again skips their
//! slow derivation. Whether it binds, its `ChannelBinding` says
//! (RULES.md D204).

mod cache;

use std::num::NonZeroU32;

use turborel::error::{Code, SqlError, SqlState};
use turborel_protocol::Failure;
use turborel_protocol::scram::{self, Channel, Client, Derived};

pub use cache::{KEY_CACHE_ENTRIES, KeyCache};

use crate::connection::{Mechanism, Result, TlsChannel};
use crate::settings::ChannelBinding;

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
/// TCP, it binds nothing. `ChannelBinding::Disable` binds nothing
/// anywhere; `ChannelBinding::Require` refuses to go on unbound.
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
    binding: ChannelBinding,
    cache: Option<KeyCache>,
    /// The salt and iterations of the keys the exchange used, to forget
    /// them if it fails.
    used: Option<(Vec<u8>, NonZeroU32)>,
}

/// A refusal of the exchange: credentials refused (SQLSTATE 28000).
fn refused(why: &str) -> Failure {
    let e = SqlError::new(Code::Connection, format!("[DESIGN] SCRAM: {why}")).with_state(SqlState::INVALID_AUTHORIZATION);
    Failure::from(&e)
}

impl std::fmt::Debug for Scram {
    /// Never the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Scram(..)")
    }
}

impl Scram {
    /// The proof of `password`, binding its channel where it can
    /// (`ChannelBinding::Prefer`), its keys derived at each login.
    pub fn new(password: &str) -> Scram {
        Scram {
            password: password.to_owned(),
            exchange: None,
            clear: false,
            channel: Channel::Unbound,
            binding: ChannelBinding::Prefer,
            cache: None,
            used: None,
        }
    }

    /// It, sent over plain TCP too: for a server whose
    /// SERVER.SCRAM_WITHOUT_TLS is ON, on a network the program trusts.
    #[must_use]
    pub fn in_the_clear(self) -> Scram {
        Scram { clear: true, ..self }
    }

    /// It, binding its channel as `binding` says.
    #[must_use]
    pub fn channel_binding(self, binding: ChannelBinding) -> Scram {
        Scram { binding, ..self }
    }

    /// It, its keys kept in `cache` and taken from it, which its clones
    /// share.
    #[must_use]
    pub fn with_cache(self, cache: &KeyCache) -> Scram {
        Scram { cache: Some(cache.clone()), ..self }
    }

    /// The keys the exchange forgets, if it used the cache's.
    fn forget(&mut self) {
        if let (Some(cache), Some((salt, iterations))) = (&self.cache, self.used.take()) {
            cache.forget(&self.password, &salt, iterations);
        }
    }

    /// client-final, answering `challenge`, server-first.
    fn prove(&mut self, challenge: &[u8]) -> Result<Vec<u8>> {
        let Some(exchange) = &mut self.exchange else { return Err(crate::connection::failed("[DESIGN] SCRAM was not begun")) };
        if self.binding == ChannelBinding::Require && !exchange.is_bound() {
            return Err(refused("channel binding is required, and this connection is not bound"));
        }
        let first = exchange.read(challenge).map_err(|e| Failure::from(&e))?;
        let keys = match &self.cache {
            Some(cache) => cache.derived(&self.password, &first.salt, first.iterations),
            None => Derived::of(&self.password, &first.salt, first.iterations),
        };
        self.used = Some((first.salt.clone(), first.iterations));
        Ok(exchange.prove(&first, &keys))
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
        // The user is Hello's, and the proof is made from derived keys, so
        // the exchange holds neither.
        let mut exchange = Client::new("", "").bound(channel);
        // With no randomness, no nonce: the empty one sent is refused by
        // the server as malformed.
        let first = exchange.first(&scram::nonce().unwrap_or_default());
        self.exchange = Some(exchange);
        self.used = None;
        first
    }

    /// client-final to server-first; then, to server-final, nothing, once
    /// the server's signature is right. A failure forgets the keys the
    /// cache gave.
    fn answer(&mut self, challenge: &[u8]) -> Result<Vec<u8>> {
        let answered = if challenge.starts_with(b"v=") || challenge.starts_with(b"e=") {
            let verified = self.exchange.as_mut().map(|e| e.verify(challenge).map_err(|e| Failure::from(&e)));
            verified.unwrap_or_else(|| Err(crate::connection::failed("[DESIGN] SCRAM was not begun"))).map(|()| Vec::new())
        } else {
            self.prove(challenge)
        };
        if answered.is_err() {
            self.forget();
        }
        answered
    }

    fn secret(&self) -> bool {
        !self.clear
    }

    /// Bound to `channel`'s `tls-exporter`, over TLS 1.3; else to its
    /// certificate's `tls-server-end-point`; else, of a certificate that
    /// defines none, able to bind but offered none. Both sides name the
    /// type by the GS2 header, which the proof covers [RFC 5802 §6, §7].
    /// Under `ChannelBinding::Disable`, nothing.
    fn bind(&mut self, channel: &TlsChannel) {
        if self.binding == ChannelBinding::Disable {
            return;
        }
        let end_point = || channel.certificate.as_deref().and_then(scram::end_point).map(Channel::EndPoint);
        self.channel = channel.exporter.clone().map(Channel::Exporter).or_else(end_point).unwrap_or(Channel::Unoffered);
    }

    fn ready(&self) -> Result<()> {
        if self.exchange.as_ref().is_some_and(Client::verified) {
            return Ok(());
        }
        Err(crate::connection::failed("[DESIGN] the server did not prove it knows the user's verifier"))
    }

    fn refused(&mut self) {
        self.forget();
    }
}

#[cfg(test)]
mod tests;
