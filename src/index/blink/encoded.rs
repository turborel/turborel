//! \[DESIGN\] (RULES.md D821) A search whose entries keep their keys as
//! the tree holds them. Each entry is lent from the scan, none copied
//! (RULES.md D809), its row id cut off and its key handed on as it is,
//! none decoded: the caller compares a row's key with it as bytes. A range
//! whose keys are each checked against the probe decodes them to check
//! them, so it is searched as `search` searches.

use std::sync::Arc;

use super::entries::{Held, Keyed};
use super::value::noted;
use super::{BLinkIndex, split};
use crate::engine::tree::{Direction, Scan, scan_owned};
use crate::error::Result;
use crate::index::{Encoded, EncodedEntries, EncodedEntry, KeyOrder, Probe};

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
