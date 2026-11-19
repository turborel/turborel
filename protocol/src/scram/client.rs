//! \[DESIGN\] The client's side of SCRAM-SHA-256 and SCRAM-SHA-256-PLUS
//! (RFC 5802 §3, §6, §7): its first message, its proof answering the
//! server's first, bound to the channel its `Channel` says, and the
//! server's signature checked, so the client knows the server knew the
//! verifier. Its proof is made from the keys a password derives
//! (`Derived`), so a client that keeps them (a key cache) proves without
//! the slow derivation, by `read` and `prove`; `answer` derives them.

use std::num::NonZeroU32;

use aws_lc_rs::constant_time;
use turborel::error::Result;

use super::{Channel, Key, attribute, decode, encode, hmac, is_nonce, refused, salted_password, saslname, sha256, text, xor};

/// What a password derives under a salt and iterations: ClientKey and
/// ServerKey [RFC 5802 §3]. Never the password, nor SaltedPassword.
#[derive(Clone, PartialEq, Eq)]
pub struct Derived {
    /// ClientKey.
    pub client: Key,
    /// ServerKey.
    pub server: Key,
}

impl std::fmt::Debug for Derived {
    /// Never the keys.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Derived(..)")
    }
}

impl Derived {
    /// The keys `password` derives under `salt` and `iterations`: the
    /// slow step, PBKDF2's iterations.
    pub fn of(password: &str, salt: &[u8], iterations: NonZeroU32) -> Derived {
        let salted = salted_password(password, salt, iterations);
        Derived { client: hmac(&salted, super::CLIENT_KEY), server: hmac(&salted, super::SERVER_KEY) }
    }
}

/// The server's first message, read and checked: its whole text, both
/// nonces, and the salt and iterations the keys are derived under.
#[derive(Debug, Clone)]
pub struct ServerFirst {
    message: String,
    both: String,
    /// The salt.
    pub salt: Vec<u8>,
    /// The iterations.
    pub iterations: NonZeroU32,
}

/// How far the client has come.
#[derive(Clone)]
enum Stage {
    /// Its first message not yet made.
    Begun,
    /// Its first message sent, `bare` without the GS2 header, of `nonce`.
    Sent { bare: String, nonce: String },
    /// Its proof sent: the server's signature it expects.
    Proved(Key),
    /// The server's signature checked.
    Verified,
}

/// \[DESIGN\] A client's exchange of SCRAM-SHA-256 as `user`, by
/// `password`; of SCRAM-SHA-256-PLUS, its channel bound (`bound`).
#[derive(Clone)]
pub struct Client {
    user: String,
    password: String,
    channel: Channel,
    stage: Stage,
}

impl std::fmt::Debug for Client {
    /// Never the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Client({}, ..)", self.user)
    }
}

impl Client {
    /// An exchange as `user`, by `password`, binding no channel.
    pub fn new(user: &str, password: &str) -> Client {
        Client { user: user.to_owned(), password: password.to_owned(), channel: Channel::Unbound, stage: Stage::Begun }
    }

    /// It, of the channel binding `channel` [RFC 5802 §6]: for
    /// SCRAM-SHA-256-PLUS, `Channel::Exporter` over TLS 1.3, of the keying
    /// material the connection exports [RFC 9266 §2], or
    /// `Channel::EndPoint` over TLS, of the hash of the certificate the
    /// server presented (`end_point`).
    #[must_use]
    pub fn bound(self, channel: Channel) -> Client {
        Client { channel, ..self }
    }

    /// client-first, of `nonce`, which should be new (`scram::nonce`):
    /// the exchange begun again.
    pub fn first(&mut self, nonce: &str) -> Vec<u8> {
        let bare = format!("n={},r={nonce}", saslname(&self.user));
        let message = format!("{}{bare}", self.channel.header());
        self.stage = Stage::Sent { bare, nonce: nonce.to_owned() };
        message.into_bytes()
    }

    /// client-final, answering `server_first` by the keys its password
    /// derives.
    ///
    /// # Errors
    ///
    /// As `read`'s.
    pub fn answer(&mut self, server_first: &[u8]) -> Result<Vec<u8>> {
        let first = self.read(server_first)?;
        let keys = Derived::of(&self.password, &first.salt, first.iterations);
        Ok(self.prove(&first, &keys))
    }

    /// `server_first`, read and checked against this exchange.
    ///
    /// # Errors
    ///
    /// SQLSTATE 28000 if it is not the server-first of this exchange: out
    /// of turn, malformed, or its nonce not begun with the client's.
    pub fn read(&self, server_first: &[u8]) -> Result<ServerFirst> {
        let Stage::Sent { nonce, .. } = &self.stage else { return Err(refused("the server's first message came out of turn")) };
        let message = text(server_first)?;
        let mut fields = message.split(',');
        let (both, salt, iterations) = (attribute(fields.next(), 'r')?, attribute(fields.next(), 's')?, attribute(fields.next(), 'i')?);
        if fields.next().is_some() {
            return Err(refused("the server's first message has more than it should"));
        }
        if !is_nonce(both) || both.len() <= nonce.len() || !both.starts_with(nonce.as_str()) {
            return Err(refused("the server's nonce does not extend the client's"));
        }
        let salt = decode(salt).ok_or_else(|| refused("the salt is not base 64"))?;
        let iterations: NonZeroU32 = iterations.parse().map_err(|_| refused("the iterations are no positive number"))?;
        Ok(ServerFirst { message: message.to_owned(), both: both.to_owned(), salt, iterations })
    }

    /// client-final, answering `first`, which `read` gave, by `keys`, as
    /// its salt and iterations derive them: the server's signature then
    /// expected.
    pub fn prove(&mut self, first: &ServerFirst, keys: &Derived) -> Vec<u8> {
        let bare = match &self.stage {
            Stage::Sent { bare, .. } => bare.clone(),
            Stage::Begun | Stage::Proved(_) | Stage::Verified => String::new(),
        };
        let without_proof = format!("c={},r={}", encode(&self.channel.input()), first.both);
        let auth = format!("{bare},{},{without_proof}", first.message);
        let proof = xor(&keys.client, &hmac(&sha256(&keys.client), auth.as_bytes()));
        self.stage = Stage::Proved(hmac(&keys.server, auth.as_bytes()));
        format!("{without_proof},p={}", encode(&proof)).into_bytes()
    }

    /// Checks `server_final`: the server's signature, proving it knew the
    /// user's verifier.
    ///
    /// # Errors
    ///
    /// SQLSTATE 28000 if it is out of turn, malformed, an error of the
    /// server's, or not the signature expected.
    pub fn verify(&mut self, server_final: &[u8]) -> Result<()> {
        let Stage::Proved(expected) = &self.stage else { return Err(refused("the server's final message came out of turn")) };
        let message = text(server_final)?;
        if let Some(error) = message.strip_prefix("e=") {
            return Err(refused(&format!("the server refused: {error}")));
        }
        let signature = decode(attribute(Some(message), 'v')?).ok_or_else(|| refused("the server's signature is not base 64"))?;
        constant_time::verify_slices_are_equal(&signature, expected).map_err(|_| refused("the server did not prove it knows the user"))?;
        self.stage = Stage::Verified;
        Ok(())
    }

    /// Whether the server proved it knew the user's verifier.
    pub fn verified(&self) -> bool {
        matches!(self.stage, Stage::Verified)
    }

    /// Whether the exchange is bound to its channel.
    pub fn is_bound(&self) -> bool {
        matches!(self.channel, Channel::EndPoint(_) | Channel::Exporter(_))
    }
}
