//! \[DESIGN\] A delimited record split straight into `Cells` (RULES.md
//! D1148): each field's text appended to its column as it is found, a
//! quoted one's doubled quotes taken as one, a field written as the
//! dialect's null text, not in quotes, null; so the fields are those
//! `split` gives, but no string is made of one. The record is checked to
//! be UTF-8 once, whole: a delimiter and a quote are ASCII, so each field
//! cut from it at one is UTF-8 too. A record that is wrong in any way is
//! split again by `split`, so its failure is told in the same words.

use super::{Dialect, QUOTE, split};
use crate::import::Cells;

/// Appends the fields of the record `bytes`, written as `dialect` says,
/// to `cells`.
///
/// # Errors
///
/// As `split`'s, in its words.
pub(super) fn split_into(bytes: &[u8], dialect: &Dialect, cells: &mut Cells) -> Result<(), String> {
    let whole = if std::str::from_utf8(bytes).is_err() {
        false
    } else if dialect.quoted {
        quoted(bytes, dialect, cells)
    } else {
        plain(bytes, dialect, cells);
        true
    };
    if whole {
        return Ok(());
    }
    Err(split(bytes, dialect).err().unwrap_or_else(|| "[DESIGN] the record cannot be split".to_owned()))
}

/// Appends the fields of `bytes`, never quoted, to `cells`.
fn plain(bytes: &[u8], dialect: &Dialect, cells: &mut Cells) {
    for field in bytes.split(|b| *b == dialect.delimiter) {
        if field == dialect.null.as_bytes() {
            cells.null();
        } else {
            cells.piece(field);
            cells.end();
        }
    }
}

/// Appends the fields of `bytes`, each perhaps enclosed in quotes
/// [RFC 4180 §2 rules 5-7], to `cells`: false if one is written wrong.
fn quoted(bytes: &[u8], dialect: &Dialect, cells: &mut Cells) -> bool {
    let mut rest = bytes;
    loop {
        let after =
            if let Some((&QUOTE, inside)) = rest.split_first() { enclosed(inside, cells) } else { unenclosed(rest, dialect, cells) };
        let Some(after) = after else { return false };
        match after.split_first() {
            None => return true,
            Some((&b, next)) => {
                if b != dialect.delimiter {
                    return false;
                }
                rest = next;
            }
        }
    }
}

/// Appends the field not enclosed in quotes `rest` begins with, to the
/// next delimiter, to `cells`: what follows it, or none if it holds a
/// quote.
fn unenclosed<'a>(rest: &'a [u8], dialect: &Dialect, cells: &mut Cells) -> Option<&'a [u8]> {
    let end = rest.iter().position(|b| *b == dialect.delimiter).unwrap_or(rest.len());
    let (field, after) = rest.split_at_checked(end)?;
    if field.contains(&QUOTE) {
        return None;
    }
    if field == dialect.null.as_bytes() {
        cells.null();
    } else {
        cells.piece(field);
        cells.end();
    }
    Some(after)
}

/// Appends the field enclosed in quotes whose text `inside` begins, its
/// opening quote passed, to `cells`, a quote written twice one [RFC 4180
/// §2 rule 7]: what follows its closing quote, or none if it has none.
fn enclosed<'a>(mut inside: &'a [u8], cells: &mut Cells) -> Option<&'a [u8]> {
    loop {
        let at = inside.iter().position(|b| *b == QUOTE)?;
        let (text, from) = inside.split_at_checked(at)?;
        cells.piece(text);
        let after = from.get(1..)?;
        let Some((&QUOTE, more)) = after.split_first() else {
            cells.end();
            return Some(after);
        };
        cells.piece(&[QUOTE]);
        inside = more;
    }
}
