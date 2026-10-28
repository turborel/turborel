//! \[DESIGN\] A transaction's changes to a table, and the garbage commits
//! make, as lists that spill (`spill/list.rs`, RULES.md D38): what each
//! holds in memory, and its record. Split from engine/rows/mvcc.rs
//! (RULES.md R-SIZE-2).
//!
//! ```text
//! change:  row id u64, then 1 if it has a position appended, and the
//!          position u64, or 0; then the entry it replaced, if its own
//! garbage: kind u8, then by kind: 0 own version, its row id and bytes;
//!          1 older version, its row id and position; 2 deletion, its row
//!          id; 3 undone, its position (little-endian)
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
        FIXED.saturating_add(self.before.as_ref().map_or(0, Vec::len))
    }
}

impl ListItem for Dead {
    fn bytes(&self) -> usize {
        FIXED.saturating_add(if let Dead::Own(_, bytes) = self { bytes.len() } else { 0 })
    }
}

/// `change` as a record.
pub(super) fn encode_change(change: &Change) -> Vec<u8> {
    let mut out = Vec::new();
    out.put_u64(change.id);
    match change.appended {
        Some(at) => {
            out.put_u8(1);
            out.put_u64(at.0);
        }
        None => out.put_u8(0),
    }
    out.extend_from_slice(change.before.as_deref().unwrap_or_default());
    out
}

/// The change of a record `encode_change` made.
pub(super) fn decode_change(record: &[u8]) -> Result<Change> {
    let mut r = Reader::new(record, "row change");
    let id = r.u64()?;
    let appended = if r.u8()? == 1 { Some(Lsn(r.u64()?)) } else { None };
    let rest = r.rest();
    Ok(Change { id, before: (!rest.is_empty()).then(|| rest.to_vec()), appended })
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
            Change { id: 1, before: None, appended: None },
            Change { id: 2, before: Some(vec![1, 2, 3]), appended: None },
            Change { id: 3, before: None, appended: Some(Lsn(9)) },
        ] {
            let back = decode_change(&encode_change(&change)).unwrap();
            assert_eq!((back.id, back.before.clone(), back.appended), (change.id, change.before.clone(), change.appended));
            assert!(back.bytes() >= FIXED);
        }
        for dead in [Dead::Own(1, vec![4, 5]), Dead::Older(2, Lsn(7)), Dead::Deletion(3), Dead::Undone(Lsn(8))] {
            let record = encode_dead(&dead);
            assert_eq!(encode_dead(&decode_dead(&record).unwrap()), record);
            assert!(dead.bytes() >= FIXED);
        }
        assert_eq!(decode_dead(&[9]).err().map(|e| e.code), Some(Code::Corrupt));
    }
}
