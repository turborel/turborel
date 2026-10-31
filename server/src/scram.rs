//! \[DESIGN\] The mechanism `scram-sha-256` (RULES.md D43; RFC 5802, RFC
//! 7677; the messages in `turborel_protocol::scram`): a client proves it
//! knows the user's password, and the server that it knows the user's
//! verifier, the password never sent. The proof is checked against the
//! StoredKey of the verifier `password.rs` makes, so one verifier serves
//! this mechanism and `password`.
//!
//! ```text
//! client: Hello, mechanism scram-sha-256, client-first
//! server: Challenge, server-first
//! client: Answer, client-final
//! server: Challenge, server-final      (the proof right)
//! client: Answer, empty                (the server's signature right)
//! server: Ready
//! ```
//!
//! The exchange does not give the password away, but one overheard lets
//! its eavesdropper try passwords against it at its own pace, and without
//! channel binding a server in the middle could relay it. So it is taken
//! over TLS or a Unix socket, and over plain TCP only while
//! SERVER.SCRAM_WITHOUT_TLS is ON. A user not there is answered with a salt
//! made of the user's name, the same each time, as a user's is, and refused
//! as a wrong proof is, so a client cannot tell who exists.

use std::num::NonZeroU32;

use aws_lc_rs::hmac;
use turborel::db::Database;
use turborel::error::{Code, Result, SqlError};
use turborel_protocol::scram::{self, First, KEY_BYTES, Keys, Server};

use crate::auth::{Authenticator, Exchange, Peer, Step};
use crate::password::{Pbkdf2, SALT_BYTES, Verifier};
use crate::settings::{self, SCRAM_WITHOUT_TLS};

/// The name of the mechanism of `Scram`.
pub const MECHANISM: &str = scram::MECHANISM;

/// \[DESIGN\] The mechanism `scram-sha-256`.
pub struct Scram {
    db: Database,
    /// A secret of this server's, of which a user not there is given a
    /// salt and keys.
    secret: hmac::Key,
    /// The iterations a user not there is given.
    iterations: NonZeroU32,
}

impl Scram {
    /// The mechanism over the users of `db`, a user not there given the
    /// iterations `credentials` makes verifiers of.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the system gives no randomness.
    pub fn new(db: Database, credentials: &Pbkdf2) -> Result<Scram> {
        let mut secret = [0_u8; KEY_BYTES];
        aws_lc_rs::rand::fill(&mut secret).map_err(|_| SqlError::new(Code::Io, "[DESIGN] the system gave no randomness for a secret"))?;
        Ok(Scram { db, secret: hmac::Key::new(hmac::HMAC_SHA256, &secret), iterations: credentials.iterations() })
    }

    /// The verifier of `user`, or, not there, one made of its name and
    /// this server's secret; and whether it is there.
    fn verifier(&self, user: &str) -> (Verifier, bool) {
        if let Some(read) = self.db.verifier(user).ok().flatten().as_deref().and_then(Verifier::read) {
            return (read, true);
        }
        let made = hmac::sign(&self.secret, user.as_bytes());
        let salt = made.as_ref().get(..SALT_BYTES).unwrap_or_default().to_vec();
        let salted = made.as_ref().try_into().unwrap_or([0; KEY_BYTES]);
        (Verifier { iterations: self.iterations, salt, keys: Keys::of(&salted) }, false)
    }
}

impl Authenticator for Scram {
    fn mechanism(&self) -> &'static str {
        MECHANISM
    }

    fn begin(&self, user: &str, peer: &Peer) -> Box<dyn Exchange> {
        let allowed = peer.secure || settings::text(&self.db, SCRAM_WITHOUT_TLS) == turborel::settings::ON;
        let (verifier, known) = self.verifier(user);
        Box::new(Proving { user: user.to_owned(), stage: Stage::Begun { allowed, verifier, known } })
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
                "[DESIGN] SCRAM-SHA-256 is taken over TLS or a Unix socket, over plain TCP only if SERVER.SCRAM_WITHOUT_TLS is ON".into(),
            ),
            Stage::Begun { allowed: true, verifier, known } => {
                let begun = First::read(input).and_then(|first| Ok((first, scram::nonce()?)));
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
    /// salt each time, another's another.
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
    }
}
