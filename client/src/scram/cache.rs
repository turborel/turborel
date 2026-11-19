//! \[DESIGN\] The SCRAM key cache (RULES.md D205). After an exchange the
//! server proved, the keys a password derived under the server's salt and
//! iterations, ClientKey and ServerKey, are kept in the process's memory,
//! so a connection made again to a server that sends the same salt and
//! iterations, as RFC 5802 §5.1 expects of one, skips PBKDF2, the slow
//! step of a login.
//!
//! An entry is found by the salt, the iterations and a tag of the
//! password: an HMAC of it under a secret of the cache's own, random, so
//! a wrong password finds nothing and the cache never holds a password
//! nor what a password could be guessed from offline. The keys a password
//! derives depend on the salt and iterations alone, not on the user or
//! the server, so neither is part of the index, a salt being a
//! verifier's, made at random. The cache holds `KEY_CACHE_ENTRIES` at most, the
//! oldest dropped first; an entry an exchange used is dropped when that
//! exchange fails. A cache is shared by its clones, across threads.

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use turborel_protocol::scram::{Derived, Key, hmac, nonce};

/// \[DESIGN\] The most entries a cache holds: a process's servers and
/// users, few, each of one salt at a time.
pub const KEY_CACHE_ENTRIES: usize = 64;

/// What an entry is found by.
#[derive(Clone, PartialEq, Eq)]
struct Index {
    tag: Key,
    salt: Vec<u8>,
    iterations: NonZeroU32,
}

/// The entries, oldest first, and what was looked up.
#[derive(Default)]
struct Entries {
    held: VecDeque<(Index, Derived)>,
    hits: u64,
    misses: u64,
}

/// A bounded cache of the keys passwords derived, shared by its clones.
#[derive(Clone)]
pub struct KeyCache {
    /// The secret its tags are made under; none if the system gave no
    /// randomness, and then nothing is kept.
    secret: Option<Arc<str>>,
    entries: Arc<Mutex<Entries>>,
}

impl std::fmt::Debug for KeyCache {
    /// Never the secret, nor the keys.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KeyCache({} entries)", self.len())
    }
}

impl Default for KeyCache {
    fn default() -> KeyCache {
        KeyCache::new()
    }
}

impl KeyCache {
    /// An empty cache, its secret new.
    pub fn new() -> KeyCache {
        KeyCache { secret: nonce().ok().map(Arc::from), entries: Arc::default() }
    }

    /// The entries, whatever a thread that panicked holding them left:
    /// each entry is whole, pushed or popped at once.
    fn entries(&self) -> MutexGuard<'_, Entries> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The index of `password`'s keys under `salt` and `iterations`.
    fn index(&self, password: &str, salt: &[u8], iterations: NonZeroU32) -> Option<Index> {
        let secret = self.secret.as_deref()?;
        Some(Index { tag: hmac(secret.as_bytes(), password.as_bytes()), salt: salt.to_vec(), iterations })
    }

    /// The keys `password` derives under `salt` and `iterations`: the
    /// cache's, or derived, outside the lock, and kept.
    pub(crate) fn derived(&self, password: &str, salt: &[u8], iterations: NonZeroU32) -> Derived {
        let Some(index) = self.index(password, salt, iterations) else { return Derived::of(password, salt, iterations) };
        {
            let mut entries = self.entries();
            let found = entries.held.iter().find(|(i, _)| *i == index).map(|(_, keys)| keys.clone());
            if let Some(keys) = found {
                entries.hits = entries.hits.saturating_add(1);
                return keys;
            }
            entries.misses = entries.misses.saturating_add(1);
        }
        let keys = Derived::of(password, salt, iterations);
        let mut entries = self.entries();
        if !entries.held.iter().any(|(i, _)| *i == index) {
            if entries.held.len() >= KEY_CACHE_ENTRIES {
                entries.held.pop_front();
            }
            entries.held.push_back((index, keys.clone()));
        }
        keys
    }

    /// Drops the keys of `password` under `salt` and `iterations`, as an
    /// exchange by them failed.
    pub(crate) fn forget(&self, password: &str, salt: &[u8], iterations: NonZeroU32) {
        if let Some(index) = self.index(password, salt, iterations) {
            self.entries().held.retain(|(i, _)| *i != index);
        }
    }

    /// How many entries it holds.
    pub fn len(&self) -> usize {
        self.entries().held.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many lookups found their keys, and how many derived them.
    pub fn lookups(&self) -> (u64, u64) {
        let entries = self.entries();
        (entries.hits, entries.misses)
    }

    /// Drops every entry.
    pub fn clear(&self) {
        self.entries().held.clear();
    }
}
