//! \[DESIGN\] A frozen row group's header, written and read back as
//! `group.rs` describes it.

use super::super::sketch::Edges;
use super::summary::{Overall, Placed, Summary};
use super::{Filter, GROUP_FORMAT, Group, Pages, Part, Run, Seal, Zone, corrupt, count32, read_chain};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::value::{DataType, Value};

fn put_u32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, n: u64) {
    out.extend_from_slice(&n.to_le_bytes());
}

fn put_run(out: &mut Vec<u8>, run: &Run) {
    put_u32(out, run.first);
    put_u32(out, run.rows);
    put_u64(out, run.page.0);
    put_u32(out, run.at);
    put_u32(out, run.len);
}

fn put_bound(out: &mut Vec<u8>, v: &Value, t: &DataType) -> Result<()> {
    let bytes = t.encode_record(v)?;
    put_u32(out, count32(bytes.len())?);
    out.extend_from_slice(&bytes);
    Ok(())
}

/// Appends `bounds`, if any, of type `t`.
fn put_bounds(out: &mut Vec<u8>, bounds: Option<&(Value, Value)>, t: &DataType) -> Result<()> {
    match bounds {
        Some((least, greatest)) => {
            out.push(1);
            put_bound(out, least, t)?;
            put_bound(out, greatest, t)?;
        }
        None => out.push(0),
    }
    Ok(())
}

/// The header of `group`, its columns of `types`.
pub(super) fn encode(group: &Group, types: &[DataType]) -> Result<Vec<u8>> {
    let mut out = vec![GROUP_FORMAT];
    put_u64(&mut out, group.first);
    put_u64(&mut out, group.last);
    put_u32(&mut out, group.rows);
    put_places(&mut out, &group.order)?;
    out.push(u8::from(group.pax));
    for runs in [&group.ids, &group.by_id] {
        put_u32(&mut out, count32(runs.len())?);
        for run in runs {
            put_run(&mut out, run);
        }
    }
    put_u32(&mut out, count32(group.columns.len())?);
    for (zones, t) in group.columns.iter().zip(types) {
        put_u32(&mut out, count32(zones.len())?);
        for zone in zones {
            put_run(&mut out, &zone.run);
            put_u32(&mut out, zone.nulls);
            put_bounds(&mut out, zone.bounds.as_ref(), t)?;
            put_u32(&mut out, count32(zone.parts.len())?);
            for part in &zone.parts {
                put_u32(&mut out, part.nulls);
                put_bounds(&mut out, part.bounds.as_ref(), t)?;
                put_filter(&mut out, part.filter);
                put_filter(&mut out, part.imprint);
                put_filter(&mut out, part.sketch);
            }
        }
    }
    for (edges, t) in group.edges.iter().zip(types) {
        put_u32(&mut out, count32(edges.len())?);
        for edge in edges {
            put_bound(&mut out, edge, t)?;
        }
    }
    put_places(&mut out, &group.sketched)?;
    for (edges, t) in group.sketches.iter().zip(types) {
        put_u32(&mut out, count32(edges.each().count())?);
        for (text, point) in edges.each() {
            out.push(u8::from(point));
            put_bound(&mut out, &Value::Char(text.into()), t)?;
        }
    }
    Ok(out)
}

/// Appends the places of columns `places`.
fn put_places(out: &mut Vec<u8>, places: &[usize]) -> Result<()> {
    put_u32(out, count32(places.len())?);
    for &place in places {
        put_u32(out, u32::try_from(place).map_err(|_| corrupt("a column's place is too large"))?);
    }
    Ok(())
}

/// Appends where a part's filter or imprints are, if they are.
fn put_filter(out: &mut Vec<u8>, filter: Option<Filter>) {
    match filter {
        Some(f) => {
            out.push(1);
            put_u64(out, f.page.0);
            put_u32(out, f.at);
            put_u32(out, f.len);
        }
        None => out.push(0),
    }
}

