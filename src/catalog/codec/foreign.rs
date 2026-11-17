//! A referential constraint's description [ISO/IEC 9075-2:2023 §11.8
//! GR2]: its columns, the table and columns it references, its match type
//! and actions, and of a temporal one its two periods' names.

use super::triggers::{optional, put_optional};
use super::{ACTIONS, MATCHES, of_tag, positions, put_name, put_positions, qualified, tag_of};
use crate::bytes::{Put, Reader};
use crate::catalog::ForeignKey;
use crate::error::{Result, SqlError};

/// Writes `fk`.
pub(super) fn put_foreign_key(out: &mut Vec<u8>, fk: &ForeignKey) {
    put_positions(out, fk.columns.iter());
    put_name(out, &fk.table);
    put_positions(out, fk.referenced.iter());
    out.put_u8(tag_of(&MATCHES, &fk.match_type));
    out.put_u8(tag_of(&ACTIONS, &fk.on_update));
    out.put_u8(tag_of(&ACTIONS, &fk.on_delete));
    put_optional(out, fk.period.as_ref().map(|(own, _)| own.as_str()));
    put_optional(out, fk.period.as_ref().map(|(_, referenced)| referenced.as_str()));
}

/// The foreign key `put_foreign_key` wrote.
///
/// # Errors
///
/// SQLCODE -916 of a tag it knows not, or of one period without the other.
pub(super) fn foreign_key(r: &mut Reader<'_>) -> Result<ForeignKey> {
    let (columns, table, referenced) = (positions(r)?, qualified(r)?, positions(r)?);
    let (m, u, d) = (r.u8()?, r.u8()?, r.u8()?);
    let (match_type, on_update, on_delete) = (of_tag(r, &MATCHES, m)?, of_tag(r, &ACTIONS, u)?, of_tag(r, &ACTIONS, d)?);
    let period = match (optional(r)?, optional(r)?) {
        (Some(own), Some(theirs)) => Some((own, theirs)),
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => return Err(corrupt(r)),
    };
    Ok(ForeignKey { columns, table, referenced, match_type, on_update, on_delete, period })
}

/// A foreign key of one period alone, refused.
fn corrupt(r: &Reader<'_>) -> SqlError {
    r.corrupt("a foreign key of one period")
}
