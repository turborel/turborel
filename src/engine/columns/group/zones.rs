//! \[DESIGN\] A frozen row group's chunks of a column, as `group.rs`
//! writes them: each chunk's zone, its least and greatest values and its
//! nulls, and those of each part of it, and the parts' Bloom filters.

use std::cmp::Ordering;

use super::super::{Fields, bloom};
use super::{PART_ROWS, Pages, Part, Run, Seal, Zone, count32};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
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

/// Writes each column of `rows`, of `types`, as `write_column` does, on
/// up to `threads` threads at once, each a share of the columns: their
/// zones, in order. Every page written is added to `written`, so a failure
/// frees them all.
pub(super) fn write_columns<S: Seal + Sync>(
    pages: &Pages<'_, S>,
    types: &[DataType],
    (rows, bloom): (&[Row], &[usize]),
    (threads, written): (usize, &mut Vec<PageId>),
) -> Result<Vec<Vec<Zone>>> {
    let one = |c: usize, t: &DataType, w: &mut Vec<PageId>| write_column(pages, t, (rows, c, bloom.contains(&c)), w);
    if threads <= 1 || types.len() < 2 {
        return types.iter().enumerate().map(|(c, t)| one(c, t, written)).collect();
    }
    let each = types.len().div_ceil(threads);
    let one = &one;
    let shares: Vec<Share> = std::thread::scope(|scope| {
        let started: Vec<_> = (0..types.len())
            .step_by(each)
            .map(|from| {
                scope.spawn(move || {
                    let mut mine = Vec::new();
                    let share = types.iter().enumerate().skip(from).take(each);
                    let zones = share.map(|(c, t)| one(c, t, &mut mine)).collect();
                    (zones, mine)
                })
            })
            .collect();
        started.into_iter().map(|s| s.join().unwrap_or_else(|_| (Err(failed()), Vec::new()))).collect()
    });
    let mut columns = Vec::with_capacity(types.len());
    let mut first = None;
    for (zones, mine) in shares {
        written.extend(mine);
        match zones {
            Ok(zones) => columns.extend(zones),
            Err(e) => first = first.or(Some(e)),
        }
    }
    first.map_or(Ok(columns), Err)
}

/// A share of a group's columns as one thread wrote it: their zones, and
/// the pages it wrote.
type Share = (Result<Vec<Vec<Zone>>>, Vec<PageId>);

/// A thread writing a share of a group's columns that ended without
/// finishing.
fn failed() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a thread writing a frozen row group's columns failed")
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
