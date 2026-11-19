//! \[DESIGN\] The client's messages of SCRAM-SHA-256 (RFC 5802 §3, §7),
//! proved from the keys a password derives rather than the password, so
//! the keys may come from the key cache (`cache.rs`, RULES.md D205)
//! instead of the slow derivation. HMAC-SHA-256 is made here of the
//! protocol's SHA-256 [RFC 2104 §2]; the messages are those the protocol's
//! own client makes (`turborel_protocol::scram::Client`), whose server
//! side checks them.

use std::num::NonZeroU32;

use turborel::error::{Code, SqlError, SqlState};
use turborel::plugin::package::crypto::PackageCrypto;
use turborel_protocol::Failure;
use turborel_protocol::packages::Crypto;
use turborel_protocol::scram::{Channel, KEY_BYTES, Key, decode, encode, salted_password};

use crate::connection::Result;

/// The bytes of a block of SHA-256, which HMAC pads its key to [RFC 2104
/// §2, B = 64].
const BLOCK: usize = 64;
/// The inner pad of HMAC [RFC 2104 §2].
const IPAD: u8 = 0x36;
/// The outer pad of HMAC [RFC 2104 §2].
const OPAD: u8 = 0x5c;
/// What ClientKey is the HMAC of [RFC 5802 §3].
const CLIENT_KEY: &[u8] = b"Client Key";
/// What ServerKey is the HMAC of [RFC 5802 §3].
const SERVER_KEY: &[u8] = b"Server Key";
/// The GS2 headers of the channel bindings [RFC 5802 §7, RFC 5929 §4.2,
/// RFC 9266 §2], as the protocol's client writes them.
const GS2_NONE: &str = "n,,";
/// See `GS2_NONE`.
const GS2_UNOFFERED: &str = "y,,";
/// See `GS2_NONE`.
const GS2_END_POINT: &str = "p=tls-server-end-point,,";
/// See `GS2_NONE`.
const GS2_EXPORTER: &str = "p=tls-exporter,,";
/// The lowest and highest byte of a nonce: printable ASCII [RFC 5802 §7].
const PRINTABLE: std::ops::RangeInclusive<u8> = 0x21..=0x7e;

/// HMAC-SHA-256 of `data` under `key` [RFC 2104 §2].
pub(crate) fn hmac(key: &[u8], data: &[u8]) -> Key {
    let mut block = [0_u8; BLOCK];
    let hashed;
    let key = if key.len() > BLOCK {
        hashed = Crypto.sha256(key);
        hashed.as_slice()
    } else {
        key
    };
    block.iter_mut().zip(key).for_each(|(b, k)| *b = *k);
    let inner: Vec<u8> = block.iter().map(|b| b ^ IPAD).chain(data.iter().copied()).collect();
    let outer: Vec<u8> = block.iter().map(|b| b ^ OPAD).chain(Crypto.sha256(&inner)).collect();
    Crypto.sha256(&outer)
}

/// What a password derives under a salt and iterations: ClientKey and
/// ServerKey [RFC 5802 §3]. Never the password, nor SaltedPassword.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Derived {
    pub(crate) client: Key,
    pub(crate) server: Key,
}

impl std::fmt::Debug for Derived {
    /// Never the keys.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Derived(..)")
    }
}

/// The keys `password` derives under `salt` and `iterations`: the slow
/// step, PBKDF2's iterations.
pub(crate) fn derive(password: &str, salt: &[u8], iterations: NonZeroU32) -> Derived {
    let salted = salted_password(password, salt, iterations);
    Derived { client: hmac(&salted, CLIENT_KEY), server: hmac(&salted, SERVER_KEY) }
}

/// A refusal of the exchange: credentials refused (SQLSTATE 28000).
pub(crate) fn refused(why: &str) -> Failure {
    let e = SqlError::new(Code::Connection, format!("[DESIGN] SCRAM: {why}")).with_state(SqlState::INVALID_AUTHORIZATION);
    Failure::from(&e)
}

/// The GS2 header of `channel`.
fn header(channel: &Channel) -> &'static str {
    match channel {
        Channel::Unbound => GS2_NONE,
        Channel::Unoffered => GS2_UNOFFERED,
        Channel::EndPoint(_) => GS2_END_POINT,
        Channel::Exporter(_) => GS2_EXPORTER,
    }
}

/// cbind-input of `channel`: its header, then the data it binds [RFC
/// 5802 §7].
fn input(channel: &Channel) -> Vec<u8> {
    let mut bytes = header(channel).as_bytes().to_vec();
    match channel {
        Channel::EndPoint(data) | Channel::Exporter(data) => bytes.extend_from_slice(data),
        Channel::Unbound | Channel::Unoffered => {}
    }
    bytes
}

/// The server's first message, read: its whole text, both nonces, the
/// salt and the iterations.
pub(crate) struct ServerFirst {
    message: String,
    both: String,
    pub(crate) salt: Vec<u8>,
    pub(crate) iterations: NonZeroU32,
}

/// How far the client has come.
#[derive(Clone)]
enum Stage {
    Begun,
    Sent { bare: String, nonce: String },
    Proved(Key),
    Verified,
}

