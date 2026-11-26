//! \[EXTENSION\] A load's group as one record (RULES.md D1146): the group's
//! extent, as its segment holds it, after where it goes. Redone, a
//! committed load's group is appended to a segment of its table again
//! (`Loading::redo_group`); so a standby, and a recovery to a point in time
//! from the archive, make the group from the log alone. Its rows are in no
//! other record. The extent is the record's last field, after its
//! length, so its record is written as its head and the extent's own
//! buffer, the extent copied for neither (`Txn::log_parts`).
//!
//! ```text
//! group loaded (27): table | generation u32 | first id u64 | last id
//!                    u64 | extent bytes u64 | extent
//! ```

use super::kinds::GROUP_LOADED;
use super::{Change, RECORD_VERSION, put_table, table};
use crate::bytes::{Put, Reader};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::storage::RowId;
use crate::txn::TxId;

/// \[EXTENSION\] A load's group (RULES.md D1146).
#[derive(Debug, Clone, PartialEq)]
pub struct GroupLoaded {
    /// Its table.
    pub table: QualifiedName,
    /// The generation of the table's storage (`TableDesc::generation`).
    pub generation: u32,
    /// Its first row id.
    pub first: RowId,
    /// Its last.
    pub last: RowId,
    /// Its extent (`group/sink.rs`).
    pub extent: Vec<u8>,
}

/// The head of a group's record before its extent: so its fields.
fn put_head(out: &mut Vec<u8>, (table, generation): (&QualifiedName, u32), (first, last): (RowId, RowId), extent: usize) {
    put_table(out, table);
    out.put_u32(generation);
    out.put_u64(first);
    out.put_u64(last);
    out.put_u64(u64::try_from(extent).unwrap_or(u64::MAX));
}

/// Writes the body of `loaded`.
pub(super) fn put_change(out: &mut Vec<u8>, loaded: &GroupLoaded) {
    put_head(out, (&loaded.table, loaded.generation), (loaded.first, loaded.last), loaded.extent.len());
    out.extend_from_slice(&loaded.extent);
}

/// The change `put_change` wrote.
pub(super) fn loaded_change(r: &mut Reader<'_>) -> Result<Change> {
    let (table, generation, first, last) = (table(r)?, r.u32()?, r.u64()?, r.u64()?);
    let length = usize::try_from(r.u64()?).unwrap_or(usize::MAX);
    let extent = r.rest();
    if extent.len() != length {
        return Err(r.corrupt("a group's record is not as long as its extent says"));
    }
    Ok(Change::GroupLoaded(GroupLoaded { table, generation, first, last, extent: extent.to_vec() }))
}

/// \[EXTENSION\] The bytes of the record of transaction `tx` loading a
/// group of rows `first` to `last` into `table`'s storage of `generation`,
/// before its extent of `extent` bytes: the record is these and the extent
/// end to end, as `Record::encode` writes a `Change::GroupLoaded`.
pub fn group_record_head(tx: TxId, (table, generation): (&QualifiedName, u32), rows: (RowId, RowId), extent: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.put_u8(RECORD_VERSION);
    out.put_u8(GROUP_LOADED);
    out.put_u64(tx);
    put_head(&mut out, (table, generation), rows, extent);
    out
}
