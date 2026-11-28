//! \[DESIGN\] (RULES.md D1267) The octet lengths of a frozen row group's
//! column at positions, read of its chunks with no text made
//! (`encoded/lengths.rs`), for a reader that takes the length of a
//! column and never reads its text.

use super::super::group::{Group, Seal};
use super::super::padding;
use super::{Reading, nulls_of};
use crate::error::Result;
use crate::storage::{Ints, Mask};
use crate::value::DataType;

/// \[DESIGN\] (RULES.md D1267) The octet length of the value of column
/// `column`, of type `t`, of each row at `positions` of `group`,
/// ascending, as exact numbers of scale 0, a null's null. None if `t`
/// is not VARCHAR, as a CHARACTER value is padded as it is read and no
/// chunk holds its length, or if a chunk asked of cannot give them
/// (`Chunk::octets`).
///
/// # Errors
///
/// As `rows`.
pub fn octets_at<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    positions: &[u32],
) -> Result<Option<Ints>> {
    if padding(t) != Some(None) {
        return Ok(None);
    }
    let Some(zones) = group.zones(column)? else { return Ok(None) };
    let (mut units, mut nulls) = (Vec::with_capacity(positions.len()), Mask::default());
    let mut rest = positions;
    for zone in zones {
        let end = zone.run.first.saturating_add(zone.run.rows);
        let (these, after) = rest.split_at(rest.partition_point(|&p| p < end));
        rest = after;
        if these.is_empty() {
            continue;
        }
        let places: Vec<usize> = these.iter().map(|&p| usize::try_from(p.saturating_sub(zone.run.first)).unwrap_or(usize::MAX)).collect();
        let held = prepared.with(pages, (column, zone, t), |chunk, _| Ok(chunk.octets(&places, (&mut units, &mut nulls))))?;
        if held.is_none() {
            return Ok(None);
        }
    }
    if units.len() != positions.len() {
        return Ok(None);
    }
    Ok(Some(Ints::new(units, 0, nulls_of(nulls))))
}