/// Why a header ends before its fields do. Its layout changes in place
/// while the format stays 1 (no migration), so a group written by a build
/// of an older layout reads so, as a damaged one does.
const SHORT: &str = "its header ends before its fields do, as one written by a build of an older layout of this format would (reload the table), or a damaged one";

/// A header's bytes, read in order.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let (taken, rest) = self.0.split_at_checked(n).ok_or_else(|| corrupt(SHORT))?;
        self.0 = rest;
        Ok(taken)
    }

    fn byte(&mut self) -> Result<u8> {
        self.take(1)?.first().copied().ok_or_else(|| corrupt(SHORT))
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes(<[u8; 4]>::try_from(b).map_err(|_| corrupt(SHORT))?))
    }

    fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(<[u8; 8]>::try_from(b).map_err(|_| corrupt(SHORT))?))
    }

    fn count(&mut self) -> Result<usize> {
        usize::try_from(self.u32()?).map_err(|_| corrupt("a count is too large"))
    }

    fn run(&mut self) -> Result<Run> {
        Ok(Run { first: self.u32()?, rows: self.u32()?, page: PageId(self.u64()?), at: self.u32()?, len: self.u32()? })
    }

    /// A group's layout: whether the hybrid.
    fn layout(&mut self) -> Result<bool> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(corrupt("its layout is of another format")),
        }
    }

    fn bound(&mut self, t: &DataType) -> Result<Value> {
        let len = self.count()?;
        t.decode_record(self.take(len)?).ok_or_else(|| corrupt("a bound is not of its column's type"))
    }

    fn bounds(&mut self, t: &DataType) -> Result<Option<(Value, Value)>> {
        match self.byte()? {
            0 => Ok(None),
            1 => Ok(Some((self.bound(t)?, self.bound(t)?))),
            _ => Err(corrupt("a chunk's bounds are of another format")),
        }
    }

    fn filter(&mut self) -> Result<Option<Filter>> {
        match self.byte()? {
            0 => Ok(None),
            1 => Ok(Some(Filter { page: PageId(self.u64()?), at: self.u32()?, len: self.u32()? })),
            _ => Err(corrupt("a part's filter is of another format")),
        }
    }

    fn part(&mut self, t: &DataType) -> Result<Part> {
        Ok(Part { nulls: self.u32()?, bounds: self.bounds(t)?, filter: self.filter()?, imprint: self.filter()?, sketch: self.filter()? })
    }

    fn places(&mut self) -> Result<Vec<usize>> {
        (0..self.count()?).map(|_| self.count()).collect()
    }

    /// A column's sketch's edges, of type `t`.
    fn sketch(&mut self, t: &DataType) -> Result<Edges> {
        let (mut texts, mut points) = (Vec::new(), Vec::new());
        for _ in 0..self.count()? {
            points.push(match self.byte()? {
                0 => false,
                1 => true,
                _ => return Err(corrupt("a sketch's edge is of another format")),
            });
            let Value::Char(text) = self.bound(t)? else { return Err(corrupt("a sketch's edge is not a string")) };
            texts.push(text.into());
        }
        if texts.is_empty() {
            return Ok(Edges::default());
        }
        Edges::new(texts, points).ok_or_else(|| corrupt("a sketch's edges are not in order"))
    }

    fn zone(&mut self, t: &DataType) -> Result<Zone> {
        let run = self.run()?;
        let nulls = self.u32()?;
        let bounds = self.bounds(t)?;
        let parts = (0..self.count()?).map(|_| self.part(t)).collect::<Result<Vec<_>>>()?;
        Ok(Zone { run, nulls, bounds, parts })
    }
}

