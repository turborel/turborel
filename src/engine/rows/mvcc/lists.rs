//! \[DESIGN\] A transaction's changes to a table, and the garbage commits
//! make, as lists that spill (`spill/list.rs`, RULES.md D38): what each
//! holds in memory, and its record.
//!
//! ```text
//! change:  row id u64, or object id; flags u8, 1 if it has a position
//!          appended, 2 if it made an object, 4 a group; the position u64 if it has
//!          one; the objects it dropped, their count u32 and each id u64;
//!          then the entry it replaced, if its own
//! garbage: kind u8, then by kind: 0 own version, its row id and bytes;
//!          1 older version, its row id and position; 2 deletion, its row
//!          id; 3 undone, its position; 4 objects dropped, their count u32
//!          and each id u64 (little-endian)
//! ```

use super::{Change, Dead};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::log::Lsn;
use crate::spill::ListItem;

/// The bytes of a change, or of garbage, beside the entry it holds.
const FIXED: usize = 48;

impl ListItem for Change {
    fn bytes(&self) -> usize {
        FIXED.saturating_add(self.before.as_ref().map_or(0, Vec::len)).saturating_add(self.dropped.len().saturating_mul(8))
    }
}

impl ListItem for Dead {
    fn bytes(&self) -> usize {
        FIXED.saturating_add(match self {
            Dead::Own(_, bytes) => bytes.len(),
            Dead::Objects(ids) => ids.len().saturating_mul(8),
            Dead::Older(..) | Dead::Deletion(_) | Dead::Undone(_) => 0,
        })
    }
}

/// The flag of a change that made an object.
const MADE: u8 = 2;
/// The flag of a change that made a group (`frozen/stamped.rs`).
const GROUP: u8 = 4;

/// Writes `ids`, their count then each.
fn put_ids(ids: &[u64], out: &mut Vec<u8>) {
    out.put_u32(u32::try_from(ids.len()).unwrap_or(u32::MAX));
    for &id in ids {
        out.put_u64(id);
    }
}

/// The ids `put_ids` wrote.
fn read_ids(r: &mut Reader<'_>) -> Result<Vec<u64>> {
    let count = r.u32()?;
    (0..count).map(|_| r.u64()).collect()
}

/// `change` as a record.
pub(super) fn encode_change(change: &Change) -> Vec<u8> {
    let mut out = Vec::new();
    out.put_u64(change.id);
    out.put_u8(u8::from(change.appended.is_some()) | if change.made { MADE } else { 0 } | if change.group { GROUP } else { 0 });
    if let Some(at) = change.appended {
        out.put_u64(at.0);
    }
    put_ids(&change.dropped, &mut out);
    out.extend_from_slice(change.before.as_deref().unwrap_or_default());
    out
}

/// The change of a record `encode_change` made.
pub(super) fn decode_change(record: &[u8]) -> Result<Change> {
    let mut r = Reader::new(record, "row change");
    let id = r.u64()?;
    let flags = r.u8()?;
    let appended = if flags & 1 == 1 { Some(Lsn(r.u64()?)) } else { None };
    let dropped = read_ids(&mut r)?;
    let rest = r.rest();
    let (made, group) = (flags & MADE == MADE, flags & GROUP == GROUP);
    Ok(Change { id, before: (!rest.is_empty()).then(|| rest.to_vec()), appended, dropped, made, group })
}

/// `dead` as a record.
pub(super) fn encode_dead(dead: &Dead) -> Vec<u8> {
    let mut out = Vec::new();
    match dead {
        Dead::Own(id, bytes) => {
            out.put_u8(0);
            out.put_u64(*id);
            out.extend_from_slice(bytes);
        }
        Dead::Older(id, at) => {
            out.put_u8(1);
            out.put_u64(*id);
            out.put_u64(at.0);
        }
        Dead::Deletion(id) => {
            out.put_u8(2);
            out.put_u64(*id);
        }
        Dead::Undone(at) => {
            out.put_u8(3);
            out.put_u64(at.0);
        }
        Dead::Objects(ids) => {
            out.put_u8(4);
            put_ids(ids, &mut out);
        }
    }
    out
}

/// The garbage of a record `encode_dead` made.
pub(super) fn decode_dead(record: &[u8]) -> Result<Dead> {
    let mut r = Reader::new(record, "row garbage");
    let dead = match r.u8()? {
        0 => Dead::Own(r.u64()?, r.rest().to_vec()),
        1 => Dead::Older(r.u64()?, Lsn(r.u64()?)),
        2 => Dead::Deletion(r.u64()?),
        3 => Dead::Undone(Lsn(r.u64()?)),
        4 => Dead::Objects(read_ids(&mut r)?),
        _ => return Err(r.corrupt("not a kind of garbage")),
    };
    r.finish()?;
    Ok(dead)
}

// Unit tests: each record read back as written (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;

    /// Verifies: [DESIGN] RULES.md D38: each kind of change and of garbage
    /// is read back from its record as it was written; a record of no kind
    /// of garbage is SQLCODE -916.
    #[test]
    fn records_are_read_back_as_written() {
        for change in [
            Change::of_row(1, None, None, Vec::new()),
            Change::of_row(2, Some(vec![1, 2, 3]), None, vec![7, 8]),
            Change::of_row(3, None, Some(Lsn(9)), vec![5]),
            Change { id: 4, before: None, appended: None, dropped: Vec::new(), made: true, group: false },
            Change { id: 5, before: None, appended: None, dropped: Vec::new(), made: false, group: true },
        ] {
            let back = decode_change(&encode_change(&change)).unwrap();
            assert_eq!(
                (back.id, back.before.clone(), back.appended, back.dropped.clone(), back.made, back.group),
                (change.id, change.before.clone(), change.appended, change.dropped.clone(), change.made, change.group)
            );
            assert!(back.bytes() >= FIXED);
        }
        for dead in [Dead::Own(1, vec![4, 5]), Dead::Older(2, Lsn(7)), Dead::Deletion(3), Dead::Undone(Lsn(8)), Dead::Objects(vec![3, 4])] {
            let record = encode_dead(&dead);
            assert_eq!(encode_dead(&decode_dead(&record).unwrap()), record);
            assert!(dead.bytes() >= FIXED);
        }
        assert_eq!(decode_dead(&[9]).err().map(|e| e.code), Some(Code::Corrupt));
    }
}
