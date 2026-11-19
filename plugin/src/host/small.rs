//! \[EXTENSION\] The host kinds a call or two serves (RULES.md D246,
//! D247, D252 to D254): encodings, checksums, the clock, package
//! cryptography, keyrings, served connections and jobs, each a trait
//! whose methods are its operations.

use turborel::bytes::{Put, Reader};
use turborel::plugin::manifest::HostKind;
use turborel::plugin::payload::hosted as h;

use super::super::registry::Plugin;
use super::super::serve::Cx;
use super::super::{Answer, PluginError};
use super::HostObject;

/// Kind 20, an encoding: each page and record the database writes passes
/// through `encode`, and back through `decode` as it is read. `context`
/// is unique to the page or record, for a cipher's nonce.
pub trait PageEncoding: Send {
    /// `page` encoded, as `argument`, the encoding's in STORAGE.ENCODINGS,
    /// says.
    ///
    /// # Errors
    ///
    /// The encoding's, which stops the node.
    fn encode(&mut self, context: u64, argument: &str, page: &[u8]) -> Answer<Vec<u8>>;
    /// `page` decoded.
    ///
    /// # Errors
    ///
    /// The page does not decode: the read fails, and the node goes on.
    fn decode(&mut self, context: u64, page: &[u8]) -> Answer<Vec<u8>>;
    /// Whether `argument` is one it takes. Default: any.
    ///
    /// # Errors
    ///
    /// It is not, said why.
    fn accepts(&mut self, _argument: &str) -> Answer<()> {
        Ok(())
    }
}

/// Kind 21, a checksum: the digest of the parts it is given, of the
/// LENGTH its declaration gives.
pub trait Checksum: Send {
    /// The digest of `parts`, as if joined.
    fn hash(&mut self, parts: &[Vec<u8>]) -> Vec<u8>;
}

/// Kind 22, a clock: the time, in microseconds since 1970 UTC.
pub trait TimeSource: Send {
    /// The time now.
    fn now(&mut self) -> i64;
}

/// Kind 29, package cryptography: SHA-256 and Ed25519.
pub trait Crypto: Send {
    /// The SHA-256 digest of `bytes`.
    fn sha256(&mut self, bytes: &[u8]) -> [u8; 32];
    /// Whether `signature` is `key`'s of `message`.
    fn verify(&mut self, key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool;
}

/// Kind 30, a keyring: 256-bit keys by name.
pub trait Keys: Send {
    /// The key of `name`, if it has one.
    ///
    /// # Errors
    ///
    /// The keyring's: the read that wanted the key fails.
    fn key(&mut self, name: &str) -> Answer<Option<[u8; 32]>>;
}

/// Kind 27, served connections: those the database asks ended or
/// cancelled, by id.
pub trait Connections: Send {
    /// Whether the connection `id` was ended.
    fn end(&mut self, id: u64) -> bool;
    /// Whether what the connection `id` runs was cancelled.
    fn cancel(&mut self, id: u64) -> bool;
}

/// Kind 31, a background job.
pub trait Job: Send {
    /// Does its work once: how many milliseconds after to run again, none
    /// to wait to be asked.
    ///
    /// # Errors
    ///
    /// The job's, which stops the node.
    fn run(&mut self) -> Answer<Option<u64>>;
}

/// An operation no kind has.
fn unknown(op: u8) -> PluginError {
    PluginError::external(&format!("no host operation {op}"))
}

/// A reader of a host call's payload.
fn reader(payload: &[u8]) -> Reader<'_> {
    Reader::received(payload, "host call")
}

/// A flag, as a byte.
fn flag(out: &mut Vec<u8>, on: bool) {
    out.put_u8(u8::from(on));
}

/// An encoding served.
struct Encoded<T>(T);

impl<T: PageEncoding> HostObject for Encoded<T> {
    fn answer(&mut self, _: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        let mut r = reader(payload);
        let mut out = Vec::new();
        match op {
            h::ENCODING_ENCODE => {
                let (context, argument, page) = (r.u64()?, r.str()?, r.bytes()?);
                out.put_bytes(&self.0.encode(context, &argument, page)?);
            }
            h::ENCODING_DECODE => {
                let (context, page) = (r.u64()?, r.bytes()?);
                out.put_bytes(&self.0.decode(context, page)?);
            }
            h::ENCODING_ACCEPTS => self.0.accepts(&r.str()?)?,
            other => return Err(unknown(other)),
        }
        Ok(out)
    }
}

/// A checksum served.
struct Hashed<T>(T);

impl<T: Checksum> HostObject for Hashed<T> {
    fn answer(&mut self, _: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        if op != h::HASH_HASH {
            return Err(unknown(op));
        }
        let parts = h::parts(&mut reader(payload))?;
        let mut out = Vec::new();
        out.put_bytes(&self.0.hash(&parts));
        Ok(out)
    }
}

