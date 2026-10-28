//! \[DESIGN\] The row changes recovery reads from the log (RULES.md D25,
//! D38), kept in a list that spills (`spill/list.rs`) so recovery holds a
//! work area's bytes of them however long the log it reads: undone newest
//! first, redone oldest first, then read twice more to mend the indexes.
//! Split from db/recovery.rs (RULES.md R-SIZE-2).
//!
//! ```text
//! change: position u64 | transaction u64 | table | generation u32 |
//!         row id u64 | before (0, or 1 and a row) | after (likewise)
//! ```

use crate::bytes::{Put, Reader};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::log::Lsn;
use crate::spill::{ListItem, SpillList, row_bytes};
use crate::storage::{Row, RowId};
use crate::txn::TxId;
use crate::value::codec::{put_row, row};

/// A row change read from the log.
pub(in crate::db) struct RowChange {
    pub(super) lsn: Lsn,
    pub(super) tx: TxId,
    pub(super) table: QualifiedName,
    /// The generation of the table's storage it was made in.
    pub(super) generation: u32,
    pub(super) row: RowId,
    pub(super) before: Option<Row>,
    pub(super) after: Option<Row>,
}

/// The row changes recovery read: where an index reopened from a
/// checkpoint may be wrong (`State::mend_indexes`).
pub(in crate::db) type Touched = SpillList<RowChange>;

impl ListItem for RowChange {
    fn bytes(&self) -> usize {
        let images = self.before.iter().chain(&self.after).map(row_bytes).sum::<usize>();
        size_of::<RowChange>().saturating_add(images)
    }
}

/// `change` as a record.
pub(super) fn encode_change(change: &RowChange) -> Vec<u8> {
    let mut out = Vec::new();
    out.put_u64(change.lsn.0);
    out.put_u64(change.tx);
    out.put_str(&change.table.auth);
    out.put_str(&change.table.ident);
    out.put_u32(change.generation);
    out.put_u64(change.row);
    for image in [&change.before, &change.after] {
        match image {
            Some(values) => {
                out.put_u8(1);
                put_row(&mut out, values);
            }
            None => out.put_u8(0),
        }
    }
    out
}

/// The change of a record `encode_change` made.
pub(super) fn decode_change(record: &[u8]) -> Result<RowChange> {
    let mut r = Reader::new(record, "recovered row change");
    let (lsn, tx) = (Lsn(r.u64()?), r.u64()?);
    let table = QualifiedName { auth: r.str()?, ident: r.str()? };
    let (generation, id) = (r.u32()?, r.u64()?);
    let mut image = || -> Result<Option<Row>> { if r.u8()? == 1 { row(&mut r).map(Some) } else { Ok(None) } };
    let (before, after) = (image()?, image()?);
    r.finish()?;
    Ok(RowChange { lsn, tx, table, generation, row: id, before, after })
}
