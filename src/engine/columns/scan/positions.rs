//! \[DESIGN\] (RULES.md D31, D360) The values of a frozen row group's
//! column at positions, ascending, as a scan gives the rows of a run the
//! tree shadows some of, or those its conditions kept
//! (`engine/columns/judged.rs`): of each chunk, positions one after
//! another read as a run, positions dense in their span by decoding the
//! span whole, and the rest each read alone (`packed::dense`), so a scan
//! that kept one row in a hundred decodes about one value in a hundred.

use super::super::group::{Group, Seal, Zone};
use super::super::packed::dense;
use super::super::prepared::Made;
use super::super::texts::{Asked, Gathered};
use super::super::{Chunk, padding};
use super::{Reading, Words, datetime_shape, float_of, ints_scale, nulls_of, stored};
use crate::error::Result;
use crate::storage::{Column, Datetimes, Floats, Ints, Mask, Texts};
use crate::value::{DataType, Value};

/// \[DESIGN\] Column `c`, of type `t`, of the rows at `positions` of
/// `group`, ascending: as integers, approximate numbers, datetimes, truth
/// values or text if it is of them, else as values.
///
/// # Errors
///
/// As `rows`.
pub fn column_at<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (c, t): (usize, &DataType),
    positions: &[u32],
) -> Result<Column> {
    if let Some(ints) = ints_at(group, (pages, &mut *prepared), (c, t), positions)? {
        return Ok(Column::Ints(ints));
    }
    if let Some(floats) = floats_at(group, (pages, &mut *prepared), (c, t), positions)? {
        return Ok(Column::Floats(floats));
    }
    if let Some(datetimes) = datetimes_at(group, (pages, &mut *prepared), (c, t), positions)? {
        return Ok(Column::Datetimes(datetimes));
    }
    if let Some(bools) = super::bools_at(group, (pages, &mut *prepared), (c, t), positions)? {
        return Ok(Column::Bools(bools));
    }
    if let Some(texts) = texts_at(group, (pages, &mut *prepared), (c, t), positions)? {
        return Ok(Column::Texts(texts));
    }
    Ok(Column::from(values_at(group, (pages, prepared), (c, t), positions)?))
}

/// \[DESIGN\] As `ints_at`, as approximate numbers, as `floats` gives
/// those of a run.
///
/// # Errors
///
/// As `rows`.
pub fn floats_at<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    positions: &[u32],
) -> Result<Option<Floats>> {
    let DataType::Approximate { .. } = t else { return Ok(None) };
    Ok(words_at(group, (pages, prepared), (column, t), positions)?
        .map(|(words, nulls)| Floats::new(words.into_iter().map(float_of).collect(), nulls_of(nulls))))
}

/// \[DESIGN\] As `ints_at`, as datetimes, as `datetimes` gives those of a
/// run.
///
/// # Errors
///
/// As `rows`.
pub fn datetimes_at<S: Seal>(
    group: &Group,
    reading: Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    positions: &[u32],
) -> Result<Option<Datetimes>> {
    let Some(shape) = datetime_shape(t) else { return Ok(None) };
    Ok(words_at(group, reading, (column, t), positions)?.map(|(utc, nulls)| Datetimes::new(utc, None, shape, nulls_of(nulls))))
}

/// \[DESIGN\] The values of column `column`, of type `t`, of the rows at
/// `positions` of `group`, in increasing order, as integers, as `ints`
/// gives those of a run: each chunk's span from the first position asked
/// of it to the last decoded at once, and those asked taken of it, if they
/// are dense in it, else each read alone (`words_of`).
///
/// # Errors
///
/// As `rows`.
pub fn ints_at<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    positions: &[u32],
) -> Result<Option<Ints>> {
    // A chunk is read of if a position asked for is among its rows.
    let touched = |z: &Zone| {
        positions.get(positions.partition_point(|&p| p < z.run.first)).is_some_and(|&p| p < z.run.first.saturating_add(z.run.rows))
    };
    let Some(scale) = ints_scale(group, (column, t), touched)? else { return Ok(None) };
    Ok(words_at(group, (pages, prepared), (column, t), positions)?.map(|(units, nulls)| Ints::new(units, scale, nulls_of(nulls))))
}

/// The 8-byte fields of column `column` of the rows at `positions`, in
/// increasing order, of each chunk as `words_of` reads them.
fn words_at<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    positions: &[u32],
) -> Result<Words> {
    let Some(zones) = group.zones(column)? else { return Ok(None) };
    let (mut units, mut nulls) = (Vec::with_capacity(positions.len()), Mask::default());
    let mut span = (Vec::new(), Mask::default());
    let mut rest = positions;
    for zone in zones {
        let end = zone.run.first.saturating_add(zone.run.rows);
        let (these, after) = rest.split_at(rest.partition_point(|&p| p < end));
        rest = after;
        if these.is_empty() {
            continue;
        }
        let held = prepared.with(pages, (column, zone, t), |chunk, made| {
            Ok(words_of((chunk, made, pages.kernels), (zone.run.first, these), (&mut units, &mut nulls), &mut span))
        })?;
        if held.is_none() {
            return Ok(None);
        }
    }
    if units.len() != positions.len() {
        return Ok(None);
    }
    Ok(Some((units, nulls)))
}

