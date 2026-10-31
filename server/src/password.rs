//! \[DESIGN\] Passwords (RULES.md D26, D43): PBKDF2-HMAC-SHA256 from
//! aws-lc-rs, the same library as TLS's, with a FIPS 140-3 validated build.
//! A user's verifier is made of its password with a random salt and many
//! iterations, so a verifier stolen is slow to guess from. It keeps what
//! SCRAM-SHA-256 (RFC 5802, `scram.rs`) checks a proof against, StoredKey
//! and ServerKey, never the salted password itself, which would let
//! whoever read it log in by SCRAM. The authenticator `password` checks a
//! password a client sends against the same verifier, over a secure
//! connection only, as the password travels whole.
//!
//! ```text
//! verifier: $scram-sha-256$i=<iterations>$<salt, hex>$<StoredKey, hex>$<ServerKey, hex>
//! salted:   PBKDF2-HMAC-SHA256(password, salt, iterations), 32 bytes
//! keys:     StoredKey = SHA-256(HMAC(salted, "Client Key"))
//!           ServerKey = HMAC(salted, "Server Key")
//! ```
//!
//! The verifier names its scheme and iterations, so one made with fewer
//! still checks once SERVER.PASSWORD_ITERATIONS is raised, and another
//! scheme may come beside this one.

use std::fmt::Write as _;
use std::num::NonZeroU32;

use turborel::db::{Credentials, Database};
use turborel::error::{Code, Result, SqlError};
use turborel_protocol::scram::{Key, Keys, salted_password};

use crate::auth::{Authenticator, Exchange, Peer, Step};
use crate::settings::{self, PASSWORD_ITERATIONS};

/// The name of the mechanism of `Password`.
pub const MECHANISM: &str = "password";
/// The scheme a verifier names.
pub const SCHEME: &str = "scram-sha-256";
/// \[DESIGN\] The bytes of a salt: 128 bits, as NIST SP 800-132 asks at
/// least.
pub const SALT_BYTES: usize = 16;
/// The iterations if the setting cannot be read: its default.
const DEFAULT: NonZeroU32 = NonZeroU32::new(600_000).unwrap();

/// \[DESIGN\] What makes verifiers: PBKDF2-HMAC-SHA256, at `iterations`,
/// and SCRAM's keys of it.
#[derive(Debug, Clone, Copy)]
pub struct Pbkdf2 {
    iterations: NonZeroU32,
}

/// `bytes` in hex, two lower-case digits each.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        // Writing to a String does not fail.
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = text.chars().map(|c| c.to_digit(16).and_then(|d| u8::try_from(d).ok())).collect::<Option<_>>()?;
    let (pairs, odd) = digits.as_chunks::<2>();
    if !odd.is_empty() {
        return None;
    }
    Some(pairs.iter().map(|[high, low]| high.wrapping_shl(4) | low).collect())
}

/// The key `text` is in hex, if it is one of `scram::KEY_BYTES`.
fn key(text: &str) -> Option<Key> {
    unhex(text)?.try_into().ok()
}

/// A verifier, read: its iterations, salt and keys.
pub struct Verifier {
    /// The iterations of PBKDF2.
    pub iterations: NonZeroU32,
    /// The salt.
    pub salt: Vec<u8>,
    /// StoredKey and ServerKey.
    pub keys: Keys,
}

impl Verifier {
    /// `verifier` read; none if it is not of this scheme, or malformed.
    pub fn read(verifier: &[u8]) -> Option<Verifier> {
        let text = std::str::from_utf8(verifier).ok()?;
        let mut fields = text.strip_prefix('$')?.split('$');
        let (scheme, iterations, salt) = (fields.next()?, fields.next()?, fields.next()?);
        let (stored, server) = (fields.next()?, fields.next()?);
        if scheme != SCHEME || fields.next().is_some() {
            return None;
        }
        let iterations = iterations.strip_prefix("i=")?.parse().ok()?;
        let keys = Keys { stored: key(stored)?, server: key(server)? };
        Some(Verifier { iterations, salt: unhex(salt)?, keys })
    }
}

/// Whether `password` is the one `verifier` was made of; false for a
/// verifier not of this scheme. Takes as long whichever.
pub fn check(verifier: &[u8], password: &str) -> bool {
    let Some(read) = Verifier::read(verifier) else { return false };
    read.keys.match_salted(&salted_password(password, &read.salt, read.iterations))
}

impl Pbkdf2 {
    /// Verifiers of `iterations` iterations.
    pub fn new(iterations: NonZeroU32) -> Pbkdf2 {
        Pbkdf2 { iterations }
    }

    /// Verifiers of the iterations SERVER.PASSWORD_ITERATIONS of `db` says.
    pub fn of(db: &Database) -> Pbkdf2 {
        let iterations = u32::try_from(settings::count(db, PASSWORD_ITERATIONS)).ok().and_then(NonZeroU32::new);
        Pbkdf2::new(iterations.unwrap_or(DEFAULT))
    }

    /// The iterations of the verifiers it makes.
    pub fn iterations(&self) -> NonZeroU32 {
        self.iterations
    }

    /// The verifier of `password`, with a new random salt.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the system gives no randomness.
    pub fn make(&self, password: &str) -> Result<String> {
        let mut salt = [0_u8; SALT_BYTES];
        aws_lc_rs::rand::fill(&mut salt).map_err(|_| SqlError::new(Code::Io, "[DESIGN] the system gave no randomness for a salt"))?;
        let keys = Keys::of(&salted_password(password, &salt, self.iterations));
        Ok(format!("${SCHEME}$i={}${}${}${}", self.iterations, hex(&salt), hex(&keys.stored), hex(&keys.server)))
    }
}

