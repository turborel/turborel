//! \[EXTENSION\] The cryptography of packages (design document §13.4):
//! SHA-256 digests and Ed25519 signatures. The core declares it and the
//! protocol crate gives it (`turborel_protocol::packages`), as the
//! cryptography library is used by that crate, the server's and the
//! encryption crate alone (RULES.md D26).

/// The bytes of a SHA-256 digest.
pub const DIGEST_LEN: usize = 32;
/// The bytes of an Ed25519 public key.
pub const KEY_LEN: usize = 32;
/// The bytes of an Ed25519 signature.
pub const SIGNATURE_LEN: usize = 64;

/// A SHA-256 digest.
pub type Digest = [u8; DIGEST_LEN];
/// An Ed25519 public key.
pub type PublicKey = [u8; KEY_LEN];
/// An Ed25519 signature.
pub type Signature = [u8; SIGNATURE_LEN];

/// The digest and signature checks a package needs.
pub trait PackageCrypto: Send + Sync {
    /// The SHA-256 digest of `bytes`.
    fn sha256(&self, bytes: &[u8]) -> Digest;
    /// Whether `signature` is `key`'s Ed25519 signature of `message`.
    fn verify(&self, key: &PublicKey, message: &[u8], signature: &Signature) -> bool;
}

/// What signs a package: an Ed25519 private key, held where
/// the cryptography is (`turborel_protocol::packages::SigningKey`).
pub trait Signer {
    /// Its public key.
    fn public_key(&self) -> PublicKey;
    /// Its signature of `message`.
    fn sign(&self, message: &[u8]) -> Signature;
}
