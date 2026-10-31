//! \[DESIGN\] The mechanisms `scram-sha-256` and `scram-sha-256-plus`
//! (RULES.md D43; RFC 5802, RFC 7677; the messages in
//! `turborel_protocol::scram`): a client proves it knows the user's
//! password, and the server that it knows the user's verifier, the password
//! never sent. The proof is checked against the StoredKey of the verifier
//! `password.rs` makes, so one verifier serves these mechanisms and
//! `password`.
//!
//! ```text
//! client: Hello, mechanism scram-sha-256(-plus), client-first
//! server: Challenge, server-first
//! client: Answer, client-final
//! server: Challenge, server-final      (the proof right)
//! client: Answer, empty                (the server's signature right)
//! server: Ready
//! ```
//!
//! The exchange does not give the password away, but one overheard lets
//! its eavesdropper try passwords against it at its own pace. So it is
//! taken over TLS, a Unix socket or a named pipe, and over plain TCP only
//! while SERVER.SCRAM_WITHOUT_TLS is ON. Over TLS, `scram-sha-256-plus`
//! binds it to the connection by `tls-server-end-point` (RFC 5929 §4), the
//! hash of the certificate this server presented (`Peer::end_point`), so a
//! server in the middle, presenting its own, cannot relay it; a client of
//! `scram-sha-256` that could bind but believes the server cannot is
//! refused where it can [RFC 5802 §6]. A user not there is answered with a
//! salt made of the user's name, the same each time, as a user's is, and
//! refused as a wrong proof is, so a client cannot tell who exists.

use std::num::NonZeroU32;
use std::sync::Arc;

use aws_lc_rs::hmac;
use turborel::db::Database;
use turborel::error::{Code, Result, SqlError};
use turborel_protocol::scram::{self, First, KEY_BYTES, Keys, Offer, Server};

use crate::auth::{Authenticator, Exchange, Peer, Step};
use crate::password::{Pbkdf2, SALT_BYTES, Verifier};
use crate::settings::{self, SCRAM_WITHOUT_TLS};

/// The name of the mechanism of `Scram`.
pub const MECHANISM: &str = scram::MECHANISM;
/// The name of the mechanism of `Scram::plus`, bound to its channel.
pub const MECHANISM_PLUS: &str = scram::MECHANISM_PLUS;

/// \[DESIGN\] The mechanism `scram-sha-256`, or, of `plus`,
/// `scram-sha-256-plus`.
pub struct Scram {
    shared: Arc<Shared>,
    /// Whether it is `scram-sha-256-plus`.
    plus: bool,
}

/// What both mechanisms of a server share, so a user not there is given
/// the same salt by each, as a user is.
struct Shared {
    db: Database,
    /// A secret of this server's, of which a user not there is given a
    /// salt and keys.
    secret: hmac::Key,
    /// The iterations a user not there is given.
    iterations: NonZeroU32,
}

impl Scram {
    /// The mechanism `scram-sha-256` over the users of `db`, a user not
    /// there given the iterations `credentials` makes verifiers of.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the system gives no randomness.
    pub fn new(db: Database, credentials: &Pbkdf2) -> Result<Scram> {
        let mut secret = [0_u8; KEY_BYTES];
        aws_lc_rs::rand::fill(&mut secret).map_err(|_| SqlError::new(Code::Io, "[DESIGN] the system gave no randomness for a secret"))?;
        let shared = Shared { db, secret: hmac::Key::new(hmac::HMAC_SHA256, &secret), iterations: credentials.iterations() };
        Ok(Scram { shared: Arc::new(shared), plus: false })
    }

    /// The mechanism `scram-sha-256-plus`, bound to its TLS connection, of
    /// the same users and secret: a server offering binding serves both
    /// [RFC 5802 §6], as clients over TLS ask for this one.
    #[must_use]
    pub fn plus(&self) -> Scram {
        Scram { shared: Arc::clone(&self.shared), plus: true }
    }

    /// The verifier of `user`, or, not there, one made of its name and
    /// this server's secret; and whether it is there.
    fn verifier(&self, user: &str) -> (Verifier, bool) {
        if let Some(read) = self.shared.db.verifier(user).ok().flatten().as_deref().and_then(Verifier::read) {
            return (read, true);
        }
        let made = hmac::sign(&self.shared.secret, user.as_bytes());
        let salt = made.as_ref().get(..SALT_BYTES).unwrap_or_default().to_vec();
        let salted = made.as_ref().try_into().unwrap_or([0; KEY_BYTES]);
        (Verifier { iterations: self.shared.iterations, salt, keys: Keys::of(&salted) }, false)
    }
}

