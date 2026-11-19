//! \[EXTENSION\] The cryptography of extension packages (design document
//! §13.4, RULES.md D26): SHA-256 digests and Ed25519 signatures from
//! aws-lc-rs, the library TLS and SCRAM use, given to the core through
//! its `PackageCrypto` and `Signer`, which declare them without a
//! dependency of their own.
//!
//! A signing key is kept as a file of one line, the 32 bytes of its
//! Ed25519 seed in hexadecimal; its public key is the 32 bytes the
//! package names its signer by, also in hexadecimal.

use std::path::Path;

use aws_lc_rs::digest::{SHA256, digest};
use aws_lc_rs::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use turborel::error::{Code, Result, SqlError};
use turborel::plugin::package::crypto::{Digest, PackageCrypto, PublicKey, Signature, Signer};
use turborel::plugin::package::{hex, unhex_array};

/// The bytes of an Ed25519 seed.
pub const SEED_LEN: usize = 32;

/// SHA-256 and Ed25519 verification, from aws-lc-rs.
#[derive(Debug, Clone, Copy, Default)]
pub struct Crypto;

impl PackageCrypto for Crypto {
    fn sha256(&self, bytes: &[u8]) -> Digest {
        digest(&SHA256, bytes).as_ref().try_into().unwrap_or([0_u8; 32])
    }

    fn verify(&self, key: &PublicKey, message: &[u8], signature: &Signature) -> bool {
        UnparsedPublicKey::new(&ED25519, key.as_slice()).verify(message, signature.as_slice()).is_ok()
    }
}

/// An Ed25519 signing key.
pub struct SigningKey {
    pair: Ed25519KeyPair,
    seed: [u8; SEED_LEN],
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SigningKey({})", hex(&self.public_key()))
    }
}

/// A key's failure, `why`.
fn refused(why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] {why}"))
}

impl SigningKey {
    /// A new key, of the system's randomness.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the system gives no randomness.
    pub fn generate() -> Result<SigningKey> {
        let mut seed = [0_u8; SEED_LEN];
        aws_lc_rs::rand::fill(&mut seed).map_err(|_| SqlError::new(Code::Io, "[DESIGN] the system gave no randomness for a key"))?;
        SigningKey::of_seed(seed)
    }

    /// The key of `seed`.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if aws-lc-rs refuses it.
    pub fn of_seed(seed: [u8; SEED_LEN]) -> Result<SigningKey> {
        let pair = Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|_| refused("the signing key is not an Ed25519 seed"))?;
        Ok(SigningKey { pair, seed })
    }

    /// The key the file `path` holds.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot be read; -301 if it holds no key.
    pub fn read(path: &Path) -> Result<SigningKey> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| SqlError::new(Code::Io, format!("[EXTENSION] {} cannot be read: {e}", path.display())))?;
        let seed = unhex_array(text.trim()).ok_or_else(|| refused(&format!("{} holds no Ed25519 seed in hexadecimal", path.display())))?;
        SigningKey::of_seed(seed)
    }

    /// The text of its key file: its seed in hexadecimal.
    pub fn file_text(&self) -> String {
        format!("{}\n", hex(&self.seed))
    }
}

impl Signer for SigningKey {
    fn public_key(&self) -> PublicKey {
        self.pair.public_key().as_ref().try_into().unwrap_or([0_u8; 32])
    }

    fn sign(&self, message: &[u8]) -> Signature {
        self.pair.sign(message).as_ref().try_into().unwrap_or([0_u8; 64])
    }
}

#[cfg(test)]
mod package_tests {
    use super::*;

    /// Verifies: [EXTENSION] design document §13.4: SHA-256 of "abc" is the
    /// standard's; a signature verifies by its key, and not of another
    /// message or by another key; a key written to its file reads back.
    #[test]
    fn digests_and_signatures_are_ed25519_and_sha256() {
        assert_eq!(hex(&Crypto.sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let (key, other) = (SigningKey::generate().unwrap(), SigningKey::generate().unwrap());
        let signature = key.sign(b"message");
        assert!(Crypto.verify(&key.public_key(), b"message", &signature));
        assert!(!Crypto.verify(&key.public_key(), b"messagf", &signature));
        assert!(!Crypto.verify(&other.public_key(), b"message", &signature));
        let seed = unhex_array(key.file_text().trim()).unwrap();
        assert_eq!(SigningKey::of_seed(seed).unwrap().public_key(), key.public_key());
    }
}
