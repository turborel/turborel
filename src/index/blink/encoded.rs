//! \[DESIGN\] (RULES.md D821) A search whose entries keep their keys as
//! the tree holds them. Each entry's bytes, read of its leaf, lose their
//! row id in place and are handed on as they are, none decoded: the
//! caller compares a row's key with them as bytes. A range whose keys are
//! each checked against the probe decodes them to check them, so it is
//! searched as `search` searches.

use std::sync::Arc;

use super::{BLinkIndex, Held, noted, split};
use crate::engine::tree::{Direction, scan_of};
use crate::index::{Encoded, KeyOrder, Probe};

/// The entries `probe` selects of `index`, in `order`, their keys as the
/// tree holds them; none if a key read must be checked.
pub(super) fn searched(index: Arc<BLinkIndex>, probe: &Probe, order: KeyOrder) -> Option<Encoded> {
    let range = index.range(probe).filter(|range| !range.checked)?;
    let direction = match order {
        KeyOrder::Ascending => Direction::Forward,
        KeyOrder::Descending => Direction::Backward,
    };
    let types = Arc::clone(&index.types);
    let scan = scan_of(Held(index), range.lower.as_ref().map(Vec::as_slice), range.upper.as_ref().map(Vec::as_slice), direction);
    let entries = scan.map(|entry| {
        entry.and_then(|(mut entry, value)| {
            let (key, row) = split(&entry)?;
            noted(row, &value)?;
            let length = key.len();
            entry.truncate(length);
            Ok((entry, row))
        })
    });
    Some(Encoded { entries: Box::new(entries), types })
}
