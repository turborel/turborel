//! \[DESIGN\] The hybrid layout of a frozen row group (PAX: Ailamaki,
//! DeWitt, Hill and Skounakis, VLDB 2001; RULES.md D31): each page holds a
//! range of the group's rows, as many as fit, a chunk of each column of
//! them one after another, each chunk encoded as the column layout's are
//! (`columns/encoded.rs`) and found by where it lies on the page
//! (`Run::at`). A row read whole reads one page, not a page a column; a
//! scan of a few columns reads every column's bytes. Rows whose chunks fit
//! no page together, of values too long, are each written as the column
//! layout writes them, a chunk of each column on a page of its own.

use super::{Pages, Run, Seal, Zone, count32, put_filters, zone_of};
use crate::disk::pager::PageId;
use crate::engine::columns::Fields;
use crate::error::Result;
use crate::storage::Row;
use crate::value::{DataType, Value};

/// Writes `rows`, a column of each of `types`, in the hybrid layout, a
/// Bloom filter of each part of the columns at `bloom`: each column's
/// zones, in order.
pub(super) fn write_pax<S: Seal>(
    pages: &Pages<'_, S>,
    types: &[DataType],
    (rows, bloom): (&[Row], &[usize]),
    written: &mut Vec<PageId>,
) -> Result<Vec<Vec<Zone>>> {
    let values: Vec<Vec<Value>> =
        (0..types.len()).map(|c| rows.iter().map(|r| r.get(c).cloned().unwrap_or(Value::Null)).collect()).collect();
    let fields = values.iter().zip(types).map(|(v, t)| Fields::of(t, v.iter().cloned())).collect::<Result<Vec<_>>>()?;
    let columns: Vec<(&Fields, &DataType)> = fields.iter().zip(types).collect();
    let mut zones: Vec<Vec<Zone>> = vec![Vec::new(); types.len()];
    let mut from = 0_usize;
    while from < rows.len() {
        let count = Fields::fitting_together(&columns, from, (pages.capacity, pages.compress));
        let runs = match page_of(pages, &columns, (from, count), written)? {
            Some(runs) => runs,
            None => apart(pages, &columns, from, written)?,
        };
        let step = runs.first().map_or(1, |r| usize::try_from(r.rows).unwrap_or(1)).max(1);
        for ((column, held), run) in zones.iter_mut().zip(&values).zip(runs) {
            column.push(zone_of(held, run));
        }
        from = from.saturating_add(step);
    }
    for &c in bloom {
        if let (Some(held), Some(column)) = (values.get(c), zones.get_mut(c)) {
            put_filters(pages, (held, column), written)?;
        }
    }
    Ok(zones)
}

/// Writes the rows from `from`, `count` of them, as one page holding a
/// chunk of each of `columns`: each chunk's run; none if there are none,
/// or their chunks are too long for a page together after all.
fn page_of<S: Seal>(
    pages: &Pages<'_, S>,
    columns: &[(&Fields, &DataType)],
    (from, count): (usize, usize),
    written: &mut Vec<PageId>,
) -> Result<Option<Vec<Run>>> {
    if count == 0 {
        return Ok(None);
    }
    let mut content = Vec::with_capacity(pages.capacity);
    let mut places = Vec::with_capacity(columns.len());
    for &(f, t) in columns {
        let chunk = f.chunk(t, from, (count, pages.compress))?;
        places.push((count32(content.len())?, count32(chunk.len())?));
        content.extend_from_slice(&chunk);
    }
    if content.len() > pages.capacity {
        return Ok(None);
    }
    let page = pages.put(&content, written)?;
    let (first, rows) = (count32(from)?, count32(count)?);
    Ok(Some(places.into_iter().map(|(at, len)| Run { first, rows, page, at, len }).collect()))
}

/// Writes the row at `from` as the column layout does, a chunk of each of
/// `columns` on a page of its own: each chunk's run.
fn apart<S: Seal>(pages: &Pages<'_, S>, columns: &[(&Fields, &DataType)], from: usize, written: &mut Vec<PageId>) -> Result<Vec<Run>> {
    let first = count32(from)?;
    columns.iter().map(|&(f, t)| Ok(Run::whole(first, 1, pages.put(&f.chunk(t, from, (1, pages.compress))?, written)?))).collect()
}
