//! \[DESIGN\] (RULES.md D1293) A comparison a run's parts prove. Each
//! part of a chunk, `PART_ROWS` rows, is written with the least and the
//! greatest of its values and the count of its nulls (`group/zones.rs`),
//! and a scan passes over a part these say a conjunct cannot match
//! (`scan.rs`). The same bounds can say the opposite. If every part a run
//! lies in holds no null, and its least and greatest both satisfy a
//! comparison with a constant, the comparison is true of every row of the
//! run, so its column is not decoded to judge it and no row is tested. A
//! range a hash join tells its probing side's scan holds nearly every
//! part of a table whose keys the built rows span, and a range of a
//! column a table is written in the order of holds every part between
//! its ends.
//!
//! A part's bounds are those of the rows frozen in it, every one of them.
//! A run gives those rows or fewer: a row changed or deleted since the
//! freeze has an entry in the tree, its frozen row is left out of the run
//! (`rows/frozen/batches.rs`, `unshadowed`), and what the tree holds of it
//! is given apart and judged by the filter alone. So the bounds cover
//! every row the run gives, and what they prove of all the part's rows is
//! true of those.
//!
//! Only a column of exact numbers or of datetimes of no time zone is
//! proved so, whose bounds are its values whole and compare with the
//! constant as each value does (`value::compare`). A part whose bounds do
//! not compare with the constant proves nothing.

use std::cmp::Ordering;

use super::super::group::{Group, PART_ROWS, Part};
use super::Pred;
use crate::error::Result;
use crate::value::{CompOp, DataType, Value, compare};

/// Whether `pred`, of a column of type `t`, is a comparison with a
/// constant that the bounds of every part the run `from` to `to` of
/// `group` lies in prove true of every row of it.
///
/// # Errors
///
/// SQLCODE -910 if `group`'s header was read without the column.
pub(super) fn proved(group: &Group, (pred, t): (&Pred, &DataType), (from, to): (u32, u32)) -> Result<bool> {
    let Some((op, constant)) = pred.compared().filter(|_| whole_bounds(t)) else { return Ok(false) };
    let Some(zones) = group.zones(pred.column)? else { return Ok(false) };
    let first = zones.partition_point(|zone| zone.run.first.saturating_add(zone.run.rows) <= from);
    let mut at = from;
    for zone in zones.iter().skip(first) {
        // A chunk that does not go on from the rows proved so far leaves
        // rows no bounds are known of.
        if at >= to || zone.run.first > at {
            break;
        }
        let end = zone.run.first.saturating_add(zone.run.rows).min(to);
        let (low, high) = (part_of(at, zone.run.first), part_of(end.saturating_sub(1), zone.run.first));
        let Some(parts) = zone.parts.get(low..=high) else { return Ok(false) };
        if !parts.iter().all(|p| every(op, p, constant)) {
            return Ok(false);
        }
        at = end;
    }
    Ok(at >= to)
}

/// The place among a chunk's parts of the part row `row` of the group is
/// in, the chunk's first row `first`.
fn part_of(row: u32, first: u32) -> usize {
    usize::try_from(row.saturating_sub(first).checked_div(PART_ROWS).unwrap_or(0)).unwrap_or(usize::MAX)
}

/// Whether a column of type `t` is written with bounds that are its
/// values whole: exact numbers and datetimes of no time zone.
fn whole_bounds(t: &DataType) -> bool {
    matches!(t, DataType::Exact { .. } | DataType::Date | DataType::Time { zoned: false, .. } | DataType::Timestamp { zoned: false, .. })
}

/// Whether `op` with `constant` is true of every row of `part`: it holds
/// no null, of which no comparison is true [X3.135-1986 §5.11 GR2], and
/// its least and greatest values leave no value between them that fails.
/// Only the end that decides a comparison is compared with the constant.
fn every(op: CompOp, part: &Part, constant: &Value) -> bool {
    let Some((least, greatest)) = part.bounds.as_ref().filter(|_| part.nulls == 0) else { return false };
    let (low, high) = (|| compare(least, constant), || compare(greatest, constant));
    match op {
        CompOp::Eq => low() == Some(Ordering::Equal) && high() == Some(Ordering::Equal),
        CompOp::Ne => low() == Some(Ordering::Greater) || high() == Some(Ordering::Less),
        CompOp::Lt => high() == Some(Ordering::Less),
        CompOp::Le => high().is_some_and(Ordering::is_le),
        CompOp::Gt => low() == Some(Ordering::Greater),
        CompOp::Ge => low().is_some_and(Ordering::is_ge),
    }
}

// Unit tests: what a run's parts prove of each comparison, at its
// boundary values, with a null and across chunks (RULES.md R-TEST-1,
// R-TEST-8).
#[cfg(test)]
mod tests;
