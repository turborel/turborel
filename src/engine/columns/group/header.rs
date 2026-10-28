//! \[DESIGN\] A frozen row group's header, written and read back as
//! `group.rs` describes it. Split from group.rs (RULES.md R-SIZE-2).

use super::{GROUP_FORMAT, Group, Pages, Run, Seal, Zone, corrupt, count32, read_chain};
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

/// The header of `group`, its columns of `types`.
pub(super) fn encode(group: &Group, types: &[DataType]) -> Result<Vec<u8>> {
    let mut out = vec![GROUP_FORMAT];
    put_u64(&mut out, group.first);
    put_u64(&mut out, group.last);
    put_u32(&mut out, group.rows);
    put_u32(&mut out, count32(group.ids.len())?);
    group.ids.iter().for_each(|run| put_run(&mut out, run));
    put_u32(&mut out, count32(group.columns.len())?);
    for (zones, t) in group.columns.iter().zip(types) {
        put_u32(&mut out, count32(zones.len())?);
        for zone in zones {
            put_run(&mut out, &zone.run);
            put_u32(&mut out, zone.nulls);
            match &zone.bounds {
                Some((least, greatest)) => {
                    out.push(1);
                    put_bound(&mut out, least, t)?;
                    put_bound(&mut out, greatest, t)?;
                }
                None => out.push(0),
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

    fn zone(&mut self, t: &DataType) -> Result<Zone> {
        let run = self.run()?;
        let nulls = self.u32()?;
        let bounds = match self.byte()? {
            0 => None,
            1 => Some((self.bound(t)?, self.bound(t)?)),
            _ => return Err(corrupt("a chunk's bounds are of another format")),
        };
        Ok(Zone { run, nulls, bounds })
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
    if r.byte()? != GROUP_FORMAT {
        return Err(corrupt("its header is of another format"));
    }
    let (first_id, last, rows) = (r.u64()?, r.u64()?, r.u32()?);
    let ids = (0..r.count()?).map(|_| r.run()).collect::<Result<Vec<_>>>()?;
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
    Ok(Group { first: first_id, last, rows, ids, columns, header })
}
