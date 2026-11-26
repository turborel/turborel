//! \[DESIGN\] CopyColumns (RULES.md D1148): a batch of a copy's rows by
//! column, each cell the text a field of the file holds, so a client
//! makes no value of a cell and the server parses each column straight
//! into its type.
//!
//! ```text
//! 'O' copy columns: rows u32, columns u32, then of each column its
//!                   nulls (bytes, a bit a row, the first row's the low
//!                   bit of the first byte), its ends (a count u32 then
//!                   each u32, where each cell's text ends) and its text
//!                   (bytes, every cell's end to end)
//! ```
//!
//! A peer's frame is checked whole before anything takes it: each column
//! has a bit and an end for each row, no bit past the last row, ends in
//! order, a null's where the cell before it ends, and the last the
//! text's length. A frame of rows has a column, so a false count of rows
//! cannot make the server hold more than it was sent.

use turborel::bytes::{Put, Reader};
use turborel::error::Result;
use turborel::storage::lanes::{Lane, Lanes};

use crate::fields::put_count;

/// The kind of CopyColumns.
pub(super) const COPY_COLUMNS: u8 = b'O';
/// The bytes of an end, a u32.
const END_BYTES: usize = 4;
/// The null bits a byte holds.
const BITS: usize = 8;
/// The bits of a row's number that say its bit within its byte.
const WITHIN: usize = BITS - 1;

/// Appends `lanes`, CopyColumns' body, to `b`.
pub(super) fn put(b: &mut Vec<u8>, lanes: &Lanes) {
    put_count(b, lanes.rows);
    put_count(b, lanes.columns.len());
    for lane in &lanes.columns {
        b.put_bytes(&lane.nulls);
        put_count(b, lane.ends.len());
        b.reserve(lane.ends.len().saturating_mul(END_BYTES).saturating_add(lane.bytes.len()));
        for end in &lane.ends {
            b.put_u32(*end);
        }
        b.put_bytes(&lane.bytes);
    }
}

/// The rows by column CopyColumns' body `r` holds.
///
/// # Errors
///
/// `Reader::corrupt`'s, if it ends too soon, or a column is not as the
/// module says.
pub(super) fn read(r: &mut Reader<'_>) -> Result<Lanes> {
    let rows = usize::try_from(r.u32()?).map_err(|_| r.corrupt("a count of rows is too large"))?;
    let count = r.u32()?;
    if count == 0 && rows > 0 {
        return Err(r.corrupt("a CopyColumns of rows has no column"));
    }
    // Not reserved for `count`: each column read takes bytes of the frame.
    let mut columns = Vec::new();
    for _ in 0..count {
        columns.push(lane(r, rows)?);
    }
    Ok(Lanes { rows, columns })
}

/// A column of `rows` cells, read from `r`.
fn lane(r: &mut Reader<'_>, rows: usize) -> Result<Lane> {
    let nulls = r.bytes()?.to_vec();
    if nulls.len() != rows.div_ceil(BITS) || !spare_clear(&nulls, rows) {
        return Err(r.corrupt("a column has other than a null bit for each row"));
    }
    let count = usize::try_from(r.u32()?).map_err(|_| r.corrupt("a count of ends is too large"))?;
    if count != rows {
        return Err(r.corrupt("a column has other than an end for each row"));
    }
    let raw = r.take(count.checked_mul(END_BYTES).ok_or_else(|| r.corrupt("a count of ends is too large"))?)?;
    // `raw` is a whole number of ends, as `take` gave exactly their bytes.
    let ends = raw.as_chunks::<END_BYTES>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
    let lane = Lane { nulls, ends, bytes: r.bytes()?.to_vec() };
    if !ordered(&lane) {
        return Err(r.corrupt("a column's ends are out of order, or not its text's"));
    }
    Ok(lane)
}

/// Whether the bits of `nulls` past its `rows`th are clear.
fn spare_clear(nulls: &[u8], rows: usize) -> bool {
    let used = rows & WITHIN;
    used == 0 || nulls.last().is_none_or(|last| last.checked_shr(u32::try_from(used).unwrap_or(0)).unwrap_or(0) == 0)
}

/// Whether `lane`'s ends rise, each null's where the cell before it ends,
/// and the last is its text's length.
fn ordered(lane: &Lane) -> bool {
    let mut before = 0_u32;
    for (n, &end) in lane.ends.iter().enumerate() {
        if end < before || (lane.is_null(n) && end != before) {
            return false;
        }
        before = end;
    }
    usize::try_from(before).is_ok_and(|last| last == lane.bytes.len())
}