/// The group whose header's chain begins at page `first`, its columns of
/// `types`.
///
/// # Errors
///
/// As `Pages::get`; SQLCODE -916 if its header is malformed, of another
/// format, or of other columns.
pub fn read<S: Seal>(pages: &Pages<'_, S>, first: PageId, types: &[DataType]) -> Result<Group> {
    let (bytes, header) = read_chain(pages, first)?;
    let mut r = Reader(&bytes);
    let format = r.byte()?;
    if format != GROUP_FORMAT {
        return Err(corrupt("its header is of another format"));
    }
    let (first_id, last, rows) = (r.u64()?, r.u64()?, r.u32()?);
    let order = r.places()?;
    let pax = r.layout()?;
    let ids = (0..r.count()?).map(|_| r.run()).collect::<Result<Vec<_>>>()?;
    let by_id = (0..r.count()?).map(|_| r.run()).collect::<Result<Vec<_>>>()?;
    if r.count()? != types.len() {
        return Err(corrupt("its columns are not its table's"));
    }
    let mut columns = Vec::with_capacity(types.len());
    for t in types {
        let zones = (0..r.count()?).map(|_| r.zone(t)).collect::<Result<Vec<_>>>()?;
        columns.push(zones);
    }
    let edges = types.iter().map(|t| (0..r.count()?).map(|_| r.bound(t)).collect::<Result<Vec<_>>>()).collect::<Result<Vec<_>>>()?;
    let named = r.places()?;
    let sketches = types.iter().map(|t| r.sketch(t)).collect::<Result<Vec<_>>>()?;
    if !r.0.is_empty() {
        return Err(corrupt("its header has bytes past its end"));
    }
    let place = (pages.file, pages.base);
    Ok(Group { first: first_id, last, rows, ids, by_id, columns, order, pax, edges, sketched: named, sketches, header, place })
}

/// Appends `summary`, its columns of `types`, as a table's directory holds
/// it:
///
/// ```text
/// summary:  header page u64 | first id u64 | last id u64 | rows u32 |
///           order u32 | each: column u32 | blooms u32 | each: column u32 |
///           sketched u32 | each: column u32 | layout u8 (1 of the hybrid) |
///           columns u32 | each: nulls u32 | bounds u8 | least | greatest
/// ```
///
/// # Errors
///
/// SQLCODE -910 if a bound is not of its column's type; -900 if it is too
/// large.
pub fn put_summary(out: &mut Vec<u8>, summary: &Summary, types: &[DataType]) -> Result<()> {
    put_u64(out, summary.header.0);
    // [DESIGN] RULES.md D1146: its segment, 0 for none, its base and length.
    let placed = summary.extent.map_or([0; 3], |p| [p.segment, p.base, p.length]);
    for n in placed {
        put_u64(out, n);
    }
    put_u64(out, summary.first);
    put_u64(out, summary.last);
    put_u32(out, summary.rows);
    for places in [&summary.order, &summary.blooms, &summary.sketched] {
        put_places(out, places)?;
    }
    out.push(u8::from(summary.pax));
    put_u32(out, count32(summary.columns.len())?);
    for (overall, t) in summary.columns.iter().zip(types) {
        put_u32(out, overall.nulls);
        put_bounds(out, overall.bounds.as_ref(), t)?;
    }
    Ok(())
}

/// The summaries `bytes` hold one after another, their columns of `types`.
///
/// # Errors
///
/// SQLCODE -916 if one is malformed, or of other columns.
pub fn read_summaries(bytes: &[u8], types: &[DataType]) -> Result<Vec<Summary>> {
    let mut r = Reader(bytes);
    let mut out = Vec::new();
    while !r.0.is_empty() {
        let header = PageId(r.u64()?);
        let (segment, base, length) = (r.u64()?, r.u64()?, r.u64()?);
        let extent = (segment != 0).then_some(Placed { segment, base, length });
        let (first, last, rows) = (r.u64()?, r.u64()?, r.u32()?);
        let (order, blooms, sketched) = (r.places()?, r.places()?, r.places()?);
        let pax = r.layout()?;
        if r.count()? != types.len() {
            return Err(corrupt("its summary's columns are not its table's"));
        }
        let columns = types.iter().map(|t| Ok(Overall { nulls: r.u32()?, bounds: r.bounds(t)? })).collect::<Result<Vec<_>>>()?;
        out.push(Summary { header, first, last, rows, order, blooms, sketched, pax, columns, extent });
    }
    Ok(out)
}
