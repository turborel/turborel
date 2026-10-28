//! \[EXTENSION\] AES-256-GCM encryption for turborel, as an encoding
//! (RULES.md D26, D30, D48), on aws-lc-rs: registered with a database
//! (`Database::register_encoding`), it is named with its key, `AES-256-GCM:
//! NAME`, by STORAGE.ENCODINGS for every file, or by a table's option
//! ENCODINGS for its rows and their log records. Keys come from a
//! `Keyring` its host gives it, never from the database's files.
//!
//! **Keys used.** AES-GCM's nonces must never repeat under one key, and
//! aws-lc-rs's FIPS rules draw them at random, which is safe for 2^32
//! encryptions a key at most; a database rewrites its pages more often
//! than that over its life. So no key is used so often: a page is
//! encrypted with a subkey, HKDF-SHA256 [RFC 5869] of the named key and a
//! random salt, the salt drawn anew after `SUBKEY_USES` encryptions, and
//! kept in what is encrypted, so a reader derives the same subkey.
//!
//! ```text
//! encrypted:  VERSION u8 | name length u8 | the key's name | salt, 16 bytes
//!             | nonce, 12 bytes | the input encrypted | tag, 16 bytes
//! ```
//!
//! The authenticated data is the context (where the data is stored, as the
//! encoding contract says) and the key's name: data moved to another place,
//! or another key named, fails to decrypt (SQLCODE -916), as does any byte
//! changed.
//!
//! **No key named** (`Encoding::encode`, as a spill area's files are
//! written): the process's ephemeral key, drawn as it is first used and
//! kept only in memory, its name empty; what it encrypts no later process
//! reads. A table, or a database's files, always name a key: `accepts`
//! refuses none.

mod keyring;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use aws_lc_rs::aead::{AES_256_GCM, Aad, NONCE_LEN, Nonce, RandomizedNonceKey};
use aws_lc_rs::hkdf::{HKDF_SHA256, KeyType, Salt};
pub use keyring::{KEY_BYTES, KeyDirectory, Keyring, MAX_KEY_NAME, valid_key_name};
use turborel::disk::encoding::{Encoding, EncodingId};
use turborel::error::{Code, Result, SqlError};

/// \[EXTENSION\] Its id in file formats: never changed or reused (RULES.md
/// D48).
pub const AES_ID: EncodingId = EncodingId(2);
/// Its name.
pub const AES_NAME: &str = "AES-256-GCM";
/// The format of what it writes.
const VERSION: u8 = 1;
/// Bytes of a salt.
const SALT: usize = 16;
/// Bytes of a tag.
const TAG: usize = 16;
/// \[DESIGN\] Encryptions with one subkey before a new salt is drawn: far
/// fewer than the 2^32 random nonces a key bears.
pub const SUBKEY_USES: u64 = 1 << 30;
/// The name of the process's ephemeral key: no key's name.
const EPHEMERAL: &str = "";
/// What HKDF derives a subkey for.
const INFO: &[u8] = b"turborel AES-256-GCM subkey";

/// A subkey's length, as HKDF takes it.
struct Length;

impl KeyType for Length {
    fn len(&self) -> usize {
        KEY_BYTES
    }
}

/// A subkey: its key's name, and its salt.
type Subkey = (String, [u8; SALT]);

/// The subkey a key's writes use now.
struct Current {
    salt: [u8; SALT],
    key: Arc<RandomizedNonceKey>,
    uses: u64,
}

/// \[EXTENSION\] AES-256-GCM, named "AES-256-GCM", its keys from a keyring.
pub struct AesGcm {
    keyring: Arc<dyn Keyring>,
    /// The subkeys derived, by key and salt.
    derived: RwLock<HashMap<Subkey, Arc<RandomizedNonceKey>>>,
    /// The subkey each key's writes use now.
    current: Mutex<HashMap<String, Current>>,
    /// The process's ephemeral key, once drawn.
    ephemeral: Mutex<Option<[u8; KEY_BYTES]>>,
}

fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[EXTENSION] {AES_NAME} data is malformed: {why}"))
}

fn failed(what: &str) -> SqlError {
    SqlError::new(Code::Io, format!("[EXTENSION] {AES_NAME}: {what} failed"))
}

/// The authenticated data of what is stored at `context` with `name`.
fn aad(context: u64, name: &str) -> Vec<u8> {
    let mut aad = context.to_le_bytes().to_vec();
    aad.extend_from_slice(name.as_bytes());
    aad
}

impl AesGcm {
    /// AES-256-GCM, with the keys of `keyring`.
    pub fn new(keyring: Arc<dyn Keyring>) -> AesGcm {
        AesGcm { keyring, derived: RwLock::default(), current: Mutex::default(), ephemeral: Mutex::default() }
    }

    /// The key named `name`, from the keyring; the process's ephemeral
    /// key, drawn as it is first used, if `name` is empty.
    fn master(&self, name: &str) -> Result<[u8; KEY_BYTES]> {
        if name.is_empty() {
            return self.ephemeral();
        }
        self.keyring
            .key(name)?
            .ok_or_else(|| SqlError::new(Code::Api, format!("[EXTENSION] the keyring ({}) has no key {name}", self.keyring.name())))
    }

