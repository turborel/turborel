//! \[DESIGN\] Owed removals: the index entries of versions collection
//! removed that an index failed to take back (RULES.md D34). Each
//! version's are kept on its table's lock and tried again by the table's
//! next collections (`State::collect_table`). A checkpoint logs each as it
//! logs the entries of what awaits collection, so recovery takes it back
//! if the database closes or crashes first (`db/checkpoint.rs`). While a
//! version's are owed, the table's engine keeps the row's zone from being
//! marked visible to every reader, until the last is taken back
//! (`TableStorage::taken_back`), so no index-only read gives an entry's
//! key for the row's. A blind second removal would not do:
//! `Index::remove` takes back one entry of several alike, and the row's
//! newest version may have one under the same key.

use std::sync::{Arc, Mutex};

use crate::error::Result;
use crate::index::{Index, Key};
use crate::storage::{RowId, TableStorage};
use crate::sync::lock_builtin;

/// An entry still to be taken back: under `key` in the index named
/// `name`, `index`.
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) index: Arc<dyn Index>,
    pub(crate) key: Key,
}

/// The entries still to be taken back of a version of `row`, of the table
/// whose storage is `storage`.
pub(crate) struct Owed {
    pub(crate) row: RowId,
    pub(crate) storage: Arc<dyn TableStorage>,
    pub(crate) entries: Vec<Entry>,
}

/// The removals a table owes.
#[derive(Default)]
pub(crate) struct OwedRemovals {
    owed: Mutex<Vec<Owed>>,
}

impl OwedRemovals {
    /// Adds `owed`.
    pub(crate) fn add(&self, owed: Owed) {
        lock_builtin(&self.owed).push(owed);
    }

    /// Tries each removal again, keeping those that fail still; one of an
    /// index `current` says is no longer the table's, dropped, is owed no
    /// more. A version owing none now lets its engine mark the row's zone
    /// visible again (`TableStorage::taken_back`).
    pub(crate) fn retry(&self, current: &dyn Fn(&str, &Arc<dyn Index>) -> bool) {
        let mut owed = lock_builtin(&self.owed);
        owed.retain_mut(|version| {
            let row = version.row;
            version.entries.retain(|e| current(&e.name, &e.index) && e.index.remove(&e.key, row).is_err());
            if version.entries.is_empty() {
                version.storage.taken_back(row);
            }
            !version.entries.is_empty()
        });
    }

    /// Whether none is owed.
    pub(crate) fn is_empty(&self) -> bool {
        lock_builtin(&self.owed).is_empty()
    }

    /// Calls `each` with each removal owed, the index's name, the key and
    /// the row, until it fails: its error.
    pub(crate) fn each(&self, each: &mut dyn FnMut(&str, &Key, RowId) -> Result<()>) -> Result<()> {
        for version in lock_builtin(&self.owed).iter() {
            for entry in &version.entries {
                each(&entry.name, &entry.key, version.row)?;
            }
        }
        Ok(())
    }
}
