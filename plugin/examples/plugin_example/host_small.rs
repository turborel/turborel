//! The example's host objects a call or two serves (RULES.md D229): the
//! encoding XOR, the checksum XXH64, the clock SYSTEM, the keyring DERIVED,
//! the package cryptography DIGESTS, the served connections NOTED and the
//! job TICK. A file `crash.<what>` in the plug-in's directory makes the
//! process exit as it does `what`, for the tests that kill it at bad
//! moments: `crash.encode` as a page is written.

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use turborel_plugin::host::{Checksum, Connections, Crypto, Job, Keys, PageEncoding, TimeSource};
use turborel_plugin::{Answer, Plugin, PluginError};

/// Dies, as a process killed, if the file `crash.<what>` is there.
pub fn crash_if(what: &str) {
    if Path::new(&format!("crash.{what}")).exists() {
        std::process::abort();
    }
}

/// The plug-in, with these objects.
pub fn with(plugin: Plugin) -> Plugin {
    plugin
        .host_encoding("XOR", Xor)
        .host_checksum("XXH64", Xxh64Sum)
        .host_clock("SYSTEM", System)
        .host_keyring("DERIVED", Derived)
        .host_crypto("DIGESTS", Digests)
        .host_connections("NOTED", Noted)
        .host_job("TICK", Tick)
}

/// XXH64 of `parts`, as if joined, seed 0.
pub fn xxh64_of(parts: &[&[u8]]) -> u64 {
    let mut state = turborel::hash::xxhash::Xxh64::new(0);
    for part in parts {
        state.update(part);
    }
    state.digest()
}

/// Each byte XORed with its argument's first byte, 0x5A if it has none:
/// an encoding that shows the pages pass through it, and keeps nothing.
struct Xor;

/// The byte XOR takes of `argument`.
fn mask(argument: &str) -> u8 {
    argument.bytes().next().unwrap_or(0x5A)
}

impl PageEncoding for Xor {
    fn encode(&mut self, _context: u64, argument: &str, page: &[u8]) -> Answer<Vec<u8>> {
        crash_if("encode");
        let m = mask(argument);
        let mut out = vec![m];
        out.extend(page.iter().map(|b| b ^ m));
        Ok(out)
    }
    fn decode(&mut self, _context: u64, page: &[u8]) -> Answer<Vec<u8>> {
        let (m, rest) = page.split_first().ok_or_else(|| PluginError::data("an empty page is no page XOR wrote"))?;
        Ok(rest.iter().map(|b| b ^ m).collect())
    }
}

/// XXH64, 8 bytes, big-endian, xxHash's canonical form.
struct Xxh64Sum;

impl Checksum for Xxh64Sum {
    fn hash(&mut self, parts: &[Vec<u8>]) -> Vec<u8> {
        let parts: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        xxh64_of(&parts).to_be_bytes().to_vec()
    }
}

/// The system's time.
struct System;

impl TimeSource for System {
    fn now(&mut self) -> i64 {
        let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        i64::try_from(since.as_micros()).unwrap_or(i64::MAX)
    }
}

/// A key derived of its name: for showing the keyring's calls, never for
/// keeping anything secret.
struct Derived;

impl Keys for Derived {
    fn key(&mut self, name: &str) -> Answer<Option<[u8; 32]>> {
        let mut key = [0_u8; 32];
        for (i, chunk) in key.chunks_mut(8).enumerate() {
            let salt = u8::try_from(i).unwrap_or(0);
            chunk.copy_from_slice(&xxh64_of(&[name.as_bytes(), &[salt]]).to_le_bytes());
        }
        Ok(Some(key))
    }
}

/// Package cryptography that digests and trusts no signature: packages
/// install by the digests their operator pins, never by a key.
struct Digests;

impl Crypto for Digests {
    fn sha256(&mut self, bytes: &[u8]) -> [u8; 32] {
        super::sha256::digest(bytes)
    }
    fn verify(&mut self, _key: &[u8; 32], _message: &[u8], _signature: &[u8; 64]) -> bool {
        false
    }
}

/// Served connections it ends and cancels none of, noting each asked in
/// `served.log`.
struct Noted;

/// Notes `line` in `file`.
pub fn note(file: &str, line: &str) {
    let opened = std::fs::OpenOptions::new().create(true).append(true).open(file);
    if let Ok(mut f) = opened {
        drop(writeln!(f, "{line}"));
    }
}

impl Connections for Noted {
    fn end(&mut self, id: u64) -> bool {
        note("served.log", &format!("end {id}"));
        false
    }
    fn cancel(&mut self, id: u64) -> bool {
        note("served.log", &format!("cancel {id}"));
        false
    }
}

/// A job noting each run in `tick.log`, again 100 ms after.
struct Tick;

impl Job for Tick {
    fn run(&mut self) -> Answer<Option<u64>> {
        crash_if("tick");
        note("tick.log", "tick");
        Ok(Some(100))
    }
}