    /// The process's ephemeral key: drawn once, kept in memory only.
    fn ephemeral(&self) -> Result<[u8; KEY_BYTES]> {
        let mut held = self.ephemeral.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(key) = *held {
            return Ok(key);
        }
        let mut key = [0_u8; KEY_BYTES];
        aws_lc_rs::rand::fill(&mut key).map_err(|_| failed("drawing an ephemeral key"))?;
        *held = Some(key);
        Ok(key)
    }

    /// The subkey of `name` and `salt`, derived once.
    fn subkey(&self, name: &str, salt: [u8; SALT]) -> Result<Arc<RandomizedNonceKey>> {
        let known = self.derived.read().unwrap_or_else(PoisonError::into_inner).get(&(name.to_owned(), salt)).map(Arc::clone);
        if let Some(key) = known {
            return Ok(key);
        }
        let master = self.master(name)?;
        let mut bytes = [0_u8; KEY_BYTES];
        let info = [INFO];
        let prk = Salt::new(HKDF_SHA256, &salt).extract(&master);
        prk.expand(&info, Length).and_then(|okm| okm.fill(&mut bytes)).map_err(|_| failed("deriving a subkey"))?;
        let key = Arc::new(RandomizedNonceKey::new(&AES_256_GCM, &bytes).map_err(|_| failed("making a key"))?);
        self.derived.write().unwrap_or_else(PoisonError::into_inner).insert((name.to_owned(), salt), Arc::clone(&key));
        Ok(key)
    }

    /// The salt and subkey a write with `name` uses: a new salt once the
    /// last was used `SUBKEY_USES` times.
    fn for_writing(&self, name: &str) -> Result<([u8; SALT], Arc<RandomizedNonceKey>)> {
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(now) = current.get_mut(name).filter(|now| now.uses < SUBKEY_USES) {
            now.uses = now.uses.saturating_add(1);
            return Ok((now.salt, Arc::clone(&now.key)));
        }
        let mut salt = [0_u8; SALT];
        aws_lc_rs::rand::fill(&mut salt).map_err(|_| failed("drawing a salt"))?;
        let key = self.subkey(name, salt)?;
        current.insert(name.to_owned(), Current { salt, key: Arc::clone(&key), uses: 1 });
        Ok((salt, key))
    }
}

impl Encoding for AesGcm {
    fn id(&self) -> EncodingId {
        AES_ID
    }

    fn name(&self) -> &'static str {
        AES_NAME
    }

    /// The version, the name's length and the name, the salt, the nonce
    /// and the tag.
    fn overhead(&self, argument: &str) -> usize {
        argument.len().saturating_add(2 + SALT + NONCE_LEN + TAG)
    }

    fn confidential(&self) -> bool {
        true
    }

    /// The name of a key the keyring has.
    fn accepts(&self, argument: &str) -> Result<()> {
        if !valid_key_name(argument) {
            return Err(SqlError::new(
                Code::Api,
                format!(
                    "[EXTENSION] {AES_NAME} names its key, AES-256-GCM:NAME, of 1 to {MAX_KEY_NAME} letters, digits, _, - or ., not {argument:?}"
                ),
            ));
        }
        self.master(argument).map(|_| ())
    }

    /// With no key named: the process's ephemeral key, for data no later
    /// process reads (a spill area's files).
    fn encode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        self.encrypt(context, EPHEMERAL, input, out)
    }

    fn encode_with(&self, context: u64, argument: &str, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        if !valid_key_name(argument) {
            return self.accepts(argument);
        }
        self.encrypt(context, argument, input, out)
    }

    fn decode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let Some((&VERSION, rest)) = input.split_first() else { return Err(corrupt("it is of no version known")) };
        let (&length, rest) = rest.split_first().ok_or_else(|| corrupt("it is too short"))?;
        let (name, rest) = rest.split_at_checked(usize::from(length)).ok_or_else(|| corrupt("it is too short"))?;
        let named = |n: &&str| n.is_empty() || valid_key_name(n);
        let name = std::str::from_utf8(name).ok().filter(named).ok_or_else(|| corrupt("it names no key"))?;
        let (salt, rest) = rest.split_first_chunk::<SALT>().ok_or_else(|| corrupt("it is too short"))?;
        let (nonce, sealed) = rest.split_first_chunk::<NONCE_LEN>().ok_or_else(|| corrupt("it is too short"))?;
        let key = self.subkey(name, *salt)?;
        let mut opened = sealed.to_vec();
        let plain = key
            .open_in_place(Nonce::assume_unique_for_key(*nonce), Aad::from(aad(context, name)), &mut opened)
            .map_err(|_| corrupt("it fails its authentication: changed, moved, or of another key"))?;
        out.extend_from_slice(plain);
        Ok(())
    }
}

impl AesGcm {
    /// `input`, at `context`, encrypted with a subkey of the key `name`,
    /// the ephemeral key if empty, appended to `out`.
    fn encrypt(&self, context: u64, name: &str, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let (salt, key) = self.for_writing(name)?;
        let mut sealed = input.to_vec();
        let nonce = key.seal_in_place_append_tag(Aad::from(aad(context, name)), &mut sealed).map_err(|_| failed("encrypting"))?;
        out.push(VERSION);
        out.push(u8::try_from(name.len()).unwrap_or(u8::MAX));
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&salt);
        out.extend_from_slice(nonce.as_ref());
        out.extend_from_slice(&sealed);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
