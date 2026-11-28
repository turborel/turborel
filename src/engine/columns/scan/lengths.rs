//! \[DESIGN\] (RULES.md D1267) The octet lengths of a frozen row group's
//! column at positions, for a reader that takes the length of a column
//! and never reads its text. They are read of the column's lengths lane
//! where its group has one (RULES.md D1319), as integers are, and else
//! of its text's chunks with no text made (`encoded/lengths.rs`).

use super::super::group::{Group, LENGTH, Seal, Zone};
use super::super::padding;
use super::positions::words_of;
use super::{Reading, nulls_of};
use crate::error::Result;
use crate::index::Probe;
use crate::storage::{Condition, Conjunct, Ints, Mask};
use crate::value::{DataType, Decimal, Value};

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
    if let Some(lane) = group.lane(column) {
        return lane_at(lane, (pages, prepared), group.lane_key(column), positions);
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

/// \[DESIGN\] (RULES.md D1319) The lengths at `positions`, ascending, read
/// of a lengths lane's chunks `lane`, kept open by `key`
/// (`Group::lane_key`), as a column of integers is read at positions
/// (`positions::words_of`). None if a chunk does not give them.
fn lane_at<S: Seal>(lane: &[Zone], (pages, prepared): Reading<'_, '_, S>, key: usize, positions: &[u32]) -> Result<Option<Ints>> {
    let (mut units, mut nulls) = (Vec::with_capacity(positions.len()), Mask::default());
    let mut span = (Vec::new(), Mask::default());
    let mut rest = positions;
    for zone in lane {
        let end = zone.run.first.saturating_add(zone.run.rows);
        let (these, after) = rest.split_at(rest.partition_point(|&p| p < end));
        rest = after;
        if these.is_empty() {
            continue;
        }
        let held = prepared.with(pages, (key, zone, &LENGTH), |chunk, made| {
            Ok(words_of((chunk, made, pages.kernels), (zone.run.first, these), (&mut units, &mut nulls), &mut span))
        })?;
        if held.is_none() {
            return Ok(None);
        }
    }
    if units.len() != positions.len() {
        return Ok(None);
    }
    Ok(Some(Ints::new(units, 0, nulls_of(nulls))))
}

/// \[DESIGN\] (RULES.md D1319) The runs of positions of `group`'s rows in
/// the parts `condition` may match, if it compares a column unpadded
/// with the empty string and the group keeps the column's lengths. The
/// comparison is that of the octet length with 0, so a part is passed
/// over by the least and the greatest of its lengths: one with no empty
/// value by an equality, one of empty values alone by an inequality. A
/// part of nulls alone holds no row a comparison is true of. None of any
/// other condition, or of a group with no lane of the column.
pub(super) fn kept_parts(group: &Group, condition: &Conjunct) -> Option<Vec<(u32, u32)>> {
    let Conjunct::Unpadded { column, op, octets } = condition else { return None };
    let lane = group.lane(*column).filter(|_| octets.is_empty())?;
    let none = Value::Exact(Decimal::new(0, 0));
    let asked = Conjunct::Condition(Condition { columns: vec![*column], probe: Probe::Compare { op: *op, value: none } });
    Some(super::parts_kept(lane, &mut |part, _| asked.may_match_range(part.bounds.as_ref())))
}
