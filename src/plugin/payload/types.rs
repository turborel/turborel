//! \[DESIGN\] The payloads of a type's calls (RULES.md D223): values made
//! of their text forms (INPUT) or binary forms (RECEIVE), each answered
//! by its three forms, which the database keeps and works with alone.
//!
//! ```text
//! INPUT    count u32 | str...     -> count u32 | made...
//! RECEIVE  count u32 | bytes...   -> count u32 | made...
//! made:    key bytes | binary bytes | text str
//! ```
//!
//! `key` orders and groups values, compared as unsigned bytes: values of
//! equal keys are one value. `binary` is the value's binary form, which
//! CAST to a binary string gives and RECEIVE reads; `text` its canonical
//! text form, which CAST to a character string gives and INPUT reads.

use crate::bytes::{Put, Reader};
use crate::error::Result;

use super::count;

/// A value of a plug-in's type: its three forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    /// The bytes it is ordered and grouped by.
    pub key: Vec<u8>,
    /// Its binary form.
    pub binary: Vec<u8>,
    /// Its canonical text form.
    pub text: String,
}

/// `made`, written: a value's bytes as the database keeps them, and as an
/// answer carries each.
pub fn put_made(out: &mut Vec<u8>, made: &Made) {
    out.put_bytes(&made.key);
    out.put_bytes(&made.binary);
    out.put_str(&made.text);
}

/// What `put_made` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn made(r: &mut Reader<'_>) -> Result<Made> {
    Ok(Made { key: r.bytes()?.to_vec(), binary: r.bytes()?.to_vec(), text: r.str()? })
}

/// INPUT's payload.
pub fn put_texts(out: &mut Vec<u8>, texts: &[&str]) {
    out.put_u32(count(texts.len()));
    for t in texts {
        out.put_str(t);
    }
}

/// RECEIVE's payload.
pub fn put_binaries(out: &mut Vec<u8>, binaries: &[&[u8]]) {
    out.put_u32(count(binaries.len()));
    for b in binaries {
        out.put_bytes(b);
    }
}

/// What `put_texts` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn texts(r: &mut Reader<'_>) -> Result<Vec<String>> {
    (0..r.u32()?).map(|_| r.str()).collect()
}

/// What `put_binaries` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn binaries(r: &mut Reader<'_>) -> Result<Vec<Vec<u8>>> {
    (0..r.u32()?).map(|_| r.bytes().map(<[u8]>::to_vec)).collect()
}

/// An answer: a value made of each asked.
pub fn put_answered(out: &mut Vec<u8>, all: &[Made]) {
    out.put_u32(count(all.len()));
    for m in all {
        put_made(out, m);
    }
}

/// What `put_answered` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn answered(r: &mut Reader<'_>) -> Result<Vec<Made>> {
    (0..r.u32()?).map(|_| made(r)).collect()
}
