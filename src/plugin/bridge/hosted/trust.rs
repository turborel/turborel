//! \[EXTENSION\] A host plug-in's package cryptography, keys and served
//! connections (RULES.md D252, D254).
//!
//! Package cryptography checks the signatures of packages, so it cannot
//! be vouched for by a package. PLUGINS.HOST pins its program's digest,
//! which the host's own cryptography checks as each process starts
//! (`plugin::hosted`). A keyring gives a key by name, which encryption
//! keeps for as long as it reads with it (`turborel-aes` serves its
//! `Keyring` so). Served connections are ended and cancelled by id.

use crate::bytes::Put;
use crate::db::ServedConnections;
use crate::error::Result;
use crate::plugin::bridge::{answer, byte, leaked};
use crate::plugin::errors::violation;
use crate::plugin::hosted::HostLink;
use crate::plugin::manifest::HostDeclared;
use crate::plugin::package::crypto::{DIGEST_LEN, Digest, KEY_LEN, PackageCrypto, PublicKey, SIGNATURE_LEN, Signature};
use crate::plugin::payload::hosted::{CRYPTO_SHA256, CRYPTO_VERIFY, KEYRING_KEY, SERVED_CANCEL, SERVED_END};

/// The bytes of a key.
pub const KEY_BYTES: usize = 32;

/// A host plug-in's package cryptography.
#[derive(Debug)]
pub struct PluginCrypto {
    link: HostLink,
}

impl PluginCrypto {
    /// The cryptography of `link`.
    pub fn new(link: HostLink) -> PluginCrypto {
        PluginCrypto { link }
    }

    /// The digest of `bytes` the program gives.
    fn digest(&self, bytes: &[u8]) -> Result<Digest> {
        let mut payload = Vec::new();
        payload.put_bytes(bytes);
        let got = answer(&self.link.call(CRYPTO_SHA256, payload)?, |r| r.take(DIGEST_LEN).map(<[u8]>::to_vec))?;
        Digest::try_from(got.as_slice()).map_err(|_| violation("a SHA-256 digest is not of 32 bytes"))
    }
}

impl PackageCrypto for PluginCrypto {
    fn sha256(&self, bytes: &[u8]) -> Digest {
        // A digest the program cannot give stops the node; the zeros given
        // in its place match no package's.
        self.digest(bytes).unwrap_or_else(|e| {
            self.link.plugin().stop.stop(&e.msg);
            [0; DIGEST_LEN]
        })
    }
    fn verify(&self, key: &PublicKey, message: &[u8], signature: &Signature) -> bool {
        let mut payload = Vec::with_capacity(message.len().saturating_add(KEY_LEN).saturating_add(SIGNATURE_LEN).saturating_add(4));
        payload.extend_from_slice(key);
        payload.put_bytes(message);
        payload.extend_from_slice(signature);
        // Not answered is not verified.
        self.link.call(CRYPTO_VERIFY, payload).and_then(|got| answer(&got, byte)).is_ok_and(|v| v == 1)
    }
}

/// A host plug-in's keyring.
#[derive(Debug)]
pub struct PluginKeys {
    link: HostLink,
    name: &'static str,
}

impl PluginKeys {
    /// The keyring `d` declares.
    pub fn new(d: &HostDeclared, link: HostLink) -> PluginKeys {
        PluginKeys { link, name: leaked(&d.name) }
    }

    /// Its name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The key of `name`, if the program has one.
    ///
    /// # Errors
    ///
    /// The program's error; SQLSTATE 38T11 if its process failed.
    pub fn key(&self, name: &str) -> Result<Option<[u8; KEY_BYTES]>> {
        let mut payload = Vec::new();
        payload.put_str(name);
        let got = self.link.ask(KEYRING_KEY, payload)?;
        answer(&got, |r| {
            if r.u8()? == 0 {
                return Ok(None);
            }
            let key = r.take(KEY_BYTES)?;
            Ok(<[u8; KEY_BYTES]>::try_from(key).ok())
        })
    }
}

/// A host plug-in's served connections.
#[derive(Debug)]
pub struct PluginServed {
    link: HostLink,
    name: &'static str,
}

impl PluginServed {
    /// The served connections `d` declares.
    pub fn new(d: &HostDeclared, link: HostLink) -> PluginServed {
        PluginServed { link, name: leaked(&d.name) }
    }

    /// Whether the program did `op` of the connection `id`.
    fn did(&self, op: u8, id: u64) -> bool {
        let mut payload = Vec::new();
        payload.put_u64(id);
        self.link.call(op, payload).and_then(|got| answer(&got, byte)).is_ok_and(|v| v == 1)
    }
}

impl ServedConnections for PluginServed {
    fn name(&self) -> &'static str {
        self.name
    }
    fn end(&self, id: u64) -> bool {
        self.did(SERVED_END, id)
    }
    fn cancel(&self, id: u64) -> bool {
        self.did(SERVED_CANCEL, id)
    }
}
