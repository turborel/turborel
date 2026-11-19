//! \[EXTENSION\] What a database trusts of packages (design document
//! §13.4): the Ed25519 keys of PLUGINS.TRUSTED_KEYS, and whether
//! PLUGINS.REQUIRE_SIGNED refuses a package no key of them signed.

use std::path::Path;

use crate::error::Result;

use super::crypto::{PackageCrypto, PublicKey, Signature};
use super::errors::untrusted;
use super::{hex, unhex_array};

/// What separates the keys of PLUGINS.TRUSTED_KEYS.
pub const KEY_SEPARATOR: char = ';';
/// What begins PLUGINS.TRUSTED_KEYS that names a file of keys.
pub const FILE_PREFIX: char = '@';

/// The keys trusted, and whether a signature by one is required.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trust {
    /// The public keys trusted.
    pub keys: Vec<PublicKey>,
    /// Whether a package must be signed by one of `keys`.
    pub require_signed: bool,
}

impl Trust {
    /// The trust PLUGINS.TRUSTED_KEYS's `keys` and PLUGINS.REQUIRE_SIGNED's
    /// `require_signed` say. `keys` is public keys in hexadecimal,
    /// separated by `;`, or `@` and a file of them, one a line, a line
    /// beginning `#` passed over.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T08 if a key is not 32 bytes of hexadecimal, or the file
    /// cannot be read.
    pub fn parse(keys: &str, require_signed: bool) -> Result<Trust> {
        let written = keys.trim();
        let listed = match written.strip_prefix(FILE_PREFIX) {
            Some(file) => std::fs::read_to_string(Path::new(file.trim()))
                .map_err(|e| untrusted(&format!("the file of trusted keys {file} cannot be read: {e}")))?,
            None => written.replace(KEY_SEPARATOR, "\n"),
        };
        let words = listed.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
        let parsed = words
            .map(|w| unhex_array(w).ok_or_else(|| untrusted(&format!("{w} is not an Ed25519 public key in hexadecimal"))))
            .collect::<Result<_>>()?;
        Ok(Trust { keys: parsed, require_signed })
    }

    /// Whether `key` is trusted.
    pub fn trusts(&self, key: &PublicKey) -> bool {
        self.keys.contains(key)
    }

    /// What `signature`, by `key`, of `message`, if any, says: the signer,
    /// in hexadecimal, if a key trusted signed it; none if no signature is
    /// there, or one by a key not trusted, and none is required.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T08 if a signature does not verify, or none by a key
    /// trusted is there and one is required; `what` names what is signed.
    pub fn check(
        &self,
        crypto: &dyn PackageCrypto,
        signed: Option<(&PublicKey, &Signature)>,
        message: &[u8],
        what: &str,
    ) -> Result<Option<String>> {
        let Some((key, signature)) = signed else {
            if self.require_signed {
                return Err(untrusted(&format!("{what} is not signed, and PLUGINS.REQUIRE_SIGNED is ON")));
            }
            return Ok(None);
        };
        if !crypto.verify(key, message, signature) {
            return Err(untrusted(&format!("the signature of {what} does not verify")));
        }
        if self.trusts(key) {
            return Ok(Some(hex(key)));
        }
        if self.require_signed {
            return Err(untrusted(&format!("{what} is signed by {}, a key not in PLUGINS.TRUSTED_KEYS", hex(key))));
        }
        Ok(None)
    }
}
