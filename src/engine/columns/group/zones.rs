//! \[DESIGN\] A frozen row group's chunks of a column, as `group.rs`
//! writes them: each chunk's zone, its least and greatest values and its
//! nulls, and those of each part of it, and the parts' Bloom filters.

use std::cmp::Ordering;

use super::super::{Fields, bloom};
use super::{PART_ROWS, Pages, Part, Run, Seal, Zone, count32};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::storage::Row;
use crate::value::{DataType, Value, compare};

/// The least and greatest of `values` that are not null, and the nulls.
pub(super) fn bounds_of(values: &[Value]) -> (Option<(Value, Value)>, u32) {
    let mut bounds: Option<(Value, Value)> = None;
    let mut nulls = 0_u32;
    for v in values {
        if v.is_null() {
            nulls = nulls.saturating_add(1);
            continue;
        }
        bounds = Some(match bounds {
            None => (v.clone(), v.clone()),
            Some((least, greatest)) => {
                let least = if compare(v, &least) == Some(Ordering::Less) { v.clone() } else { least };
                let greatest = if compare(v, &greatest) == Some(Ordering::Greater) { v.clone() } else { greatest };
                (least, greatest)
            }
        });
    }
    (bounds, nulls)
}

/// Writes the column `column` of `rows`, of type `t`, in chunks, and a
/// Bloom filter of each part if `bloom`.
pub(super) fn write_column<S: Seal>(
    pages: &Pages<'_, S>,
    t: &DataType,
    (rows, column, bloom): (&[Row], usize, bool),
    written: &mut Vec<PageId>,
) -> Result<Vec<Zone>> {
    let values: Vec<Value> = rows.iter().map(|r| r.get(column).cloned().unwrap_or(Value::Null)).collect();
    let fields = Fields::of(t, values.iter().cloned())?;
    let mut zones = Vec::new();
    let mut from = 0_usize;
    while from < fields.len() {
        let count = fields.fitting(t, from, (pages.capacity, pages.compress));
        let page = pages.put(&fields.chunk(t, from, (count, pages.compress))?, written)?;
        zones.push(zone_of(&values, Run::whole(count32(from)?, count32(count)?, page)));
        from = from.saturating_add(count);
    }
    if bloom {
        put_filters(pages, (&values, &mut zones), written)?;
    }
    Ok(zones)
}

/// The zone of the chunk of `run`, of the values of its column `values`:
/// its nulls and bounds, and those of each part of it.
pub(super) fn zone_of(values: &[Value], run: Run) -> Zone {
    let first = usize::try_from(run.first).unwrap_or(usize::MAX);
    let held = values.get(first..first.saturating_add(usize::try_from(run.rows).unwrap_or(0))).unwrap_or_default();
    let (bounds, nulls) = bounds_of(held);
    let parts = held
        .chunks(usize::try_from(PART_ROWS).unwrap_or(usize::MAX))
        .map(|part| {
            let (part_bounds, part_nulls) = bounds_of(part);
            Part { nulls: part_nulls, bounds: part_bounds, filter: None, imprint: None, sketch: None }
        })
        .collect();
    Zone { run, nulls, bounds, parts }
}

/// Writes a Bloom filter of each part of `zones`, of `values`, and puts
/// where each is in its part.
pub(super) fn put_filters<S: Seal>(
    pages: &Pages<'_, S>,
    (values, zones): (&[Value], &mut [Zone]),
    written: &mut Vec<PageId>,
) -> Result<()> {
    let (part, bytes) = (usize::try_from(PART_ROWS).unwrap_or(usize::MAX), bloom::filter_bytes(pages.capacity));
    let mut filters = Vec::new();
    for zone in zones.iter() {
        let first = usize::try_from(zone.run.first).unwrap_or(usize::MAX);
        let held = values.get(first..first.saturating_add(usize::try_from(zone.run.rows).unwrap_or(0))).unwrap_or_default();
        filters.extend(held.chunks(part).map(|these| bloom::filter_of(these, bytes)));
    }
    let places = bloom::write_filters(pages, &filters, written)?;
    for (p, place) in zones.iter_mut().flat_map(|z| z.parts.iter_mut()).zip(places) {
        p.filter = Some(place);
    }
    Ok(())
}
