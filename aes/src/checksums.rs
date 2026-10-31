//! \[EXTENSION\] Cryptographic hash functions, as `HashFunction`s (RULES.md
//! D26, D30), from aws-lc-rs, selected for a database's files by
//! STORAGE.CHECKSUM as it is created.
//!
//! - SHA-256 [FIPS 180-4]: a checksum no accident matches, nor any change a
//!   forger makes without writing the checksum again; one who can rewrite
//!   a file can rewrite its checksums too, as they are of nothing secret.
//! - HMAC-SHA-256 [RFC 2104]: a checksum keyed by a key of a keyring, so a
//!   forger without the key cannot match it. Its key is HKDF-SHA256
//!   [RFC 5869] of the named key, info of its own, so a key named for an
//!   encryption too gives each a key of its own. A file records only the
//!   function's id, never the key: one opened with another key fails its
//!   header's checksum, and is refused (SQLCODE -916).
//!
//! Neither stops a forger from putting back a page or block the database
//! once wrote at the same place: their checksums are its own.

use aws_lc_rs::digest::{self, SHA256};
use aws_lc_rs::hkdf::{HKDF_SHA256, Salt};
use aws_lc_rs::hmac::{self, HMAC_SHA256};
use turborel::error::{Code, Result, SqlError};
use turborel::hash::{Digest, HashFunction, HashId};

use crate::keyring::{KEY_BYTES, Keyring, valid_key_name};
use crate::sealer::{Length, failed};

/// \[EXTENSION\] SHA-256's id in file formats: never changed or reused
/// (RULES.md D30).
pub const SHA256_ID: HashId = HashId(11);
/// Its name, as FIPS 180-4 writes it.
pub const SHA256_NAME: &str = "SHA-256";
/// \[EXTENSION\] HMAC-SHA-256's id in file formats: never changed or
/// reused (RULES.md D30).
pub const HMAC_SHA256_ID: HashId = HashId(12);
/// Its name.
pub const HMAC_SHA256_NAME: &str = "HMAC-SHA-256";
/// Bytes of either's digest.
const DIGEST_BYTES: usize = 32;
/// What HKDF derives an HMAC key for.
const INFO: &[u8] = b"turborel HMAC-SHA-256 checksum key";

/// \[EXTENSION\] SHA-256 [FIPS 180-4], named "SHA-256".
pub struct Sha256;

impl HashFunction for Sha256 {
    fn id(&self) -> HashId {
        SHA256_ID
    }
    fn name(&self) -> &'static str {
        SHA256_NAME
    }
    fn digest_len(&self) -> usize {
        DIGEST_BYTES
    }
    fn hash_parts(&self, parts: &[&[u8]]) -> Digest {
        let mut context = digest::Context::new(&SHA256);
        for part in parts {
            context.update(part);
        }
        Digest::new(context.finish().as_ref()).unwrap_or_default()
    }
}

/// \[EXTENSION\] HMAC-SHA-256 [RFC 2104], named "HMAC-SHA-256", keyed by a
/// key of a keyring.
pub struct HmacSha256 {
    key: hmac::Key,
}

impl HmacSha256 {
    /// HMAC-SHA-256 keyed by the key `name` of `keyring`, through HKDF.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `name` names no key the keyring has; the keyring's
    /// errors; SQLCODE -915 if the key cannot be derived.
    pub fn new(keyring: &dyn Keyring, name: &str) -> Result<HmacSha256> {
        let master = valid_key_name(name).then(|| keyring.key(name)).transpose()?.flatten().ok_or_else(|| {
            SqlError::new(
                Code::Api,
                format!("[EXTENSION] {HMAC_SHA256_NAME} needs a key: the keyring ({}) has no key {name:?}", keyring.name()),
            )
        })?;
        let mut bytes = [0_u8; KEY_BYTES];
        let info = [INFO];
        let prk = Salt::new(HKDF_SHA256, &[]).extract(&master);
        prk.expand(&info, Length).and_then(|okm| okm.fill(&mut bytes)).map_err(|_| failed(HMAC_SHA256_NAME, "deriving a key"))?;
        Ok(HmacSha256::of_bytes(&bytes))
    }

    /// HMAC-SHA-256 keyed by `key` itself, for known answers [RFC 4231].
    pub(crate) fn of_bytes(key: &[u8]) -> HmacSha256 {
        HmacSha256 { key: hmac::Key::new(HMAC_SHA256, key) }
    }
}

impl HashFunction for HmacSha256 {
    fn id(&self) -> HashId {
        HMAC_SHA256_ID
    }
    fn name(&self) -> &'static str {
        HMAC_SHA256_NAME
    }
    fn digest_len(&self) -> usize {
        DIGEST_BYTES
    }
    fn hash_parts(&self, parts: &[&[u8]]) -> Digest {
        let mut context = hmac::Context::with_key(&self.key);
        for part in parts {
            context.update(part);
        }
        Digest::new(context.sign().as_ref()).unwrap_or_default()
    }
}
