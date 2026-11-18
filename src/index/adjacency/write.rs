//! \[DESIGN\] An adjacency index's entries written and taken back (RULES.md
//! D29, D34): an entry's count of versions changes in one descent, under
//! its leaf's lock (`BLinkTree::update`), so writers of one entry never
//! lose each other's; the insert that makes the entry counts one more in
//! its vertex's degree, and the remove that takes its last version one
//! fewer. Each direction kept has its entry, and the vertex written is
//! marked in the delta once its entry is (`delta.rs`).

use super::AdjacencyIndex;
use super::keys::{Encoded, degree_key, degree_of, degree_value, entry_key, versions_of, versions_value};
use crate::engine::tree::Update;
use crate::error::{Code, Result, SqlError};
use crate::graph::adjacency::Direction;
use crate::index::Key;
use crate::storage::RowId;

/// A key's parts, and the keys of its entries, with the direction of each.
type Entries = (Encoded, Vec<(Direction, Vec<u8>)>);

impl AdjacencyIndex {
    /// The entries' keys of `row` under `key`, one for each direction
    /// kept, with the vertex each is under, all checked to fit a node
    /// before any is written.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `key` is not of the layout; SQLCODE -900 if an
    /// entry would not fit a node.
    fn entries(&self, key: &Key, row: RowId) -> Result<Entries> {
        let parts = Encoded::of(key, &self.layout)?;
        let most = self.tree.max_key();
        let mut entries = Vec::new();
        for &direction in self.layout.kept.directions() {
            let entry = entry_key(direction, parts.ends(direction), row, &parts.included);
            if entry.len() > most {
                return Err(SqlError::new(
                    Code::Unsupported,
                    format!(
                        "[DESIGN] an adjacency index entry of {} bytes is longer than the limit of {most} bytes for this database's page size; a database created with a larger STORAGE.PAGE_BYTES holds longer entries",
                        entry.len()
                    ),
                ));
            }
            entries.push((direction, entry));
        }
        Ok((parts, entries))
    }

    /// Adds an entry of `row` under `key`: one more version of each of its
    /// entries, counted in its vertex's degree if it is the first.
    ///
    /// # Errors
    ///
    /// As `entries`; the tree's; SQLCODE -910 if a lock is poisoned.
    pub(super) fn add(&self, key: &Key, row: RowId) -> Result<()> {
        let (parts, entries) = self.entries(key, row)?;
        for (direction, entry) in entries {
            let mut first = false;
            self.tree.update(&entry, |old| {
                let n = old.map(versions_of).transpose()?.unwrap_or(0);
                first = n == 0;
                Ok(Update::Put(versions_value(n.saturating_add(1))))
            })?;
            let vertex = parts.ends(direction).0;
            if first {
                self.counted(direction, vertex, true)?;
            }
            self.delta.mark(direction, vertex)?;
        }
        self.keys.added(&parts.whole());
        Ok(())
    }

    /// Takes back one entry of `row` under `key`, if there is one: one
    /// version of each of its entries, its vertex's degree one fewer with
    /// the last.
    ///
    /// # Errors
    ///
    /// As `add`.
    pub(super) fn take_back(&self, key: &Key, row: RowId) -> Result<()> {
        let (parts, entries) = self.entries(key, row)?;
        let mut taken = false;
        for (direction, entry) in entries {
            let mut last = false;
            self.tree.update(&entry, |old| {
                let Some(n) = old.map(versions_of).transpose()? else { return Ok(Update::Keep) };
                (taken, last) = (true, n <= 1);
                Ok(if last { Update::Remove } else { Update::Put(versions_value(n.saturating_sub(1))) })
            })?;
            let vertex = parts.ends(direction).0;
            if last {
                self.counted(direction, vertex, false)?;
            }
            self.delta.mark(direction, vertex)?;
        }
        if taken {
            self.keys.removed();
        }
        Ok(())
    }

    /// Counts one more entry under `vertex` in `direction`, or one fewer.
    ///
    /// # Errors
    ///
    /// The tree's.
    fn counted(&self, direction: Direction, vertex: &[u8], more: bool) -> Result<()> {
        self.tree.update(&degree_key(direction, vertex), |old| {
            let n = old.map(degree_of).transpose()?.unwrap_or(0);
            let next = if more { n.saturating_add(1) } else { n.saturating_sub(1) };
            Ok(if next == 0 { Update::Remove } else { Update::Put(degree_value(next)) })
        })
    }
}