/// A clock served.
struct Timed<T>(T);

impl<T: TimeSource> HostObject for Timed<T> {
    fn answer(&mut self, _: &mut Cx<'_>, op: u8, _: &[u8]) -> Answer<Vec<u8>> {
        if op != h::CLOCK_NOW {
            return Err(unknown(op));
        }
        let mut out = Vec::new();
        out.put_i128(i128::from(self.0.now()));
        Ok(out)
    }
}

/// Package cryptography served.
struct Crypted<T>(T);

impl<T: Crypto> HostObject for Crypted<T> {
    fn answer(&mut self, _: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        let mut r = reader(payload);
        let mut out = Vec::new();
        match op {
            h::CRYPTO_SHA256 => out.extend_from_slice(&self.0.sha256(r.bytes()?)),
            h::CRYPTO_VERIFY => {
                let key = <[u8; 32]>::try_from(r.take(32)?).map_err(|_| unknown(op))?;
                let message = r.bytes()?;
                let signature = <[u8; 64]>::try_from(r.take(64)?).map_err(|_| unknown(op))?;
                flag(&mut out, self.0.verify(&key, message, &signature));
            }
            other => return Err(unknown(other)),
        }
        Ok(out)
    }
}

/// A keyring served.
struct Keyed<T>(T);

impl<T: Keys> HostObject for Keyed<T> {
    fn answer(&mut self, _: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        if op != h::KEYRING_KEY {
            return Err(unknown(op));
        }
        let mut out = Vec::new();
        let key = self.0.key(&reader(payload).str()?)?;
        flag(&mut out, key.is_some());
        out.extend_from_slice(&key.unwrap_or_default());
        Ok(out)
    }
}

/// Served connections served.
struct Connected<T>(T);

impl<T: Connections> HostObject for Connected<T> {
    fn answer(&mut self, _: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        let id = reader(payload).u64()?;
        let mut out = Vec::new();
        match op {
            h::SERVED_END => flag(&mut out, self.0.end(id)),
            h::SERVED_CANCEL => flag(&mut out, self.0.cancel(id)),
            other => return Err(unknown(other)),
        }
        Ok(out)
    }
}

/// A job served.
struct Worked<T>(T);

impl<T: Job> HostObject for Worked<T> {
    fn answer(&mut self, _: &mut Cx<'_>, op: u8, _: &[u8]) -> Answer<Vec<u8>> {
        if op != h::JOB_RUN {
            return Err(unknown(op));
        }
        let again = self.0.run()?;
        let mut out = Vec::new();
        flag(&mut out, again.is_some());
        out.put_u64(again.unwrap_or(0));
        Ok(out)
    }
}

impl Plugin {
    /// It, serving `encoding` as HOST ENCODING `name`.
    #[must_use]
    pub fn host_encoding(self, name: &str, encoding: impl PageEncoding + 'static) -> Plugin {
        self.host(HostKind::Encoding.code(), name, Encoded(encoding))
    }
    /// It, serving `checksum` as HOST HASH FUNCTION `name`.
    #[must_use]
    pub fn host_checksum(self, name: &str, checksum: impl Checksum + 'static) -> Plugin {
        self.host(HostKind::HashFunction.code(), name, Hashed(checksum))
    }
    /// It, serving `clock` as HOST CLOCK `name`.
    #[must_use]
    pub fn host_clock(self, name: &str, clock: impl TimeSource + 'static) -> Plugin {
        self.host(HostKind::Clock.code(), name, Timed(clock))
    }
    /// It, serving `crypto` as HOST PACKAGE CRYPTO `name`.
    #[must_use]
    pub fn host_crypto(self, name: &str, crypto: impl Crypto + 'static) -> Plugin {
        self.host(HostKind::PackageCrypto.code(), name, Crypted(crypto))
    }
    /// It, serving `keys` as HOST KEYRING `name`.
    #[must_use]
    pub fn host_keyring(self, name: &str, keys: impl Keys + 'static) -> Plugin {
        self.host(HostKind::Keyring.code(), name, Keyed(keys))
    }
    /// It, serving `connections` as HOST SERVED CONNECTIONS `name`.
    #[must_use]
    pub fn host_connections(self, name: &str, connections: impl Connections + 'static) -> Plugin {
        self.host(HostKind::ServedConnections.code(), name, Connected(connections))
    }
    /// It, serving `job` as HOST BACKGROUND JOB `name`.
    #[must_use]
    pub fn host_job(self, name: &str, job: impl Job + 'static) -> Plugin {
        self.host(HostKind::BackgroundJob.code(), name, Worked(job))
    }
}
