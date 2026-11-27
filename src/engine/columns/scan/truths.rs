//! \[DESIGN\] (RULES.md D354) A column of truth values read into a batch
//! as bits (`storage::Bools`), the true ones set and the nulls as the
//! chunk's bitmap keeps them, in place of a value a row.

use super::{Group, Mask, Reading, Seal, nulls_of};
use crate::error::Result;
use crate::storage::Bools;
use crate::value::DataType;

/// \[DESIGN\] The values of column `column` of the rows at positions
/// `from` to `to` of `group`, as truth values, if `t` is BOOLEAN and each
/// chunk keeps its fields whole (`Chunk::bools`).
///
/// # Errors
///
/// As `scan::rows`.
pub fn bools<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    (from, to): (u32, u32),
) -> Result<Option<Bools>> {
    let (DataType::Boolean, Some(zones)) = (t, group.zones(column)?) else { return Ok(None) };
    let (mut bits, mut nulls) = (Mask::default(), Mask::default());
    for zone in zones.iter().filter(|z| z.run.first < to && z.run.first.saturating_add(z.run.rows) > from) {
        let (first, end) = (zone.run.first.max(from), zone.run.first.saturating_add(zone.run.rows).min(to));
        let at = |p: u32| usize::try_from(p.saturating_sub(zone.run.first)).unwrap_or(usize::MAX);
        let held = prepared.with(pages, (column, zone, t), |chunk, _| Ok(chunk.bools((at(first), at(end)), (&mut bits, &mut nulls))))?;
        if held.is_none() {
            return Ok(None);
        }
    }
    if bits.len() != usize::try_from(to.saturating_sub(from)).unwrap_or(0) {
        return Ok(None);
    }
    Ok(Some(Bools::new(bits, nulls_of(nulls))))
}

/// \[DESIGN\] As `bools`, of the rows at `positions` of `group`, in
/// increasing order.
///
/// # Errors
///
/// As `scan::rows`.
pub fn bools_at<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    positions: &[u32],
) -> Result<Option<Bools>> {
    let (DataType::Boolean, Some(zones)) = (t, group.zones(column)?) else { return Ok(None) };
    let (mut bits, mut nulls) = (Mask::default(), Mask::default());
    let mut rest = positions;
    for zone in zones {
        let end = zone.run.first.saturating_add(zone.run.rows);
        let (these, after) = rest.split_at(rest.partition_point(|&p| p < end));
        rest = after;
        if these.is_empty() {
            continue;
        }
        let at = |p: u32| usize::try_from(p.saturating_sub(zone.run.first)).unwrap_or(usize::MAX);
        let held = prepared.with(pages, (column, zone, t), |chunk, _| {
            Ok(these.iter().try_for_each(|&p| chunk.bools((at(p), at(p).saturating_add(1)), (&mut bits, &mut nulls))))
        })?;
        if held.is_none() {
            return Ok(None);
        }
    }
    if bits.len() != positions.len() {
        return Ok(None);
    }
    Ok(Some(Bools::new(bits, nulls_of(nulls))))
}
