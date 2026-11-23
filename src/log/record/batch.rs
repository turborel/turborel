//! \[EXTENSION\] Rows inserted together as one record (RULES.md D530): a
//! batch of an INSERT or a bulk copy, so its rows cost the log one append,
//! not one each. Redone as each of its rows would be as an insert record.
//! Here too the records of one row's insert, update and delete made from
//! the rows as their statement holds them, none copied for its record
//! (RULES.md D531, D750).
//!
//! ```text
//! insert rows (26): table | generation u32 | rows (count u32, each: row
//!              id u64 | after-image row)
//! ```

use super::kinds::{DELETE, INSERT, INSERT_ROWS, UPDATE};
use super::{Change, RECORD_VERSION, put_row_change, put_table, table};
use crate::bytes::{Put, Reader};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::storage::{AfterImage, Row, RowId};
use crate::txn::TxId;
use crate::value::codec::{put_row, row, row_len};

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
    let images = rows.iter().fold(0_usize, |n, r| n.saturating_add(size_of::<RowId>()).saturating_add(row_len(r)));
    let mut out = Vec::with_capacity(head_len(table).saturating_add(images));
    out.put_u8(RECORD_VERSION);
    out.put_u8(INSERT_ROWS);
    out.put_u64(tx);
    put_rows(&mut out, (table, generation), ids.iter().copied().zip(rows));
    out
}

/// \[DESIGN\] The bytes of the record of transaction `tx` inserting `row`
/// as row `id` into `table`'s storage of `generation`, as `Record::encode`
/// writes a `Change::Insert`, made from the row as it is held, so it is not
/// copied for it (RULES.md D531).
pub fn insert_record(tx: TxId, table: (&QualifiedName, u32), id: RowId, row: &Row) -> Vec<u8> {
    let mut out = Vec::new();
    put_insert_record(&mut out, tx, table, id, row);
    out
}

/// \[DESIGN\] (RULES.md D882) As `insert_record`, its bytes appended to
/// `out`: the buffer a transaction holds its records in, given room for
/// the record first.
pub fn put_insert_record(out: &mut Vec<u8>, tx: TxId, (table, generation): (&QualifiedName, u32), id: RowId, row: &Row) {
    out.reserve(head_len(table).saturating_add(row_len(row)));
    out.put_u8(RECORD_VERSION);
    out.put_u8(INSERT);
    out.put_u64(tx);
    put_row_change(out, (table, generation, id), &[row]);
}

/// \[DESIGN\] As `insert_record`, of a `Change::Update` of row `id` made
/// `after` from `before` (RULES.md D750).
pub fn update_record(tx: TxId, table: (&QualifiedName, u32), id: RowId, images: (&Row, &Row)) -> Vec<u8> {
    let mut out = Vec::new();
    put_update_record(&mut out, tx, table, id, images);
    out
}

/// \[DESIGN\] (RULES.md D882) As `update_record`, appended to `out` as
/// `put_insert_record` appends.
pub fn put_update_record(
    out: &mut Vec<u8>,
    tx: TxId,
    (table, generation): (&QualifiedName, u32),
    id: RowId,
    (before, after): (&Row, &Row),
) {
    // At most the row before whole, each of its columns with its position.
    let images = row_len(after).saturating_add(row_len(before)).saturating_add(before.len().saturating_mul(POSITION_LEN));
    out.reserve(head_len(table).saturating_add(images));
    out.put_u8(RECORD_VERSION);
    out.put_u8(UPDATE);
    out.put_u64(tx);
    super::images::put_update(out, (table, generation, id), before, after);
}

/// \[DESIGN\] (RULES.md D875) As `put_update_record`, of a change that may
/// differ only in the columns `set`, in ascending order, each once. Of
/// the row before only those are read (`images::put_update_of`), so one
/// holding their values alone gives the bytes the row whole gives.
pub fn put_update_record_of(
    out: &mut Vec<u8>,
    tx: TxId,
    (table, generation): (&QualifiedName, u32),
    id: RowId,
    (before, after, set): (&Row, &Row, &[usize]),
) {
    // At most the row before as given, each column set with its position.
    let images = row_len(after).saturating_add(row_len(before)).saturating_add(set.len().saturating_mul(POSITION_LEN));
    out.reserve(head_len(table).saturating_add(images));
    out.put_u8(RECORD_VERSION);
    out.put_u8(UPDATE);
    out.put_u64(tx);
    super::images::put_update_of(out, (table, generation, id), before, after, set);
}

/// \[DESIGN\] (RULES.md D916) As `put_update_record_of`, the row after
/// given as `image` (`images::put_update_image`): the bytes it writes of
/// the row the image is of.
pub fn put_update_record_image(
    out: &mut Vec<u8>,
    tx: TxId,
    (table, generation): (&QualifiedName, u32),
    id: RowId,
    (before, image): (&Row, &AfterImage),
) {
    let (values, _, set) = image.parts();
    // At most the row before as given, each column set with its position.
    let images = values.len().saturating_add(row_len(before)).saturating_add(set.len().saturating_mul(POSITION_LEN));
    out.reserve(head_len(table).saturating_add(images));
    out.put_u8(RECORD_VERSION);
    out.put_u8(UPDATE);
    out.put_u64(tx);
    super::images::put_update_image(out, (table, generation, id), before, image);
}

/// \[DESIGN\] As `insert_record`, of a `Change::Delete` of row `id`, which
/// was `before` (RULES.md D750).
pub fn delete_record(tx: TxId, table: (&QualifiedName, u32), id: RowId, before: &Row) -> Vec<u8> {
    let mut out = Vec::new();
    put_delete_record(&mut out, tx, table, id, before);
    out
}

/// \[DESIGN\] (RULES.md D882) As `delete_record`, appended to `out` as
/// `put_insert_record` appends.
pub fn put_delete_record(out: &mut Vec<u8>, tx: TxId, (table, generation): (&QualifiedName, u32), id: RowId, before: &Row) {
    out.reserve(head_len(table).saturating_add(row_len(before)));
    out.put_u8(RECORD_VERSION);
    out.put_u8(DELETE);
    out.put_u64(tx);
    put_row_change(out, (table, generation, id), &[before]);
}

/// The bytes of a changed column's position in an update's row before.
const POSITION_LEN: usize = size_of::<u32>();

/// The bytes of a row's record before its images: its version, its kind,
/// its transaction, its table's two names each after its length, its
/// generation, its row id, and the tag of its row before. The images'
/// bytes follow, as `codec::row_len` counts them, so a record is written
/// into one allocation, not grown as it is written.
fn head_len(table: &QualifiedName) -> usize {
    HEAD_FIXED_LEN.saturating_add(table.auth.len()).saturating_add(table.ident.len())
}

/// The bytes of `head_len` but its table's names.
const HEAD_FIXED_LEN: usize = size_of::<u8>() * 3 + size_of::<u64>() * 2 + size_of::<u32>() * 4;
