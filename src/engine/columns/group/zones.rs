//! \[DESIGN\] A frozen row group's chunks of a column, as `group.rs`
//! writes them: each chunk's zone, its least and greatest values and its
//! nulls, and those of each part of it, and the parts' Bloom filters, of
//! the column's values read where they are (`column.rs`).

use super::super::{Fields, bloom};
use super::column::Column;
use super::{PART_ROWS, Pages, Part, Run, Seal, Zone, count32};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::value::{DataType, Value};

/// The least and greatest of `values` that are not null, and the nulls.
pub(super) fn bounds_of(values: &[Value]) -> (Option<(Value, Value)>, u32) {
    values.bounds((0, values.len()))
}

/// Writes the column `column`, of type `t`, `fields` its values written,
/// in chunks, and a Bloom filter of each part if `bloom`.
pub(super) fn write_column<S: Seal, C: Column>(
    pages: &Pages<'_, S>,
    t: &DataType,
    (fields, column, bloom): (&Fields<'_>, &C, bool),
    written: &mut Vec<PageId>,
) -> Result<Vec<Zone>> {
    let mut zones = Vec::new();
    for (from, count, chunk) in fields.chunks(t, (pages.capacity, pages.compress))? {
        let page = pages.put(&chunk, written)?;
        zones.push(zone_of(column, Run::whole(count32(from)?, count32(count)?, page)));
    }
    if bloom {
        put_filters(pages, (column, &mut zones), written)?;
    }
    Ok(zones)
}

/// The rows of `run` that `column` holds: its first and one past its
/// last, none if it holds not all of them.
fn rows_of<C: Column + ?Sized>(column: &C, run: &Run) -> (usize, usize) {
    let first = usize::try_from(run.first).unwrap_or(usize::MAX);
    let end = first.saturating_add(usize::try_from(run.rows).unwrap_or(0));
    if end > column.len() { (0, 0) } else { (first, end) }
}

/// The runs of each part of the rows `from` to `to`, in order.
fn parts_in((from, to): (usize, usize)) -> impl Iterator<Item = (usize, usize)> {
    let part = usize::try_from(PART_ROWS).unwrap_or(usize::MAX).max(1);
    (from..to).step_by(part).map(move |at| (at, at.saturating_add(part).min(to)))
}

/// The zone of the chunk of `run`, of its column `column`: its nulls and
/// bounds, and those of each part of it.
pub(super) fn zone_of<C: Column + ?Sized>(column: &C, run: Run) -> Zone {
    let held = rows_of(column, &run);
    let (bounds, nulls) = column.bounds(held);
    let parts = parts_in(held)
        .map(|part| {
            let (part_bounds, part_nulls) = column.bounds(part);
            Part { nulls: part_nulls, bounds: part_bounds, filter: None, imprint: None, sketch: None }
        })
        .collect();
    Zone { run, nulls, bounds, parts }
}

/// Writes a Bloom filter of each part of `zones`, of its column `column`,
/// and puts where each is in its part.
pub(super) fn put_filters<S: Seal, C: Column + ?Sized>(
    pages: &Pages<'_, S>,
    (column, zones): (&C, &mut [Zone]),
    written: &mut Vec<PageId>,
) -> Result<()> {
    let bytes = bloom::filter_bytes(pages.capacity);
    let mut filters = Vec::new();
    for zone in zones.iter() {
        let held = rows_of(column, &zone.run);
        filters.extend(parts_in(held).map(|(from, to)| bloom::filter_of((from..to).map(|n| column.value(n)), bytes)));
    }
    let places = bloom::write_filters(pages, &filters, written)?;
    for (p, place) in zones.iter_mut().flat_map(|z| z.parts.iter_mut()).zip(places) {
        p.filter = Some(place);
    }
    Ok(())
}
