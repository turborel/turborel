//! \[DESIGN\] An entry's value (RULES.md D500): the hint, the page of the
//! table's leaf its row was in as the entry was written, as four bytes, 0
//! for none or one past them, then its count, left out for one.

use super::corrupt;
use crate::engine::tree::hints;
use crate::error::Result;
use crate::storage::RowId;

/// Bytes of an entry's hint, and of its count.
pub(super) const HINT: usize = 4;

/// The count an entry's value holds.
pub(super) fn count_of(value: &[u8]) -> Result<u32> {
    match value.get(HINT..) {
        Some([]) => Ok(1),
        Some(count) => <[u8; HINT]>::try_from(count).map(u32::from_le_bytes).map_err(|_| corrupt()),
        None => Err(corrupt()),
    }
}

/// The hint an entry's value holds: the table's leaf its row was in.
pub(super) fn hint_of(value: &[u8]) -> Result<u64> {
    value.get(..HINT).and_then(|h| <[u8; HINT]>::try_from(h).ok()).map(|h| u64::from(u32::from_le_bytes(h))).ok_or_else(corrupt)
}

/// The value holding `count` and the hint `leaf`, none if it is past what
/// a hint holds.
pub(super) fn value_of(count: u32, leaf: u64) -> Vec<u8> {
    let hint = u32::try_from(leaf).unwrap_or_default().to_le_bytes();
    if count == 1 { hint.to_vec() } else { [hint, count.to_le_bytes()].concat() }
}

/// The hint for row `row`'s entry: the leaf the table noted it in as it
/// wrote the row, else `old`'s, the entry's hint before.
pub(super) fn hint_for(row: RowId, old: Option<&[u8]>) -> Result<u64> {
    match hints::leaf_of(row) {
        Some(leaf) => Ok(leaf),
        None => old.map_or(Ok(hints::NO_LEAF), hint_of),
    }
}

/// Notes the hint of the entry of row `row` whose value is `value`, for the
/// table's read of the row that follows (`engine/tree/near.rs`).
pub(super) fn noted(row: RowId, value: &[u8]) -> Result<RowId> {
    hints::note(row, hint_of(value)?);
    Ok(row)
}
