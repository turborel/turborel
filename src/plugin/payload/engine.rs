//! \[DESIGN\] The payloads of a storage engine's calls (RULES.md D224,
//! D225). A table is named by the locator CREATE gave; its rows by ids
//! the database gives, from 1; a row is its values, as a row of a
//! routine's (`wire::put_rows`).
//!
//! ```text
//! CREATE      table str | columns: count u32 | name str...      -> locator bytes
//! OPEN        locator bytes                                     -> found u8 | next u64 | rows u64 | durable u64
//! SCAN        locator bytes | after u64 | max u32               -> count u32 | (id u64 | row)... | more u8
//! GET         locator bytes | count u32 | id u64...             -> count u32 | (found u8 | row if found)...
//! APPLY       locator bytes | count u32 | (id u64 | change)...  -> count u32 | before...
//! CHECKPOINT  locator bytes | through u64                       -> (nothing)
//! DROP        locator bytes                                     -> (nothing)
//! change, before: u8 0 (no row) | u8 1 and row
//! ```
//!
//! SCAN gives rows in ascending order of id, each id above `after`, at
//! most `max`. APPLY makes each row hold its change, all or none of them,
//! and answers what each held before; applying it twice is as once. OPEN
//! answers `next`, above every id ever applied, the rows held, and the
//! `through` of the last CHECKPOINT, which the program keeps with its
//! rows, durable: every change applied before it was asked is durable.

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::plugin::values::{put_values, values};
use crate::storage::{Row, RowId};

use super::count;

/// What OPEN answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Opened {
    /// Whether the table was found.
    pub found: bool,
    /// An id above every id ever applied.
    pub next: RowId,
    /// The rows it holds.
    pub rows: u64,
    /// The `through` of its last CHECKPOINT.
    pub durable: u64,
}

/// `o`, written.
pub fn put_opened(out: &mut Vec<u8>, o: &Opened) {
    out.put_u8(u8::from(o.found));
    out.put_u64(o.next);
    out.put_u64(o.rows);
    out.put_u64(o.durable);
}

/// What `put_opened` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn opened(r: &mut Reader<'_>) -> Result<Opened> {
    Ok(Opened { found: r.u8()? != 0, next: r.u64()?, rows: r.u64()?, durable: r.u64()? })
}

/// A row or none, written.
///
/// # Errors
///
/// As `put_values`.
pub fn put_maybe(out: &mut Vec<u8>, row: Option<&Row>) -> Result<()> {
    match row {
        None => out.put_u8(0),
        Some(row) => {
            out.put_u8(1);
            put_values(out, row)?;
        }
    }
    Ok(())
}

/// What `put_maybe` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn maybe(r: &mut Reader<'_>) -> Result<Option<Row>> {
    if r.u8()? == 0 { Ok(None) } else { values(r).map(Some) }
}

/// Rows with their ids, written.
///
/// # Errors
///
/// As `put_values`.
pub fn put_identified(out: &mut Vec<u8>, rows: &[(RowId, Row)]) -> Result<()> {
    out.put_u32(count(rows.len()));
    for (id, row) in rows {
        out.put_u64(*id);
        put_values(out, row)?;
    }
    Ok(())
}

/// What `put_identified` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn identified(r: &mut Reader<'_>) -> Result<Vec<(RowId, Row)>> {
    (0..r.u32()?).map(|_| Ok((r.u64()?, values(r)?))).collect()
}

/// SCAN's payload.
pub fn put_scan(out: &mut Vec<u8>, locator: &[u8], after: RowId, max: u32) {
    out.put_bytes(locator);
    out.put_u64(after);
    out.put_u32(max);
}

/// What `put_scan` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn scan(r: &mut Reader<'_>) -> Result<(Vec<u8>, RowId, u32)> {
    Ok((r.bytes()?.to_vec(), r.u64()?, r.u32()?))
}

/// GET's payload.
pub fn put_ids(out: &mut Vec<u8>, locator: &[u8], ids: &[RowId]) {
    out.put_bytes(locator);
    out.put_u32(count(ids.len()));
    for id in ids {
        out.put_u64(*id);
    }
}

/// What `put_ids` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn ids(r: &mut Reader<'_>) -> Result<(Vec<u8>, Vec<RowId>)> {
    let locator = r.bytes()?.to_vec();
    Ok((locator, (0..r.u32()?).map(|_| r.u64()).collect::<Result<_>>()?))
}

/// Rows or none, counted: GET's answer, and APPLY's.
///
/// # Errors
///
/// As `put_values`.
pub fn put_maybes(out: &mut Vec<u8>, rows: &[Option<Row>]) -> Result<()> {
    out.put_u32(count(rows.len()));
    rows.iter().try_for_each(|row| put_maybe(out, row.as_ref()))
}

/// What `put_maybes` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn maybes(r: &mut Reader<'_>) -> Result<Vec<Option<Row>>> {
    (0..r.u32()?).map(|_| maybe(r)).collect()
}

/// APPLY's payload.
///
/// # Errors
///
/// As `put_values`.
pub fn put_changes(out: &mut Vec<u8>, locator: &[u8], changes: &[(RowId, Option<Row>)]) -> Result<()> {
    out.put_bytes(locator);
    out.put_u32(count(changes.len()));
    for (id, row) in changes {
        out.put_u64(*id);
        put_maybe(out, row.as_ref())?;
    }
    Ok(())
}

/// Rows written, each by its id: none if deleted.
pub type Changes = Vec<(RowId, Option<Row>)>;

/// What `put_changes` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn changes(r: &mut Reader<'_>) -> Result<(Vec<u8>, Changes)> {
    let locator = r.bytes()?.to_vec();
    Ok((locator, (0..r.u32()?).map(|_| Ok((r.u64()?, maybe(r)?))).collect::<Result<_>>()?))
}
