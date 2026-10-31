//! \[EXTENSION\] Cryptography for turborel from aws-lc-rs (RULES.md D26,
//! D30, D48), as its plug-ins:
//!
//! - encryptions, as encodings (`Database::register_encoding`):
//!   AES-256-GCM (`AesGcm`) and ChaCha20-Poly1305 (`ChaCha20Poly1305`),
//!   each named with its key, `AES-256-GCM:NAME`, by STORAGE.ENCODINGS for
//!   every file, or by a table's option ENCODINGS for its rows and their
//!   log records (`sealer.rs`, `ciphers.rs`);
//! - cryptographic hash functions, as `HashFunction`s
//!   (`Database::register_hash_function`): SHA-256 (`Sha256`) and
//!   HMAC-SHA-256 keyed by a key (`HmacSha256`), selected by
//!   STORAGE.CHECKSUM (`checksums.rs`).
//!
//! Keys come from a `Keyring` its host gives it, never from the database's
//! files (`keyring.rs`).

mod checksums;
mod ciphers;
mod keyring;
mod sealer;

pub use checksums::{HMAC_SHA256_ID, HMAC_SHA256_NAME, HmacSha256, SHA256_ID, SHA256_NAME, Sha256};
pub use ciphers::{AES_ID, AES_NAME, Aes256Gcm, CHACHA_ID, CHACHA_NAME, ChaCha, Cipher, NonceBytes};
pub use keyring::{KEY_BYTES, KeyDirectory, Keyring, MAX_KEY_NAME, valid_key_name};
pub use sealer::{SUBKEY_USES, Sealer};

/// \[EXTENSION\] AES-256-GCM, named "AES-256-GCM", its keys from a keyring.
pub type AesGcm = Sealer<Aes256Gcm>;

/// \[EXTENSION\] ChaCha20-Poly1305 [RFC 8439], named "ChaCha20-Poly1305",
/// its keys from a keyring.
pub type ChaCha20Poly1305 = Sealer<ChaCha>;

#[cfg(test)]
mod tests;
