//! \[DESIGN\] SCRAM-SHA-256 and SCRAM-SHA-256-PLUS (RFC 5802, RFC 7677;
//! RULES.md D43), with the feature `scram`: a challenge and response by
//! which a client proves it knows a user's password, and the server that
//! it knows the user's verifier, the password never sent. Both sides'
//! messages and keys are here, so the client and the server share one
//! implementation; the cryptography is aws-lc-rs's (D26).
//!
//! ```text
//! client-first:  <GS2 header>n=<user>,r=<client nonce>
//! server-first:  r=<client nonce><server nonce>,s=<salt, base 64>,i=<iterations>
//! client-final:  c=<GS2 header, then the data bound, base 64>,r=<both nonces>,
//!                p=<ClientProof, base 64>
//! server-final:  v=<ServerSignature, base 64>
//!
//! SaltedPassword  = PBKDF2-HMAC-SHA-256(password, salt, iterations)
//! ClientKey       = HMAC(SaltedPassword, "Client Key")
//! StoredKey       = SHA-256(ClientKey)
//! ServerKey       = HMAC(SaltedPassword, "Server Key")
//! AuthMessage     = client-first without its GS2 header, server-first,
//!                   client-final without its proof, by commas
//! ClientProof     = ClientKey XOR HMAC(StoredKey, AuthMessage)
//! ServerSignature = HMAC(ServerKey, AuthMessage)
//! ```
//!
//! The server keeps StoredKey and ServerKey, never SaltedPassword, which
//! would let whoever reads it log in. The channel is bound (`Channel`) by
//! one of two types, which both sides of a TLS connection know:
//! `tls-exporter` [RFC 9266 §2], keying material the connection exports
//! (`EXPORTER_LABEL`), the GS2 header `p=tls-exporter`; or
//! `tls-server-end-point` [RFC 5929 §4], the hash of the server's
//! certificate (`end_point`), the header `p=tls-server-end-point`; each of
//! SCRAM-SHA-256-PLUS. `n` is of a client binding none; `y` of one that
//! could, but sees the server offer none. Another binding type, or another
//! identity to act as, `a=`, is refused, as is a message with extensions.
//! The protocol's Hello names the user authenticated; the `n=` of
//! client-first is read and left aside, as it is part of AuthMessage. A
//! password is taken as its UTF-8
//! bytes, without SASLprep [RFC 5802 §2.2]: the same on both sides here,
//! and the same of an ASCII password as SASLprep's.

mod base64;
mod binding;

use std::num::NonZeroU32;

use aws_lc_rs::{constant_time, digest, hmac, pbkdf2};
use turborel::error::{Code, Result, SqlError, SqlState};

pub use base64::{decode, encode};
pub use binding::end_point;

/// The name of the mechanism, as a client asks for it.
pub const MECHANISM: &str = "scram-sha-256";
/// The name of the mechanism bound to its channel [RFC 5802 §4, RFC 7677
/// §3], as a client asks for it.
pub const MECHANISM_PLUS: &str = "scram-sha-256-plus";
/// The bytes of a key, a proof or a signature: SHA-256's.
pub const KEY_BYTES: usize = 32;
/// \[DESIGN\] The random bytes of a nonce: 144 bits, in 24 characters of
/// base 64, too many to guess or meet again.
pub const NONCE_BYTES: usize = 18;
/// \[DESIGN\] The GS2 header of a client without channel binding [RFC
/// 5802 §7]; its base 64 is the `c=` of client-final.
const GS2_NONE: &str = "n,,";
/// \[DESIGN\] The GS2 header of a client that could bind the channel, but
/// believes the server cannot [RFC 5802 §6].
const GS2_UNOFFERED: &str = "y,,";
/// \[DESIGN\] The GS2 header of a client binding the channel by
/// `tls-server-end-point` [RFC 5802 §7, RFC 5929 §4.2].
const GS2_END_POINT: &str = "p=tls-server-end-point,,";
/// \[DESIGN\] The GS2 header of a client binding the channel by
/// `tls-exporter` [RFC 5802 §7, RFC 9266 §2].
const GS2_EXPORTER: &str = "p=tls-exporter,,";
/// The label of the keying material `tls-exporter` binds: "the ASCII
/// string \"EXPORTER-Channel-Binding\" with no terminating NUL", its
/// context the zero-length string [RFC 9266 §2].
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-Channel-Binding";
/// The bytes of the keying material `tls-exporter` binds: "32 bytes"
/// [RFC 9266 §2].
pub const EXPORTER_BYTES: usize = 32;

/// \[DESIGN\] The channel binding of an exchange [RFC 5802 §6, §7]: the
/// flag of its GS2 header, and the data it binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channel {
    /// `n`: none, the client binding no channel, as over a Unix socket.
    Unbound,
    /// `y`: none, the client able to bind one, but the server offering
    /// none on this connection.
    Unoffered,
    /// `p=tls-server-end-point`: bound to the server's certificate, of
    /// this hash (`end_point`). Not a secret [RFC 5929 §4.2].
    EndPoint(Vec<u8>),
    /// `p=tls-exporter`: bound to the TLS connection itself, by the keying
    /// material it exports of `EXPORTER_LABEL` [RFC 9266 §2]; TLS 1.3's.
    Exporter(Vec<u8>),
}