/// A client's exchange, its channel bound as `Channel` says.
#[derive(Clone)]
pub(crate) struct Exchange {
    channel: Channel,
    stage: Stage,
}

/// The value of the attribute `name` that `field` is, as `name=value`.
fn attribute(field: Option<&str>, name: char) -> Result<&str> {
    let text = field.ok_or_else(|| refused(&format!("the attribute {name} is missing")))?;
    text.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix('='))
        .ok_or_else(|| refused(&format!("the attribute {name} was expected, not {text}")))
}

/// Whether `both` is a nonce extending the client's `nonce` [RFC 5802 §7].
fn extends(both: &str, nonce: &str) -> bool {
    let printable = both.bytes().all(|b| PRINTABLE.contains(&b) && b != b',');
    printable && both.len() > nonce.len() && both.starts_with(nonce)
}

impl Exchange {
    /// An exchange bound to `channel`.
    pub(crate) fn new(channel: Channel) -> Exchange {
        Exchange { channel, stage: Stage::Begun }
    }

    /// client-first, of `nonce`, the user left to Hello.
    pub(crate) fn first(&mut self, nonce: &str) -> Vec<u8> {
        let bare = format!("n=,r={nonce}");
        let message = format!("{}{bare}", header(&self.channel));
        self.stage = Stage::Sent { bare, nonce: nonce.to_owned() };
        message.into_bytes()
    }

    /// `server_first`, read and checked against this exchange.
    ///
    /// # Errors
    ///
    /// SQLSTATE 28000 if it is out of turn, malformed, or its nonce does
    /// not extend the client's.
    pub(crate) fn read(&self, server_first: &[u8]) -> Result<ServerFirst> {
        let Stage::Sent { nonce, .. } = &self.stage else { return Err(refused("the server's first message came out of turn")) };
        let message = std::str::from_utf8(server_first).map_err(|_| refused("a message is not UTF-8"))?;
        let mut fields = message.split(',');
        let (both, salt, iterations) = (attribute(fields.next(), 'r')?, attribute(fields.next(), 's')?, attribute(fields.next(), 'i')?);
        if fields.next().is_some() {
            return Err(refused("the server's first message has more than it should"));
        }
        if !extends(both, nonce) {
            return Err(refused("the server's nonce does not extend the client's"));
        }
        let salt = decode(salt).ok_or_else(|| refused("the salt is not base 64"))?;
        let iterations = iterations.parse().map_err(|_| refused("the iterations are no positive number"))?;
        Ok(ServerFirst { message: message.to_owned(), both: both.to_owned(), salt, iterations })
    }

    /// client-final, answering `first` by `keys`.
    pub(crate) fn prove(&mut self, first: &ServerFirst, keys: &Derived) -> Vec<u8> {
        let bare = match &self.stage {
            Stage::Sent { bare, .. } => bare.clone(),
            Stage::Begun | Stage::Proved(_) | Stage::Verified => String::new(),
        };
        let stored = Crypto.sha256(&keys.client);
        let without_proof = format!("c={},r={}", encode(&input(&self.channel)), first.both);
        let auth = format!("{bare},{},{without_proof}", first.message);
        let signature = hmac(&stored, auth.as_bytes());
        let proof: [u8; KEY_BYTES] =
            std::array::from_fn(|i| keys.client.get(i).copied().unwrap_or(0) ^ signature.get(i).copied().unwrap_or(0));
        self.stage = Stage::Proved(hmac(&keys.server, auth.as_bytes()));
        format!("{without_proof},p={}", encode(&proof)).into_bytes()
    }

    /// Checks `server_final`, the server's signature.
    ///
    /// # Errors
    ///
    /// SQLSTATE 28000 if it is out of turn, the server's error, malformed,
    /// or not the signature expected.
    pub(crate) fn verify(&mut self, server_final: &[u8]) -> Result<()> {
        let Stage::Proved(expected) = &self.stage else { return Err(refused("the server's final message came out of turn")) };
        let message = std::str::from_utf8(server_final).map_err(|_| refused("a message is not UTF-8"))?;
        if let Some(error) = message.strip_prefix("e=") {
            return Err(refused(&format!("the server refused: {error}")));
        }
        let signature = decode(attribute(Some(message), 'v')?).ok_or_else(|| refused("the server's signature is not base 64"))?;
        // Compared whole, every byte, whatever the first that differs.
        let differs = signature.len() != expected.len() || signature.iter().zip(expected).fold(0_u8, |d, (a, b)| d | (a ^ b)) != 0;
        if differs {
            return Err(refused("the server did not prove it knows the user"));
        }
        self.stage = Stage::Verified;
        Ok(())
    }

    /// Whether the server proved it knew the user's verifier.
    pub(crate) fn verified(&self) -> bool {
        matches!(self.stage, Stage::Verified)
    }

    /// Whether the exchange is bound to its channel.
    pub(crate) fn bound(&self) -> bool {
        matches!(self.channel, Channel::EndPoint(_) | Channel::Exporter(_))
    }
}
