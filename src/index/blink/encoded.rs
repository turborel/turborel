//! \[DESIGN\] (RULES.md D821) A search whose entries keep their keys as
//! the tree holds them. Each entry is lent from the scan, none copied
//! (RULES.md D809), its row id cut off and its key handed on as it is,
//! none decoded: the caller compares a row's key with it as bytes. A range
//! whose keys are each checked against the probe decodes them to check
//! them, so it is searched as `search` searches.

use std::ops::Bound;
use std::sync::Arc;

use super::entries::{Held, Keyed};
use super::range::after;
use super::value::{hint_of, noted};
use super::{BLinkIndex, split};
use crate::engine::key::anchor_into;
use crate::engine::tree::{Direction, Scan, Visit, hints, scan_owned};
use crate::error::Result;
use crate::index::{Encoded, EncodedEntries, EncodedEntry, KeyOrder, KeyProbe, Probe};
use crate::value::Value;

/// \[DESIGN\] (RULES.md D881) The most entries of a key a probe takes
/// from its tree at once, under a leaf's latch, before it gives them: a
/// unique key has one, or a few of versions awaiting collection.
const AT_ONCE: usize = 8;

impl KeyProbe for BLinkIndex {
    /// Each value's anchor (`engine/key.rs`), one after another, if `key`
    /// has a value for each column and each anchor is exact: the entries
    /// beginning with them are then those of keys equal to `key`, and no
    /// key read need be checked.
    fn whole_key(&self, key: &[Value], out: &mut Vec<u8>) -> bool {
        out.clear();
        let exact = |(value, data_type)| anchor_into(value, data_type, out) == Some(true);
        key.len() == self.types.len() && key.iter().zip(self.types.iter()).all(exact)
    }

    /// \[DESIGN\] (RULES.md D859) By a probe from the key borrowed
    /// (`BLinkTree::first_from`), which allocates nothing: each entry's
    /// row and hint are taken under the leaf's latch, `AT_ONCE` at most,
    /// and given once it is let go, as the tree is not to be used under
    /// it. Each entry's key is `key`, as they all begin with it. A key of
    /// more entries has the rest read by a scan past the last given.
    fn entries_of(&self, key: &[u8], each: &mut dyn FnMut(EncodedEntry<'_>) -> Result<bool>) -> Result<()> {
        let mut taken = [(0, 0); AT_ONCE];
        let mut count = 0_usize;
        self.tree.first_from(key, |entry, value| {
            let Some(place) = taken.get_mut(count).filter(|_| entry.starts_with(key)) else { return Ok(Visit::Stop) };
            *place = (split(entry)?.1, hint_of(value)?);
            count = count.saturating_add(1);
            Ok(Visit::Taken)
        })?;
        for &(row, hint) in taken.iter().take(count) {
            // Noted for the table's read of the row that follows (D500).
            hints::note(row, hint);
            if !each(EncodedEntry { key, row, types: &self.types })? {
                return Ok(());
            }
        }
        let Some(&(last, _)) = taken.last().filter(|_| count == AT_ONCE) else { return Ok(()) };
        let past = [key, last.to_be_bytes().as_slice()].concat();
        let mut scan = scan_owned(&self.tree, Bound::Excluded(past), after(key), Direction::Forward);
        while let Some(next) = scan.next_entry() {
            let (_, (entry, value)) = next?;
            let row = split(entry)?.1;
            noted(row, value)?;
            if !each(EncodedEntry { key, row, types: &self.types })? {
                break;
            }
        }
        Ok(())
    }
}

/// The entries `probe` selects of `index`, in `order`, their keys as the
/// tree holds them; none if a key read must be checked.
pub(super) fn searched(index: Arc<BLinkIndex>, probe: &Probe, order: KeyOrder) -> Option<Encoded> {
    let range = index.range(probe).filter(|range| !range.checked)?;
    let direction = match order {
        KeyOrder::Ascending => Direction::Forward,
        KeyOrder::Descending => Direction::Backward,
    };
    Some(Box::new(Lent(scan_owned(Held(index), range.lower, range.upper, direction))))
}

/// The scan of a search, its entries lent.
struct Lent(Scan<Held>);

impl EncodedEntries for Lent {
    fn next_encoded(&mut self) -> Option<Result<EncodedEntry<'_>>> {
        Some(self.0.next_entry()?.and_then(|(held, (entry, value))| {
            let (key, row) = split(entry)?;
            noted(row, value)?;
            Ok(EncodedEntry { key, row, types: held.types() })
        }))
    }
}
