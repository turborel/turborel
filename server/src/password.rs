//! \[DESIGN\] Passwords (RULES.md D26, D43): PBKDF2-HMAC-SHA256 from
//! aws-lc-rs, the same library as TLS's, with a FIPS 140-3 validated build.
//! A user's verifier is made of its password with a random salt and many
//! iterations, so a verifier stolen is slow to guess from; the
//! authenticator `password` checks a password a client sends against it,
//! over a secure connection only, as the password travels whole.
//!
//! ```text
//! verifier: $pbkdf2-sha256$i=<iterations>$<salt, hex>$<hash, hex>
//! ```
//!
//! The verifier names its scheme and iterations, so one made with fewer
//! still checks once SERVER.PASSWORD_ITERATIONS is raised, and another
//! scheme may come beside this one.

use std::fmt::Write as _;
use std::num::NonZeroU32;

use aws_lc_rs::pbkdf2::{self, PBKDF2_HMAC_SHA256};
use turborel::db::{Credentials, Database};
use turborel::error::{Code, Result, SqlError};

use crate::auth::{Authenticator, Exchange, Peer, Step};
use crate::settings::{self, PASSWORD_ITERATIONS};

/// The name of the mechanism of `Password`.
pub const MECHANISM: &str = "password";
/// The scheme a verifier names.
pub const SCHEME: &str = "pbkdf2-sha256";
/// \[DESIGN\] The bytes of a salt: 128 bits, as NIST SP 800-132 asks at
/// least.
const SALT_BYTES: usize = 16;
/// The bytes of a hash: SHA-256's.
const HASH_BYTES: usize = 32;
/// The iterations if the setting cannot be read: its default.
const DEFAULT: NonZeroU32 = NonZeroU32::new(600_000).unwrap();

/// \[DESIGN\] What makes verifiers: PBKDF2-HMAC-SHA256, at `iterations`.
#[derive(Debug, Clone, Copy)]
pub struct Pbkdf2 {
    iterations: NonZeroU32,
}

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

/// The parts of a verifier: its iterations, salt and hash.
fn parts(verifier: &[u8]) -> Option<(NonZeroU32, Vec<u8>, Vec<u8>)> {
    let text = std::str::from_utf8(verifier).ok()?;
    let mut fields = text.strip_prefix('$')?.split('$');
    let (scheme, iterations, salt, hash) = (fields.next()?, fields.next()?, fields.next()?, fields.next()?);
    if scheme != SCHEME || fields.next().is_some() {
        return None;
    }
    let iterations = iterations.strip_prefix("i=")?.parse().ok()?;
    let (salt, hash) = (unhex(salt)?, unhex(hash)?);
    (hash.len() == HASH_BYTES).then_some((iterations, salt, hash))
}

/// Whether `password` is the one `verifier` was made of; false for a
/// verifier not of this scheme. Takes as long whichever.
pub fn check(verifier: &[u8], password: &str) -> bool {
    let Some((iterations, salt, hash)) = parts(verifier) else { return false };
    pbkdf2::verify(PBKDF2_HMAC_SHA256, iterations, &salt, password.as_bytes(), &hash).is_ok()
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

    /// The verifier of `password`, with a new random salt.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the system gives no randomness.
    pub fn make(&self, password: &str) -> Result<String> {
        let mut salt = [0_u8; SALT_BYTES];
        aws_lc_rs::rand::fill(&mut salt).map_err(|_| SqlError::new(Code::Io, "[DESIGN] the system gave no randomness for a salt"))?;
        let mut hash = [0_u8; HASH_BYTES];
        pbkdf2::derive(PBKDF2_HMAC_SHA256, self.iterations, &salt, password.as_bytes(), &mut hash);
        Ok(format!("${SCHEME}$i={}${}${}", self.iterations, hex(&salt), hex(&hash)))
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
    /// and iterations, a salt of 16 bytes and a hash of 32, in hex; the same
    /// password makes another verifier each time, its salt new; each checks
    /// its password and no other.
    #[test]
    fn a_verifier_checks_its_password_only() {
        let made = Pbkdf2::new(FEW).make("secret").unwrap();
        let fields: Vec<&str> = made.split('$').collect();
        assert_eq!((fields[1], fields[2], fields[3].len(), fields[4].len()), ("pbkdf2-sha256", "i=1000", 32, 64), "{made}");
        let again = Pbkdf2::new(FEW).make("secret").unwrap();
        assert_ne!(made, again, "a new salt");
        assert!(check(made.as_bytes(), "secret") && check(again.as_bytes(), "secret"));
        assert!(!check(made.as_bytes(), "Secret") && !check(made.as_bytes(), ""));
        assert_eq!(Pbkdf2::new(FEW).verifier("ANN", "x").unwrap().first(), Some(&b'$'));
        assert_eq!(Credentials::name(&Pbkdf2::new(FEW)), SCHEME);
    }

    /// Verifies: [DESIGN] RULES.md D43, R-ERR-1: a verifier not of this
    /// scheme, or malformed, checks no password, never panics: another
    /// scheme, fields missing or extra, iterations not a number or zero, a
    /// salt or hash not hex or of odd length, a hash not 32 bytes, bytes not
    /// UTF-8.
    #[test]
    fn a_malformed_verifier_checks_nothing() {
        let made = Pbkdf2::new(FEW).make("x").unwrap();
        let swapped = |from: &str, to: &str| made.replacen(from, to, 1);
        let hash = made.rsplit('$').next().unwrap().to_owned();
        let wrong = [
            swapped("pbkdf2-sha256", "argon2id"),
            swapped("$i=1000", ""),
            format!("{made}$more"),
            swapped("i=1000", "i=many"),
            swapped("i=1000", "i=0"),
            swapped("i=1000", "1000"),
            made.replace(&hash, &format!("{}g", hash.get(1..).unwrap())),
            made.replace(&hash, hash.get(1..).unwrap()),
            made.replace(&hash, hash.get(2..).unwrap()),
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
