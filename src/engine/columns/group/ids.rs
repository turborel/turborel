//! \[DESIGN\] A frozen row group's row ids written (`group.rs`): kept only
//! if they are not every id from the first to the last, in order, a page
//! of them at a time, and their rows' positions by id too if they are not
//! in increasing order.

use super::{BY_ID, ID, Pages, Run, Seal, count32};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::storage::RowId;

/// Writes `ids`, if they are not every id from their first to their last,
/// in order.
pub(super) fn write_ids<S: Seal>(pages: &Pages<'_, S>, ids: &[RowId], written: &mut Vec<PageId>) -> Result<Vec<Run>> {
    let (Some(&first), Some(&last)) = (ids.first(), ids.last()) else { return Ok(Vec::new()) };
    if increasing(ids) && last.checked_sub(first).and_then(|span| span.checked_add(1)) == u64::try_from(ids.len()).ok() {
        return Ok(Vec::new());
    }
    let per_page = pages.capacity.checked_div(ID).unwrap_or(1).max(1);
    let mut runs = Vec::new();
    for (n, chunk) in ids.chunks(per_page).enumerate() {
        let bytes: Vec<u8> = chunk.iter().flat_map(|id| id.to_le_bytes()).collect();
        let page = pages.put(&bytes, written)?;
        runs.push(Run::whole(count32(n.saturating_mul(per_page))?, count32(chunk.len())?, page));
    }
    Ok(runs)
}

/// Whether `ids` are in increasing order.
pub(super) fn increasing(ids: &[RowId]) -> bool {
    ids.windows(2).all(|w| w.first() < w.get(1))
}

/// Writes the positions of `ids` by id, if they are not in increasing
/// order.
pub(super) fn write_by_id<S: Seal>(pages: &Pages<'_, S>, ids: &[RowId], written: &mut Vec<PageId>) -> Result<Vec<Run>> {
    if increasing(ids) {
        return Ok(Vec::new());
    }
    let mut by_id: Vec<(RowId, u32)> = ids.iter().enumerate().map(|(at, &id)| Ok((id, count32(at)?))).collect::<Result<_>>()?;
    by_id.sort_unstable();
    let per_page = pages.capacity.checked_div(BY_ID).unwrap_or(1).max(1);
    let mut runs = Vec::new();
    for (n, chunk) in by_id.chunks(per_page).enumerate() {
        let bytes: Vec<u8> = chunk.iter().flat_map(|(id, at)| id.to_le_bytes().into_iter().chain(at.to_le_bytes())).collect();
        let page = pages.put(&bytes, written)?;
        runs.push(Run::whole(count32(n.saturating_mul(per_page))?, count32(chunk.len())?, page));
    }
    Ok(runs)
}
