//! \[DESIGN\] The client's side of SCRAM-SHA-256 (RFC 5802 §3, §7): its
//! first message, its proof answering the server's first, and the server's
//! signature checked, so the client knows the server knew the verifier.

use std::num::NonZeroU32;

use aws_lc_rs::constant_time;
use turborel::error::Result;

use super::{GS2_NONE, Key, Keys, attribute, decode, encode, hmac, is_nonce, refused, salted_password, saslname, text, xor};

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
/// `password`.
#[derive(Clone)]
pub struct Client {
    user: String,
    password: String,
    stage: Stage,
}

impl std::fmt::Debug for Client {
    /// Never the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Client({}, ..)", self.user)
    }
}

impl Client {
    /// An exchange as `user`, by `password`.
    pub fn new(user: &str, password: &str) -> Client {
        Client { user: user.to_owned(), password: password.to_owned(), stage: Stage::Begun }
    }

    /// client-first, of `nonce`, which should be new (`scram::nonce`):
    /// the exchange begun again.
    pub fn first(&mut self, nonce: &str) -> Vec<u8> {
        let bare = format!("n={},r={nonce}", saslname(&self.user));
        let message = format!("{GS2_NONE}{bare}");
        self.stage = Stage::Sent { bare, nonce: nonce.to_owned() };
        message.into_bytes()
    }

    /// client-final, answering `server_first`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 28000 if it is not the server-first of this exchange: out
    /// of turn, malformed, or its nonce not begun with the client's.
    pub fn answer(&mut self, server_first: &[u8]) -> Result<Vec<u8>> {
        let Stage::Sent { bare, nonce } = &self.stage else { return Err(refused("the server's first message came out of turn")) };
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
        let salted = salted_password(&self.password, &salt, iterations);
        let client_key = hmac(&salted, super::CLIENT_KEY);
        let keys = Keys::of(&salted);
        let without_proof = format!("c={},r={both}", encode(GS2_NONE.as_bytes()));
        let auth = format!("{bare},{message},{without_proof}");
        let proof = xor(&client_key, &hmac(&keys.stored, auth.as_bytes()));
        self.stage = Stage::Proved(hmac(&keys.server, auth.as_bytes()));
        Ok(format!("{without_proof},p={}", encode(&proof)).into_bytes())
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
}