impl Authenticator for Scram {
    fn mechanism(&self) -> &'static str {
        if self.plus { MECHANISM_PLUS } else { MECHANISM }
    }

    fn begin(&self, user: &str, peer: &Peer) -> Box<dyn Exchange> {
        let allowed = peer.secure || settings::text(&self.shared.db, SCRAM_WITHOUT_TLS) == turborel::settings::ON;
        let (verifier, known) = self.verifier(user);
        let offer = (self.plus, peer.end_point.clone());
        Box::new(Proving { user: user.to_owned(), offer, stage: Stage::Begun { allowed, verifier, known } })
    }
}

/// How far an exchange has come.
enum Stage {
    /// Nothing taken yet: whether the connection may carry it, and the
    /// user's verifier, or a decoy's.
    Begun { allowed: bool, verifier: Verifier, known: bool },
    /// server-first sent.
    Challenged { server: Server, known: bool },
    /// server-final sent: the client checks the server's signature.
    Proved,
    /// Accepted or refused.
    Over,
}

/// An exchange of `Scram`.
struct Proving {
    user: String,
    /// Whether its mechanism is `scram-sha-256-plus`, and its connection's
    /// channel binding, if it has one.
    offer: (bool, Option<Vec<u8>>),
    stage: Stage,
}

/// The refusal of a wrong proof, or of a user not there: alike.
fn wrong() -> Step {
    Step::Refuse("[DESIGN] the user or the password is wrong".into())
}

impl Exchange for Proving {
    fn step(&mut self, input: &[u8]) -> Step {
        match std::mem::replace(&mut self.stage, Stage::Over) {
            Stage::Begun { allowed: false, .. } => Step::Refuse(
                "[DESIGN] SCRAM-SHA-256 is taken over TLS, a Unix socket or a named pipe, over plain TCP only if SERVER.SCRAM_WITHOUT_TLS is ON".into(),
            ),
            Stage::Begun { allowed: true, verifier, known } => {
                let offer = Offer { plus: self.offer.0, end_point: self.offer.1.as_deref() };
                let begun = First::read(input, offer).and_then(|first| Ok((first, scram::nonce()?)));
                let (first, nonce) = match begun {
                    Ok(begun) => begun,
                    Err(e) => return Step::Refuse(e.msg),
                };
                let server = Server::begin(first, &nonce, (&verifier.salt, verifier.iterations), verifier.keys);
                let challenge = server.first();
                self.stage = Stage::Challenged { server, known };
                Step::Challenge(challenge)
            }
            Stage::Challenged { server, known } => match (server.finish(input), known) {
                (Ok(last), true) => {
                    self.stage = Stage::Proved;
                    Step::Challenge(last)
                }
                (Ok(_), false) | (Err(_), _) => wrong(),
            },
            Stage::Proved => Step::Accept(self.user.clone()),
            Stage::Over => Step::Refuse("[DESIGN] SCRAM-SHA-256's exchange is over".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D43: an exchange answered after it is
    /// over is refused, never a panic; a user not there is given the same
    /// salt each time, another's another, and the same by
    /// `scram-sha-256-plus`, of the same secret.
    #[test]
    fn an_exchange_over_is_refused() {
        let db = Database::new();
        let scram = Scram::new(db, &Pbkdf2::new(NonZeroU32::new(10_000).unwrap())).unwrap();
        let mut exchange = scram.begin("NOBODY", &Peer::local());
        assert!(matches!(exchange.step(b"not scram"), Step::Refuse(_)));
        let Step::Refuse(over) = exchange.step(b"") else { panic!("not refused") };
        assert!(over.contains("over"), "{over}");
        let (first, second, other) = (scram.verifier("NOBODY").0.salt, scram.verifier("NOBODY").0.salt, scram.verifier("ELSE").0.salt);
        assert!(first == second && first != other && first.len() == SALT_BYTES);
        assert!(!scram.verifier("NOBODY").1);
        assert_eq!(scram.plus().verifier("NOBODY").0.salt, first, "the same under scram-sha-256-plus");
        assert_eq!((scram.mechanism(), scram.plus().mechanism()), (MECHANISM, MECHANISM_PLUS));
    }
}
