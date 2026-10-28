//! \[DESIGN\] Log records of a table with encodings, sealed through them
//! (RULES.md D25, D48), so the log holds a row of an encrypted table no
//! more plainly than its data file does. The record is sealed whole, its
//! row id the context, behind a mark a record's format version never is:
//!
//! ```text
//! sealed record:  SEALED u8 | row id u64 | the record (`record.rs`), sealed
//!                 (`disk/encoding/registry.rs`)
//! ```
//!
//! The sealed data names its encodings, so recovery reads it through them,
//! whatever the table's options are by then.

use std::borrow::Cow;

use super::record::Record;
use crate::bytes::{Put, Reader};
use crate::disk::encoding::{Encodings, Pipeline};
use crate::error::Result;
use crate::storage::RowId;

/// The first byte of a sealed record: no format version of a record.
const SEALED: u8 = 0xFE;

/// `record`, of row `row`, sealed through `pipeline`.
///
/// # Errors
///
/// An encoding's errors.
pub fn seal(record: &Record, row: RowId, pipeline: &Pipeline) -> Result<Vec<u8>> {
    let plain = record.encode();
    let mut out = Vec::with_capacity(plain.len().saturating_add(16));
    out.put_u8(SEALED);
    out.put_u64(row);
    pipeline.seal(row, &plain, &mut out)?;
    Ok(out)
}

/// The record `bytes` hold, sealed or not, read through `encodings`.
///
/// # Errors
///
/// As `Record::decode`; as `Encodings::unseal`.
pub fn open(bytes: &[u8], encodings: &Encodings) -> Result<Record> {
    let plain: Cow<'_, [u8]> = match bytes.first() {
        Some(&SEALED) => {
            let mut r = Reader::new(bytes.get(1..).unwrap_or_default(), "sealed log record");
            let row = r.u64()?;
            Cow::Owned(encodings.unseal(row, r.rest())?.into_owned())
        }
        Some(_) | None => Cow::Borrowed(bytes),
    };
    Record::decode(&plain)
}
