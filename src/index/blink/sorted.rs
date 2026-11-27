//! \[DESIGN\] A group's entries put in key order (RULES.md D1235): each
//! encoded as `insert_from` encodes it, sorted, and written a leaf's run
//! at a time (`BLinkTree::update_sorted`), each alone or not as
//! `insert_from` says of it.

use super::value::{count_of, hint_for, put_value_of};
use super::{BLinkIndex, ROW_ID};
use crate::error::Result;
use crate::index::Key;
use crate::storage::RowId;

impl BLinkIndex {
    /// `insert_from` of each of `entries`, in key order: whether each, in
    /// the order given, was alone.
    pub(super) fn sorted(&self, entries: &[(Key, RowId)]) -> Result<Vec<bool>> {
        let encoded = entries.iter().map(|(key, row)| self.entry(key, *row)).collect::<Result<Vec<_>>>()?;
        let mut order: Vec<usize> = (0..entries.len()).collect();
        order.sort_unstable_by(|&a, &b| encoded.get(a).cmp(&encoded.get(b)));
        let keys: Vec<(&[u8], usize)> =
            order.iter().filter_map(|&at| encoded.get(at)).map(|entry| (entry.as_slice(), entry.len().saturating_sub(ROW_ID))).collect();
        let mut change = |n: usize, old: Option<&[u8]>, into: &mut Vec<u8>| {
            let (Some(&at), Some(&(entry, prefix))) = (order.get(n), keys.get(n)) else { return Ok(()) };
            let row = entries.get(at).map_or(0, |e| e.1);
            // The key's bytes are the same for equal keys (`engine/key.rs`).
            self.keys.added(entry.get(..prefix).unwrap_or_default());
            let count = old.map_or(Ok(0), count_of)?;
            put_value_of(into, count.saturating_add(1), hint_for(row, old)?);
            Ok(())
        };
        let lone = self.tree.update_sorted(&keys, &mut change)?;
        let mut alone = vec![false; entries.len()];
        for (&at, was) in order.iter().zip(lone) {
            if let Some(slot) = alone.get_mut(at) {
                *slot = was;
            }
        }
        Ok(alone)
    }
}
