//! \[DESIGN\] An adjacency index built whole (RULES.md D34, D31): each
//! entry's keys, and a record of its place in each vertex's degree,
//! sorted in a spill area (`spill::sorted_records`), so no more of the
//! index is held than an area's bytes, and the tree loaded in key order
//! (`BLinkTree::load`), as inserting each would leave it. Keys alike, of
//! versions of a row under one key, are kept once, counted; a vertex's
//! degree records, which sort together, are one key counting the entries
//! they differ by. The compacted adjacency is built of the entries as they
//! pass (`csr.rs`), if the budget holds it.

use std::iter::Peekable;
use std::sync::Arc;

use super::AdjacencyIndex;
use super::csr::Builder;
use super::keys::{DEGREE, Encoded, degree_prefix, degree_record, degree_value, entry_key, parse, versions_value};
use super::options::Layout;
use crate::error::Result;
use crate::index::Key;
use crate::spill::{Area, Records, WorkAreas, sorted_records};
use crate::storage::RowId;

/// The records of one entry: its key and its degree's record in each
/// direction kept.
fn records_of(entry: Result<(Key, RowId)>, index: &AdjacencyIndex) -> Vec<Result<Vec<u8>>> {
    let parts = entry.and_then(|(key, row)| Ok((Encoded::of(&key, &index.layout)?, row)));
    match parts {
        Ok((parts, row)) => {
            index.keys.added(&parts.whole());
            let directions = index.layout.kept.directions().iter();
            directions
                .flat_map(|&d| {
                    let ends = parts.ends(d);
                    [Ok(entry_key(d, ends, row, &parts.included)), Ok(degree_record(d, ends, row, &parts.included))]
                })
                .collect()
        }
        Err(e) => vec![Err(e)],
    }
}

/// The tree's entries of sorted records: each entry's key once, its
/// versions counted, and each vertex's degree; the compacted adjacency
/// built of them as they pass, until it does not fit.
struct Loading<'a> {
    records: Peekable<Records>,
    layout: &'a Layout,
    builder: Option<Builder>,
}

impl Loading<'_> {
    /// The degree whose first record is `first`: its key, and the entries
    /// its records differ by.
    fn degree(&mut self, first: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        let prefix = degree_prefix(first, self.layout)?.to_vec();
        let (mut n, mut last) = (1_u64, first.to_vec());
        while let Some(next) = self.records.next_if(|r| r.as_ref().is_ok_and(|r| r.starts_with(&prefix))) {
            let next = next?;
            if next != last {
                n = n.saturating_add(1);
                last = next;
            }
        }
        Ok((prefix, degree_value(n)))
    }

    /// Adds the entry `entry` to the compacted adjacency, which is given
    /// up if it does not fit.
    fn built(&mut self, entry: &[u8]) -> Result<()> {
        if let Some(builder) = self.builder.as_mut() {
            let parsed = parse(entry, self.layout)?;
            if !builder.push(&parsed) {
                self.builder = None;
            }
        }
        Ok(())
    }
}

impl Iterator for Loading<'_> {
    type Item = Result<(Vec<u8>, Vec<u8>)>;

    fn next(&mut self) -> Option<Self::Item> {
        let first = match self.records.next()? {
            Ok(first) => first,
            Err(e) => return Some(Err(e)),
        };
        if first.first() == Some(&DEGREE) {
            return Some(self.degree(&first));
        }
        let mut versions = 1_u64;
        while self.records.next_if(|next| next.as_ref().is_ok_and(|n| *n == first)).is_some() {
            versions = versions.saturating_add(1);
        }
        Some(self.built(&first).map(|()| (first, versions_value(versions))))
    }
}

impl AdjacencyIndex {
    /// `load`: every entry's records sorted, then the tree loaded of them,
    /// and the compacted adjacency put in place, if it was built whole.
    ///
    /// # Errors
    ///
    /// As reading the entries; the spill area's; the tree's; SQLCODE -910
    /// if a lock is poisoned.
    pub(super) fn load_whole(
        &self,
        entries: &mut dyn Iterator<Item = Result<(Key, RowId)>>,
        (areas, area): (&WorkAreas, Area),
    ) -> Result<()> {
        let mut records = entries.flat_map(|entry| records_of(entry, self));
        let sorted = sorted_records(areas, area, &mut records)?;
        let builder = self.builder();
        let wanted = builder.is_some();
        let mut loading = Loading { records: sorted.peekable(), layout: &self.layout, builder };
        self.tree.load(&mut loading)?;
        if let Some(built) = loading.builder {
            self.delta.install(&Arc::new(built.finish()))?;
        } else if wanted {
            self.refuse();
        }
        Ok(())
    }
}
