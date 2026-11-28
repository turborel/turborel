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
//!
//! \[DESIGN\] (RULES.md D1319) Where the column's group keeps a lengths
//! lane the same conjunct is judged of the lane for any reader, the
//! column measured or not, as the lane is read as integers are and the
//! text is then decoded of the rows kept alone. The lane's bounds prove
//! the conjunct of a whole run where no value of its parts is empty, or
//! every one is (`proved`), and the run is then judged of nothing.

use super::super::group::{Group, Seal};
use super::super::scan::{Reading, octets_at};
use super::Pred;
use super::proved::{every, every_part};
use crate::error::Result;
use crate::storage::Conjunct;
use crate::value::{CompOp, DataType, Decimal, Value};

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
    let Some(op) = emptied(pred) else { return Ok(None) };
    // Of a column measured, or of one whose group keeps its lengths (RULES.md D1319).
    if !prepared.measures(pred.column) && group.lane(pred.column).is_none() {
        return Ok(None);
    }
    let all = super::rows(usize::try_from(to.saturating_sub(from)).unwrap_or(0), given);
    let positions: Vec<u32> = all.iter().map(|&r| from.saturating_add(r)).collect();
    let Some(lengths) = octets_at(group, (pages, &mut *prepared), (pred.column, t), &positions)? else { return Ok(None) };
    let keeps = |n: usize| if lengths.is_null(n) { pred.of_null } else { lengths.units.get(n).is_some_and(|l| op.holds(l.cmp(&0))) };
    let kept = all.iter().enumerate().filter(|(n, _)| keeps(*n)).map(|(_, &r)| r).collect();
    if given.is_none() && prepared.measures(pred.column) {
        prepared.hold_lengths(pred.column, (from, to), lengths);
    }
    Ok(Some(kept))
}

/// The operator of `pred` if it compares its column unpadded with the
/// empty string, which is the comparison of the column's octet length
/// with 0.
fn emptied(pred: &Pred) -> Option<CompOp> {
    let Conjunct::Unpadded { op, octets, .. } = &pred.conjunct else { return None };
    octets.is_empty().then_some(*op)
}

/// \[DESIGN\] (RULES.md D1319) Whether `pred`, a comparison of its column
/// with the empty string, is true of every row the run `from` to `to`
/// of `group` gives, by the bounds of the parts of the column's lengths
/// lane: no null among them, and their least and greatest lengths
/// compare with 0 as the conjunct asks (`proved::every`, RULES.md
/// D1293). Not so of any other conjunct, nor of a group with no lane of
/// the column.
pub(super) fn proved(group: &Group, pred: &Pred, (from, to): (u32, u32)) -> bool {
    let (Some(op), Some(lane)) = (emptied(pred), group.lane(pred.column)) else { return false };
    let none = Value::Exact(Decimal::new(0, 0));
    every_part(lane, (from, to), &|part| every(op, part, &none))
}

// Unit tests: the conjunct judged of lengths and what is held of a run,
// and the fixture a scan's tests of octets share (RULES.md R-TEST-1,
// R-TEST-10).
#[cfg(test)]
pub(super) mod tests;
