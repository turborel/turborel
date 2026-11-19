//! \[DESIGN\] The payload of a collation's one call, KEYS (RULES.md
//! D221): many strings in, a sort key each out, bytes the database
//! compares itself, so no comparison is a call.
//!
//! ```text
//! KEYS  pad u8 (0 PAD SPACE, 1 NO PAD) | count u32 | str...   -> count u32 | bytes...
//! ```

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::collation::Pad;

use super::{count, put_strings, strings};

/// The byte of `pad`.
fn pad_byte(pad: Pad) -> u8 {
    match pad {
        Pad::Space => 0,
        Pad::None => 1,
    }
}

/// KEYS's payload.
pub fn put_keys(out: &mut Vec<u8>, pad: Pad, texts: &[&str]) {
    out.put_u8(pad_byte(pad));
    put_strings(out, &texts.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>());
}

/// What `put_keys` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn keys(r: &mut Reader<'_>) -> Result<(Pad, Vec<String>)> {
    let pad = if r.u8()? == 0 { Pad::Space } else { Pad::None };
    Ok((pad, strings(r)?))
}

/// KEYS's answer: a key each.
pub fn put_answered(out: &mut Vec<u8>, keys: &[Vec<u8>]) {
    out.put_u32(count(keys.len()));
    for k in keys {
        out.put_bytes(k);
    }
}

/// What `put_answered` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn answered(r: &mut Reader<'_>) -> Result<Vec<Vec<u8>>> {
    (0..r.u32()?).map(|_| r.bytes().map(<[u8]>::to_vec)).collect()
}
