//! \[DESIGN\] RULES.md D86: an XML index's dictionary, kept in its own
//! tree: each expanded name a token, each path an id, as Pal et al.
//! tokenize tags and paths [§3.1.1]. Ids are given under the index's lock,
//! from counters in the tree, and never taken back: a token or path id
//! given for a write rolled back stays, unused, so no entry can name an id
//! given twice. The dictionary is durable as the index's entries are, in
//! its tree, written back at a checkpoint and recovered with it.

use std::sync::Mutex;

use super::keys::{COUNTERS, Cursor, NAMES, PATH_IDS, PATHS, corrupt, ordinal_of, put_string};
use super::too_long;
use crate::engine::tree::{BLinkTree, Update};
use crate::error::Result;
use crate::sync::lock_builtin;
use crate::xml::QName;

/// The counter of names' tokens given.
const NAME_COUNTER: u8 = 0;
/// The counter of paths' ids given.
const PATH_COUNTER: u8 = 1;
/// Bytes of an id as a value of the tree.
const ID: usize = 8;

/// An index's dictionary: its tree, and the lock ids are given under.
pub(super) struct Dictionary<'t> {
    pub(super) tree: &'t BLinkTree,
    pub(super) lock: &'t Mutex<()>,
}

/// The key of expanded name `name`: its namespace URI, empty if it has
/// none, then its local name, each a string.
pub(super) fn name_key(name: &QName) -> Vec<u8> {
    let mut key = vec![NAMES];
    put_string(&mut key, name.uri.as_deref().unwrap_or_default().as_bytes(), true);
    put_string(&mut key, name.local.as_bytes(), true);
    key
}

/// The id a value of the tree holds.
pub(super) fn id_of(value: &[u8]) -> Result<u64> {
    value.get(..ID).and_then(|b| <[u8; ID]>::try_from(b).ok()).map(u64::from_le_bytes).ok_or_else(corrupt)
}

impl Dictionary<'_> {
    /// The token of `name`, given one if `give` and it has none.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if its key is longer than the tree's longest; the
    /// tree's.
    pub(super) fn name(&self, name: &QName, give: bool) -> Result<Option<u64>> {
        self.id(&name_key(name), (NAME_COUNTER, None), give)
    }

    /// The id of `path`, `[kind][tokens]`, given one if `give` and it has
    /// none.
    ///
    /// # Errors
    ///
    /// As `name`.
    pub(super) fn path(&self, path: &[u8], give: bool) -> Result<Option<u64>> {
        let key = [&[PATHS][..], path].concat();
        self.id(&key, (PATH_COUNTER, Some(path)), give)
    }

    /// The path of id `id`, `[kind][tokens]`.
    ///
    /// # Errors
    ///
    /// The tree's.
    pub(super) fn path_of(&self, id: u64) -> Result<Option<Vec<u8>>> {
        self.tree.get(&[&[PATH_IDS][..], &ordinal_of(id)].concat())
    }

    /// The id of `key`, given the next of counter `counter` if `give` and
    /// it has none, then also kept by id if it is a path's.
    fn id(&self, key: &[u8], (counter, path): (u8, Option<&[u8]>), give: bool) -> Result<Option<u64>> {
        if key.len() > self.tree.max_key() {
            return Err(too_long(key.len(), self.tree.max_key()));
        }
        if let Some(found) = self.tree.get(key)? {
            return id_of(&found).map(Some);
        }
        if !give {
            return Ok(None);
        }
        // Under the lock, so no two writers give one key two ids.
        let _given = lock_builtin(self.lock);
        if let Some(found) = self.tree.get(key)? {
            return id_of(&found).map(Some);
        }
        let mut next = 1_u64;
        self.tree.update(&[COUNTERS, counter], |old| {
            next = old.map_or(Ok(1), id_of)?;
            Ok(Update::Put(next.saturating_add(1).to_le_bytes().to_vec()))
        })?;
        if let Some(path) = path {
            self.tree.update(&[&[PATH_IDS][..], &ordinal_of(next)].concat(), |_| Ok(Update::Put(path.to_vec())))?;
        }
        self.tree.update(key, |_| Ok(Update::Put(next.to_le_bytes().to_vec())))?;
        Ok(Some(next))
    }
}

/// The tokens of `path`, `[kind][tokens]`, from its node up, and its kind.
///
/// # Errors
///
/// SQLCODE -916 if it is malformed.
pub(super) fn tokens_of(path: &[u8]) -> Result<(u8, Vec<u64>)> {
    let mut at = Cursor { bytes: path, at: 0 };
    let kind = at.byte()?;
    let mut tokens = Vec::new();
    while at.at < path.len() {
        tokens.push(at.ordinal()?);
    }
    Ok((kind, tokens))
}

// Unit tests: ids given once, kept, and found again (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