/// \[DESIGN\] (RULES.md D360) The 8-byte fields at `these`, ascending, of a
/// chunk whose first row is `first`, pushed to `units` and whether each is
/// null to `nulls`: of positions one after another, read as a run; of
/// positions dense in their span (`packed::dense`), the span decoded whole
/// into `span` and those asked taken of it; else each read alone, so a
/// scan that kept few rows of a chunk unpacks no others. None if its
/// encoding does not hold them.
pub(super) fn words_of(
    (chunk, made, kernels): (&Chunk<'_>, &Made, &dyn crate::kernels::Kernels),
    (first, these): (u32, &[u32]),
    (units, nulls): (&mut Vec<i64>, &mut Mask),
    span: &mut (Vec<i64>, Mask),
) -> Option<()> {
    let at = |p: u32| usize::try_from(p.saturating_sub(first)).unwrap_or(usize::MAX);
    let (low, high) = (at(*these.first()?), at(*these.last()?));
    let width = high.checked_sub(low)?.saturating_add(1);
    if width == these.len() {
        return chunk.ints_made((low, high.saturating_add(1)), (units, nulls), (made, kernels));
    }
    if !dense(these.len(), width) {
        for &p in these {
            units.push(chunk.int_made(at(p), made)?);
            nulls.push(chunk.is_null(at(p)));
        }
        return Some(());
    }
    let (words, marks) = span;
    words.clear();
    *marks = Mask::default();
    chunk.ints_made((low, high.saturating_add(1)), (words, marks), (made, kernels))?;
    for &p in these {
        let n = at(p).saturating_sub(low);
        units.push(words.get(n).copied().unwrap_or(0));
        nulls.push(n >= marks.len() || marks.get(n));
    }
    Some(())
}

/// Each chunk of `zones` asked for one of `positions`, ascending, with
/// the positions asked of it.
fn asked_of<'z, 'p>(zones: &'z [Zone], positions: &'p [u32]) -> impl Iterator<Item = (&'z Zone, &'p [u32])> {
    let mut rest = positions;
    zones.iter().filter_map(move |zone| {
        let end = zone.run.first.saturating_add(zone.run.rows);
        let (these, after) = rest.split_at(rest.partition_point(|&p| p < end));
        rest = after;
        (!these.is_empty()).then_some((zone, these))
    })
}

/// \[DESIGN\] (RULES.md D1252, D1316) The values of column `column`, of
/// type `t`, of the rows at `positions` of `group`, in increasing order,
/// as text, as `texts` gives those of a run: each chunk's rows asked
/// gathered into one vector of octets, a dictionary asked for fewer rows
/// than it has entries giving only the entries those rows read
/// (`texts::Gathered`).
///
/// # Errors
///
/// As `rows`.
pub fn texts_at<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    positions: &[u32],
) -> Result<Option<Texts>> {
    let Some(pad) = padding(t) else { return Ok(None) };
    let Some(zones) = group.zones(column)? else { return Ok(None) };
    let room = stored(pages, asked_of(zones, positions).map(|(zone, these)| (zone, these.len())));
    let (mut gathered, mut at) = (Gathered::with_room(positions.len(), room), Vec::new());
    for (zone, these) in asked_of(zones, positions) {
        at.clear();
        at.extend(these.iter().map(|&p| usize::try_from(p.saturating_sub(zone.run.first)).unwrap_or(usize::MAX)));
        // Positions one after another are read as one run.
        let (low, past) = (at.first().copied().unwrap_or(0), at.last().map_or(0, |&high| high.saturating_add(1)));
        let rows = if past.saturating_sub(low) == at.len() { Asked::Run(low, past) } else { Asked::At(&at) };
        let held = prepared.with(pages, (column, zone, t), |chunk, _| Ok(chunk.gather(rows, pad, &mut gathered)))?;
        if held.is_none() {
            return Ok(None);
        }
    }
    Ok(gathered.finish(pages.kernels))
}

/// The values of column `column`, of type `t`, of the rows at `positions`
/// of `group`, in increasing order.
///
/// # Errors
///
/// As `rows`.
pub fn values_at<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (column, t): (usize, &DataType),
    positions: &[u32],
) -> Result<Vec<Value>> {
    let mut out = Vec::with_capacity(positions.len());
    let Some(zones) = group.zones(column)? else { return Ok(vec![Value::Null; positions.len()]) };
    let mut rest = positions;
    for zone in zones {
        let end = zone.run.first.saturating_add(zone.run.rows);
        let here = rest.partition_point(|&p| p < end);
        let (these, after) = rest.split_at(here);
        rest = after;
        if these.is_empty() {
            continue;
        }
        prepared.with(pages, (column, zone, t), |chunk, _| {
            for &position in these {
                out.push(chunk.value(usize::try_from(position.saturating_sub(zone.run.first)).unwrap_or(usize::MAX), t)?);
            }
            Ok(())
        })?;
    }
    Ok(out)
}
