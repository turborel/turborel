//! \[DESIGN\] A frozen row group's header, written and read back as
//! `group.rs` describes it. Split from group.rs (RULES.md R-SIZE-2).

use super::{GROUP_FORMAT, Group, Pages, Part, Run, Seal, Zone, corrupt, count32, read_chain};
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
    put_u32(&mut out, count32(group.order.len())?);
    for &place in &group.order {
        put_u32(&mut out, u32::try_from(place).map_err(|_| corrupt("a column's place is too large"))?);
    }
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
            }
        }
    }
    Ok(out)
}

/// A header's bytes, read in order.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let (taken, rest) = self.0.split_at_checked(n).ok_or_else(|| corrupt("its header is too short"))?;
        self.0 = rest;
        Ok(taken)
    }

    fn byte(&mut self) -> Result<u8> {
        self.take(1)?.first().copied().ok_or_else(|| corrupt("its header is too short"))
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes(<[u8; 4]>::try_from(b).map_err(|_| corrupt("its header is too short"))?))
    }

    fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(<[u8; 8]>::try_from(b).map_err(|_| corrupt("its header is too short"))?))
    }

    fn count(&mut self) -> Result<usize> {
        usize::try_from(self.u32()?).map_err(|_| corrupt("a count is too large"))
    }

    fn run(&mut self) -> Result<Run> {
        Ok(Run { first: self.u32()?, rows: self.u32()?, page: PageId(self.u64()?) })
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

    fn zone(&mut self, t: &DataType) -> Result<Zone> {
        let run = self.run()?;
        let nulls = self.u32()?;
        let bounds = self.bounds(t)?;
        let parts = (0..self.count()?).map(|_| Ok(Part { nulls: self.u32()?, bounds: self.bounds(t)? })).collect::<Result<Vec<_>>>()?;
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
    let order = (0..r.count()?).map(|_| r.count()).collect::<Result<Vec<usize>>>()?;
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
    if !r.0.is_empty() {
        return Err(corrupt("its header has bytes past its end"));
    }
    Ok(Group { first: first_id, last, rows, ids, by_id, columns, order, header })
}
