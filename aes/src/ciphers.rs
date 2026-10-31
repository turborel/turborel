//! \[EXTENSION\] The ciphers an encryption (`sealer.rs`) seals with, from
//! aws-lc-rs (RULES.md D26, D48): AES-256-GCM, fast where the processor
//! has AES instructions; and ChaCha20-Poly1305 [RFC 8439], fast in
//! software, for processors without them. Each has a key of 256 bits, a
//! nonce of 96 and a tag of 128, and an encoding id never changed or
//! reused.

use aws_lc_rs::aead::{AES_256_GCM, Aad, CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, RandomizedNonceKey, UnboundKey};
use aws_lc_rs::error::Unspecified;
use turborel::disk::encoding::EncodingId;

use crate::keyring::KEY_BYTES;

/// A nonce's bytes.
pub type NonceBytes = [u8; NONCE_LEN];

/// \[EXTENSION\] A cipher: an authenticated encryption of aws-lc-rs, its
/// nonces drawn at random as it seals.
pub trait Cipher: Send + Sync + 'static {
    /// Its encoding's id in file formats: never changed or reused.
    const ID: EncodingId;
    /// Its encoding's name.
    const NAME: &'static str;
    /// What HKDF derives its subkeys for: its own, so one key named by two
    /// ciphers gives each other subkeys.
    const INFO: &'static [u8];
    /// A key made ready to seal and open.
    type Key: Send + Sync;
    /// The key of `bytes`.
    ///
    /// # Errors
    ///
    /// The key cannot be made.
    fn key(bytes: &[u8; KEY_BYTES]) -> Result<Self::Key, Unspecified>;
    /// Encrypts `data` in place with `key`, authenticating it and `aad`,
    /// and appends the tag: the nonce drawn for it.
    ///
    /// # Errors
    ///
    /// No nonce could be drawn, or the encryption failed.
    fn seal(key: &Self::Key, aad: &[u8], data: &mut Vec<u8>) -> Result<NonceBytes, Unspecified>;
    /// Decrypts `data`, sealed with `key` and `nonce`, in place: the
    /// plaintext, without the tag.
    ///
    /// # Errors
    ///
    /// It fails its authentication.
    fn open<'a>(key: &Self::Key, nonce: NonceBytes, aad: &[u8], data: &'a mut [u8]) -> Result<&'a mut [u8], Unspecified>;
}

/// \[EXTENSION\] Its id in file formats: never changed or reused (RULES.md
/// D48).
pub const AES_ID: EncodingId = EncodingId(2);
/// Its name.
pub const AES_NAME: &str = "AES-256-GCM";
/// \[EXTENSION\] Its id in file formats: never changed or reused (RULES.md
/// D48).
pub const CHACHA_ID: EncodingId = EncodingId(3);
/// Its name, as RFC 8439 writes it.
pub const CHACHA_NAME: &str = "ChaCha20-Poly1305";

/// \[EXTENSION\] AES-256-GCM, its nonces drawn by aws-lc-rs, as its FIPS
/// rules require.
pub struct Aes256Gcm;

impl Cipher for Aes256Gcm {
    const ID: EncodingId = AES_ID;
    const NAME: &'static str = AES_NAME;
    const INFO: &'static [u8] = b"turborel AES-256-GCM subkey";
    type Key = RandomizedNonceKey;

    fn key(bytes: &[u8; KEY_BYTES]) -> Result<RandomizedNonceKey, Unspecified> {
        RandomizedNonceKey::new(&AES_256_GCM, bytes)
    }

    fn seal(key: &RandomizedNonceKey, aad: &[u8], data: &mut Vec<u8>) -> Result<NonceBytes, Unspecified> {
        key.seal_in_place_append_tag(Aad::from(aad), data).map(|nonce| *nonce.as_ref())
    }

    fn open<'a>(key: &RandomizedNonceKey, nonce: NonceBytes, aad: &[u8], data: &'a mut [u8]) -> Result<&'a mut [u8], Unspecified> {
        key.open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad), data)
    }
}

/// \[EXTENSION\] ChaCha20-Poly1305 [RFC 8439 §2.8], its nonces drawn at
/// random from aws-lc-rs's generator, as AES-256-GCM's are.
pub struct ChaCha;

impl ChaCha {
    /// `data` encrypted in place with `key` and `nonce`, the tag appended:
    /// a nonce given, for known answers [RFC 8439 §2.8.2]; `seal` draws
    /// one.
    pub(crate) fn seal_with(key: &LessSafeKey, nonce: NonceBytes, aad: &[u8], data: &mut Vec<u8>) -> Result<(), Unspecified> {
        key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad), data)
    }
}

impl Cipher for ChaCha {
    const ID: EncodingId = CHACHA_ID;
    const NAME: &'static str = CHACHA_NAME;
    const INFO: &'static [u8] = b"turborel ChaCha20-Poly1305 subkey";
    type Key = LessSafeKey;

    fn key(bytes: &[u8; KEY_BYTES]) -> Result<LessSafeKey, Unspecified> {
        UnboundKey::new(&CHACHA20_POLY1305, bytes).map(LessSafeKey::new)
    }

    fn seal(key: &LessSafeKey, aad: &[u8], data: &mut Vec<u8>) -> Result<NonceBytes, Unspecified> {
        let mut nonce = [0_u8; NONCE_LEN];
        aws_lc_rs::rand::fill(&mut nonce)?;
        ChaCha::seal_with(key, nonce, aad, data)?;
        Ok(nonce)
    }

    fn open<'a>(key: &LessSafeKey, nonce: NonceBytes, aad: &[u8], data: &'a mut [u8]) -> Result<&'a mut [u8], Unspecified> {
        key.open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad), data)
    }
}
