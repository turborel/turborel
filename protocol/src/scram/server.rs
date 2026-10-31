//! \[DESIGN\] The server's side of SCRAM-SHA-256 (RFC 5802 §3, §7): the
//! client's first message read, the server's sent, and the client's proof
//! checked against StoredKey, the server's signature its answer.

use std::num::NonZeroU32;

use aws_lc_rs::constant_time;
use turborel::error::Result;

use super::{GS2_NONE, GS2_UNOFFERED, Key, Keys, attribute, decode, encode, hmac, is_nonce, refused, sha256, text, xor};

/// The client's first message, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct First {
    /// Its GS2 header: `n,,` or `y,,`.
    gs2: &'static str,
    /// The message without it.
    bare: String,
    /// The client's nonce.
    nonce: String,
}

impl First {
    /// `client_first`, read.
    ///
    /// # Errors
    ///
    /// SQLSTATE 28000 if it is malformed: not UTF-8, its header asking for
    /// channel binding or another identity, its user not a saslname, its
    /// nonce not printable, or with attributes beyond the user and nonce.
    pub fn read(client_first: &[u8]) -> Result<First> {
        let message = text(client_first)?;
        let (gs2, bare) = match (message.strip_prefix(GS2_NONE), message.strip_prefix(GS2_UNOFFERED)) {
            (Some(bare), _) => (GS2_NONE, bare),
            (None, Some(bare)) => (GS2_UNOFFERED, bare),
            (None, None) => return Err(refused("the client asks for channel binding or another identity, which this server has not")),
        };
        let mut fields = bare.split(',');
        let (user, nonce) = (attribute(fields.next(), 'n')?, attribute(fields.next(), 'r')?);
        if fields.next().is_some() {
            return Err(refused("the client's first message has more than a user and a nonce"));
        }
        if !is_saslname(user) || !is_nonce(nonce) {
            return Err(refused("the client's user or nonce is malformed"));
        }
        Ok(First { gs2, bare: bare.to_owned(), nonce: nonce.to_owned() })
    }
}

/// Whether `name` is a saslname [RFC 5802 §7]: a `=` only in `=2C` or
/// `=3D`. An empty one is taken, the protocol's Hello naming the user.
fn is_saslname(name: &str) -> bool {
    name.split('=').skip(1).all(|after| after.starts_with("2C") || after.starts_with("3D"))
}

/// \[DESIGN\] A server's exchange of SCRAM-SHA-256, its first message sent.
#[derive(Clone)]
pub struct Server {
    first: First,
    /// Its first message.
    message: String,
    /// Both nonces, the client's then the server's.
    nonce: String,
    keys: Keys,
}

impl std::fmt::Debug for Server {
    /// Never the keys.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Server({}, ..)", self.message)
    }
}

impl Server {
    /// The exchange of `first`, its server-first made of `nonce`, the
    /// server's, which should be new (`scram::nonce`), and the user's
    /// `salt`, `iterations` and `keys`.
    pub fn begin(first: First, nonce: &str, (salt, iterations): (&[u8], NonZeroU32), keys: Keys) -> Server {
        let both = format!("{}{nonce}", first.nonce);
        let server_first = format!("r={both},s={},i={iterations}", encode(salt));
        Server { first, message: server_first, nonce: both, keys }
    }

    /// server-first.
    pub fn first(&self) -> Vec<u8> {
        self.message.clone().into_bytes()
    }

    /// server-final, once `client_final` proves the client knows the
    /// password.
    ///
    /// # Errors
    ///
    /// SQLSTATE 28000 if it is malformed, of another header or nonce, as a
    /// message replayed from another exchange is, or its proof is wrong.
    pub fn finish(&self, client_final: &[u8]) -> Result<Vec<u8>> {
        let message = text(client_final)?;
        let (without_proof, proof) = message.rsplit_once(",p=").ok_or_else(|| refused("the client's final message has no proof"))?;
        let mut fields = without_proof.split(',');
        let (binding, nonce) = (attribute(fields.next(), 'c')?, attribute(fields.next(), 'r')?);
        if fields.next().is_some() {
            return Err(refused("the client's final message has more than it should"));
        }
        if decode(binding).as_deref() != Some(self.first.gs2.as_bytes()) {
            return Err(refused("the client's channel binding is not its first message's"));
        }
        if nonce != self.nonce {
            return Err(refused("the nonce is not this exchange's"));
        }
        let proof: Key = decode(proof).and_then(|p| p.try_into().ok()).ok_or_else(|| refused("the proof is malformed"))?;
        let auth = format!("{},{},{without_proof}", self.first.bare, self.message);
        let client_key = xor(&proof, &hmac(&self.keys.stored, auth.as_bytes()));
        constant_time::verify_slices_are_equal(&sha256(&client_key), &self.keys.stored).map_err(|_| refused("the proof is wrong"))?;
        Ok(format!("v={}", encode(&hmac(&self.keys.server, auth.as_bytes()))).into_bytes())
    }
}
