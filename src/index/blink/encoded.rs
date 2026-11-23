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
use super::value::noted;
use super::{BLinkIndex, KEY_VALUE_ROOM, split};
use crate::engine::key::anchor_into;
use crate::engine::tree::{Direction, Scan, scan_owned};
use crate::error::Result;
use crate::index::{Encoded, EncodedEntries, EncodedEntry, KeyOrder, KeyProbe, Probe};
use crate::value::Value;

impl KeyProbe for BLinkIndex {
    /// Each value's anchor (`engine/key.rs`), one after another, if `key`
    /// has a value for each column and each anchor is exact: the entries
    /// beginning with them are then those of keys equal to `key`, and no
    /// key read need be checked.
    fn whole_key(&self, key: &[Value]) -> Option<Vec<u8>> {
        if key.len() != self.types.len() {
            return None;
        }
        let mut out = Vec::with_capacity(key.len().saturating_mul(KEY_VALUE_ROOM));
        for (value, data_type) in key.iter().zip(self.types.iter()) {
            if !anchor_into(value, data_type, &mut out)? {
                return None;
            }
        }
        Some(out)
    }

    /// Read through the tree borrowed, as the search of the same range
    /// reads them (`searched`).
    fn entries_of(&self, key: Vec<u8>, each: &mut dyn FnMut(EncodedEntry<'_>) -> Result<bool>) -> Result<()> {
        let upper = after(&key);
        let mut scan = scan_owned(&self.tree, Bound::Included(key), upper, Direction::Forward);
        while let Some(next) = scan.next_entry() {
            let (_, (entry, value)) = next?;
            let (found, row) = split(entry)?;
            noted(row, value)?;
            if !each(EncodedEntry { key: found, row, types: &self.types })? {
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
