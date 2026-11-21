//! \[EXTENSION\] Rows inserted together as one record (RULES.md D530): a
//! batch of an INSERT or a bulk copy, so its rows cost the log one append,
//! not one each. Redone as each of its rows would be as an insert record.
//!
//! ```text
//! insert rows (26): table | generation u32 | rows (count u32, each: row
//!              id u64 | after-image row)
//! ```

use super::kinds::INSERT_ROWS;
use super::{Change, RECORD_VERSION, put_table, table};
use crate::bytes::{Put, Reader};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::storage::{Row, RowId};
use crate::txn::TxId;
use crate::value::codec::{put_row, row};

/// Writes the body of a record of `rows`, each a row id and the row
/// inserted with it, into `table`'s storage of `generation`.
pub(super) fn put_rows<'r>(
    out: &mut Vec<u8>,
    (table, generation): (&QualifiedName, u32),
    rows: impl ExactSizeIterator<Item = (RowId, &'r Row)>,
) {
    put_table(out, table);
    out.put_u32(generation);
    out.put_u32(u32::try_from(rows.len()).unwrap_or(u32::MAX));
    for (id, after) in rows {
        out.put_u64(id);
        put_row(out, after);
    }
}

/// Writes the body of `change`, if it is a `Change::InsertRows`.
pub(super) fn put_change(out: &mut Vec<u8>, change: &Change) {
    if let Change::InsertRows { table, generation, rows } = change {
        put_rows(out, (table, *generation), rows.iter().map(|(id, row)| (*id, row)));
    }
}

/// The change `put_rows` wrote.
pub(super) fn rows_change(r: &mut Reader<'_>) -> Result<Change> {
    let (table, generation, count) = (table(r)?, r.u32()?, r.u32()?);
    let mut rows = Vec::new();
    for _ in 0..count {
        rows.push((r.u64()?, row(r)?));
    }
    Ok(Change::InsertRows { table, generation, rows })
}

/// \[EXTENSION\] The bytes of the record of transaction `tx` inserting
/// `rows` into `table`'s storage of `generation`, as `Record::encode`
/// writes a `Change::InsertRows`, made from the rows as they are held, so
/// none is copied for it (RULES.md D530).
pub fn insert_rows_record(tx: TxId, (table, generation): (&QualifiedName, u32), ids: &[RowId], rows: &[Row]) -> Vec<u8> {
    let mut out = Vec::new();
    out.put_u8(RECORD_VERSION);
    out.put_u8(INSERT_ROWS);
    out.put_u64(tx);
    put_rows(&mut out, (table, generation), ids.iter().copied().zip(rows));
    out
}
