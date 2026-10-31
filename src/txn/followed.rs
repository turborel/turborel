//! \[DESIGN\] The records a following database holds of a transaction not
//! yet ended (RULES.md D38, D57; `db/standby.rs`): each as its log holds
//! it, sealed or not, with its position, in a list that spills
//! (`spill/list.rs`), so a long transaction of the primary is held in a
//! work area's bytes (MEMORY.WORK_BYTES), as its undo entries are on the
//! primary. Kept sealed, a row of an encrypted table is held, and written
//! to a spill area, no more plainly than its log holds it. Split from
//! txn.rs (RULES.md R-SIZE-2).
//!
//! ```text
//! held record: position u64 | the record's bytes, as its log holds them
//! ```

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::log::Lsn;
use crate::spill::{ListItem, ListSpace, SpillList};

/// A record a following database holds: its position, and its bytes as
/// its log holds them.
pub(crate) struct Followed {
    /// Its position in the log.
    pub(crate) at: Lsn,
    /// Its bytes, sealed or not.
    pub(crate) bytes: Vec<u8>,
}

impl ListItem for Followed {
    fn bytes(&self) -> usize {
        size_of::<Followed>().saturating_add(self.bytes.len())
    }
}

/// \[DESIGN\] The records held of one transaction, in position order.
pub(crate) struct FollowedRecords {
    list: SpillList<Followed>,
}

impl FollowedRecords {
    /// None yet, spilling in `space`.
    pub(crate) fn new(space: ListSpace) -> FollowedRecords {
        FollowedRecords { list: SpillList::new(space) }
    }

    /// Holds `record`, after those held.
    ///
    /// # Errors
    ///
    /// The spill area's, SQLCODE -918 if it cannot be reserved: the record
    /// is held all the same, in memory.
    pub(crate) fn push(&mut self, record: Followed) -> Result<()> {
        self.list.push(record, &mut encode)
    }

    /// Its records, in position order, read back a segment at a time.
    pub(crate) fn into_records(self) -> impl Iterator<Item = Result<Followed>> {
        self.list.into_segments(false, &decode).flat_map(|segment| match segment {
            Ok(records) => records.into_iter().map(Ok).collect::<Vec<_>>(),
            Err(e) => vec![Err(e)],
        })
    }
}

/// `record` as a record of a run.
fn encode(record: &Followed) -> Vec<u8> {
    let mut out = Vec::with_capacity(record.bytes.len().saturating_add(8));
    out.put_u64(record.at.0);
    out.extend_from_slice(&record.bytes);
    out
}

/// The held record `encode` wrote.
fn decode(bytes: &[u8]) -> Result<Followed> {
    let mut r = Reader::new(bytes, "held log record");
    let at = Lsn(r.u64()?);
    Ok(Followed { at, bytes: r.rest().to_vec() })
}
