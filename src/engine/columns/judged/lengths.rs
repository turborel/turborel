//! \[DESIGN\] (RULES.md D1267) A comparison with the empty string judged
//! of the octet lengths of a column the scan gives measured. Such a
//! column is given late with its values' lengths and no text
//! (`rows/frozen/batches/measured.rs`), and the one conjunct a scan is
//! told of it compares it unpadded with the empty string, as a
//! collation of NO PAD that orders strings as their octets do compares
//! them [ISO/IEC 9075-2:2023 §4.3.2] (D1183). A string's octets order
//! against none as its octet length does against 0, so the conjunct is
//! judged of the lengths the scan reads of the run's chunks
//! (`scan/lengths.rs`). The lengths of a run judged of every row are
//! held for the run's batch (`prepared/lengths.rs`), so the chunks are
//! read for their lengths once, as a column decoded to be judged is
//! handed on as its batch's.

use super::super::group::{Group, Seal};
use super::super::scan::{Reading, octets_at};
use super::Pred;
use crate::error::Result;
use crate::storage::Conjunct;
use crate::value::DataType;

/// \[DESIGN\] (RULES.md D1267) The rows of `given`, places from `from`,
/// or of all of the run `from` to `to` of `group`, that `pred` keeps of
/// a column of type `t` the scan gives measured, if it compares the
/// column unpadded with the empty string, a null as the conjunct is
/// true of a null. None of any other conjunct or column, or if a chunk
/// gives no lengths, the conjunct then judged as before.
///
/// # Errors
///
/// As reading a chunk.
pub(super) fn kept<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (pred, t): (&Pred, &DataType),
    (from, to): (u32, u32),
    given: Option<&[u32]>,
) -> Result<Option<Vec<u32>>> {
    let Conjunct::Unpadded { op, octets, .. } = &pred.conjunct else { return Ok(None) };
    if !octets.is_empty() || !prepared.measures(pred.column) {
        return Ok(None);
    }
    let all = super::rows(usize::try_from(to.saturating_sub(from)).unwrap_or(0), given);
    let positions: Vec<u32> = all.iter().map(|&r| from.saturating_add(r)).collect();
    let Some(lengths) = octets_at(group, (pages, &mut *prepared), (pred.column, t), &positions)? else { return Ok(None) };
    let keeps = |n: usize| if lengths.is_null(n) { pred.of_null } else { lengths.units.get(n).is_some_and(|l| op.holds(l.cmp(&0))) };
    let kept = all.iter().enumerate().filter(|(n, _)| keeps(*n)).map(|(_, &r)| r).collect();
    if given.is_none() {
        prepared.hold_lengths(pred.column, (from, to), lengths);
    }
    Ok(Some(kept))
}

// Unit tests: the conjunct judged of lengths and what is held of a run,
// and the fixture a scan's tests of octets share (RULES.md R-TEST-1,
// R-TEST-10).
#[cfg(test)]
pub(super) mod tests;