impl Credentials for Pbkdf2 {
    fn name(&self) -> &'static str {
        SCHEME
    }

    fn verifier(&self, _: &str, password: &str) -> Result<Vec<u8>> {
        Ok(self.make(password)?.into_bytes())
    }
}

/// \[DESIGN\] The mechanism `password`: a client sends its password, which
/// is checked against the user's verifier in the database; over a secure
/// connection only. A user not there is refused as a wrong password is,
/// after as long, so a client cannot tell which.
pub struct Password {
    db: Database,
    /// A verifier no password is checked against for a user not there, so
    /// that takes as long as a check.
    decoy: Vec<u8>,
}

impl Password {
    /// The mechanism over the users of `db`, checking as slowly as
    /// `credentials` makes verifiers.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the system gives no randomness.
    pub fn new(db: Database, credentials: &Pbkdf2) -> Result<Password> {
        Ok(Password { db, decoy: credentials.make("")?.into_bytes() })
    }
}

impl Authenticator for Password {
    fn mechanism(&self) -> &'static str {
        MECHANISM
    }

    fn begin(&self, user: &str, peer: &Peer) -> Box<dyn Exchange> {
        Box::new(Checking { db: self.db.clone(), decoy: self.decoy.clone(), user: user.to_owned(), secure: peer.secure })
    }
}

/// An exchange of `Password`.
struct Checking {
    db: Database,
    decoy: Vec<u8>,
    user: String,
    secure: bool,
}

impl Exchange for Checking {
    fn step(&mut self, input: &[u8]) -> Step {
        if !self.secure {
            return Step::Refuse("[DESIGN] a password is taken only over TLS or a Unix socket".into());
        }
        let password = String::from_utf8_lossy(input);
        let kept = self.db.verifier(&self.user).ok().flatten();
        let matches = check(kept.as_deref().unwrap_or(&self.decoy), &password);
        if kept.is_some() && matches {
            return Step::Accept(self.user.clone());
        }
        Step::Refuse("[DESIGN] the user or the password is wrong".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEW: NonZeroU32 = NonZeroU32::new(1000).unwrap();

    /// Verifies: [DESIGN] RULES.md D26, D43: a verifier names its scheme
    /// and iterations, a salt of 16 bytes and two keys of 32, in hex; the
    /// same password makes another verifier each time, its salt new; each
    /// checks its password and no other. Its keys are SCRAM's of the salted
    /// password, which it does not keep.
    #[test]
    fn a_verifier_checks_its_password_only() {
        let made = Pbkdf2::new(FEW).make("secret").unwrap();
        let fields: Vec<&str> = made.split('$').collect();
        let lengths = (fields[3].len(), fields[4].len(), fields[5].len());
        assert_eq!((fields[1], fields[2], lengths), ("scram-sha-256", "i=1000", (32, 64, 64)), "{made}");
        let again = Pbkdf2::new(FEW).make("secret").unwrap();
        assert_ne!(made, again, "a new salt");
        assert!(check(made.as_bytes(), "secret") && check(again.as_bytes(), "secret"));
        assert!(!check(made.as_bytes(), "Secret") && !check(made.as_bytes(), ""));
        assert_eq!(Pbkdf2::new(FEW).verifier("ANN", "x").unwrap().first(), Some(&b'$'));
        assert_eq!((Credentials::name(&Pbkdf2::new(FEW)), Pbkdf2::new(FEW).iterations()), (SCHEME, FEW));
        let read = Verifier::read(made.as_bytes()).unwrap();
        let salted = salted_password("secret", &read.salt, read.iterations);
        assert_eq!(read.keys, Keys::of(&salted));
        assert!(!made.contains(&hex(&salted)), "the salted password is not kept");
    }

    /// Verifies: [DESIGN] RULES.md D43, R-ERR-1: a verifier not of this
    /// scheme, or malformed, checks no password, never panics: another
    /// scheme, fields missing or extra, iterations not a number or zero, a
    /// salt or key not hex or of odd length, a key not 32 bytes, bytes not
    /// UTF-8.
    #[test]
    fn a_malformed_verifier_checks_nothing() {
        let made = Pbkdf2::new(FEW).make("x").unwrap();
        let swapped = |from: &str, to: &str| made.replacen(from, to, 1);
        let server = made.rsplit('$').next().unwrap().to_owned();
        let wrong = [
            swapped("scram-sha-256", "pbkdf2-sha256"),
            swapped("$i=1000", ""),
            format!("{made}$more"),
            swapped("i=1000", "i=many"),
            swapped("i=1000", "i=0"),
            swapped("i=1000", "1000"),
            made.replace(&server, &format!("{}g", server.get(1..).unwrap())),
            made.replace(&server, server.get(1..).unwrap()),
            made.replace(&server, server.get(2..).unwrap()),
            made.trim_start_matches('$').to_owned(),
            String::new(),
        ];
        for verifier in &wrong {
            assert!(!check(verifier.as_bytes(), "x"), "{verifier}");
        }
        assert!(!check(&[0xff, 0xfe], "x"));
        assert_eq!(unhex("0aFf"), Some(vec![0x0a, 0xff]));
    }
}