impl Channel {
    /// Its GS2 header.
    fn header(&self) -> &'static str {
        match self {
            Channel::Unbound => GS2_NONE,
            Channel::Unoffered => GS2_UNOFFERED,
            Channel::EndPoint(_) => GS2_END_POINT,
            Channel::Exporter(_) => GS2_EXPORTER,
        }
    }

    /// cbind-input: its GS2 header, then the data it binds [RFC 5802 §7],
    /// whose base 64 is the `c=` of client-final.
    fn input(&self) -> Vec<u8> {
        let mut input = self.header().as_bytes().to_vec();
        match self {
            Channel::EndPoint(data) | Channel::Exporter(data) => input.extend_from_slice(data),
            Channel::Unbound | Channel::Unoffered => {}
        }
        input
    }
}
/// \[DESIGN\] What ClientKey is the HMAC of [RFC 5802 §3].
const CLIENT_KEY: &[u8] = b"Client Key";
/// \[DESIGN\] What ServerKey is the HMAC of [RFC 5802 §3].
const SERVER_KEY: &[u8] = b"Server Key";

/// A key, a proof or a signature.
pub type Key = [u8; KEY_BYTES];

/// A refusal of the exchange: credentials refused (SQLSTATE 28000).
fn refused(why: &str) -> SqlError {
    SqlError::new(Code::Connection, format!("[DESIGN] SCRAM: {why}")).with_state(SqlState::INVALID_AUTHORIZATION)
}

fn hmac(key: &[u8], data: &[u8]) -> Key {
    let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data);
    tag.as_ref().try_into().unwrap_or([0; KEY_BYTES])
}

fn sha256(data: &[u8]) -> Key {
    digest::digest(&digest::SHA256, data).as_ref().try_into().unwrap_or([0; KEY_BYTES])
}

fn xor(a: &Key, b: &Key) -> Key {
    std::array::from_fn(|i| a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0))
}

/// SaltedPassword: `password` hashed with `salt`, `iterations` times, as
/// Hi of RFC 5802 §2.2 is PBKDF2-HMAC-SHA-256 of one block.
pub fn salted_password(password: &str, salt: &[u8], iterations: NonZeroU32) -> Key {
    let mut salted = [0_u8; KEY_BYTES];
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, iterations, salt, password.as_bytes(), &mut salted);
    salted
}

/// What a server keeps of a password: StoredKey and ServerKey.
#[derive(Clone, PartialEq, Eq)]
pub struct Keys {
    /// StoredKey: SHA-256 of ClientKey.
    pub stored: Key,
    /// ServerKey.
    pub server: Key,
}

impl std::fmt::Debug for Keys {
    /// Never the keys.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Keys(..)")
    }
}

impl Keys {
    /// The keys of `salted`, a SaltedPassword.
    pub fn of(salted: &Key) -> Keys {
        Keys { stored: sha256(&hmac(salted, CLIENT_KEY)), server: hmac(salted, SERVER_KEY) }
    }

    /// Whether `salted` is the SaltedPassword these are of: compared in
    /// constant time.
    pub fn match_salted(&self, salted: &Key) -> bool {
        constant_time::verify_slices_are_equal(&Keys::of(salted).stored, &self.stored).is_ok()
    }
}

/// A new nonce, of the system's randomness: printable, without a comma.
///
/// # Errors
///
/// SQLCODE -915 if the system gives no randomness.
pub fn nonce() -> Result<String> {
    let mut bytes = [0_u8; NONCE_BYTES];
    aws_lc_rs::rand::fill(&mut bytes).map_err(|_| SqlError::new(Code::Io, "[DESIGN] the system gave no randomness for a nonce"))?;
    Ok(encode(&bytes))
}

/// The value of the attribute `name` that `field` is, as `name=value`.
fn attribute(field: Option<&str>, name: char) -> Result<&str> {
    let text = field.ok_or_else(|| refused(&format!("the attribute {name} is missing")))?;
    text.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix('='))
        .ok_or_else(|| refused(&format!("the attribute {name} was expected, not {text}")))
}

/// Whether `nonce` is one: printable ASCII but the comma [RFC 5802 §7].
fn is_nonce(nonce: &str) -> bool {
    !nonce.is_empty() && nonce.bytes().all(|b| (0x21..=0x7e).contains(&b) && b != b',')
}

/// `user` as a saslname: `=` and `,` written `=3D` and `=2C` [RFC 5802 §5.1].
fn saslname(user: &str) -> String {
    user.replace('=', "=3D").replace(',', "=2C")
}

/// The UTF-8 of `bytes`, as a message of the exchange.
fn text(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|_| refused("a message is not UTF-8"))
}

mod client;
mod server;

pub use client::Client;
pub use server::{First, Offer, Server};

#[cfg(test)]
mod tests;
