//! [ISO/IEC 9075-2:2023 §6.31, §6.33] The `<regex subject string>` of a
//! regular expression function and the units its positions count [§9.28
//! GR11b]: a character string's characters in memory, or a character
//! large object's read a chunk at a time (`regex/lob_text.rs`), never held
//! whole (RULES.md D79).

use std::borrow::Cow;

use super::{CHARACTERS, OCTETS};
use crate::error::{Code, Result, SqlError, fail};
use crate::functions::strings::text;
use crate::regex::{Ascii, LobText, Text};
use crate::value::Value;
use crate::value::lob::{CHUNK_OCTETS, Lob};

/// \[DESIGN\] The most octets of a pattern, flag or replacement given as
/// a character large object that are read, whole, into memory: a chunk's
/// (RULES.md D79). A pattern compiles to at most `MAX_INSTRUCTIONS`
/// instructions, so a longer one is of no use.
const SHORT_OCTETS: usize = CHUNK_OCTETS;

/// A subject's characters.
pub(super) enum Body<'v> {
    /// A character string's, in memory.
    Chars(Vec<char>),
    /// A character string's of ASCII alone, read where it lies (RULES.md
    /// D1188): its octets are its characters.
    Ascii(Ascii<'v>),
    /// A character large object's, read a chunk at a time.
    Lob(LobText<'v>),
}

/// Runs `$e` with `$t` the text of the subject `$s`, of whichever kind,
/// each kind's code its own so a string's characters are read as fast as
/// before.
macro_rules! on_text {
    ($s:expr, $t:ident => $e:expr) => {
        match &$s.body {
            $crate::functions::regex::subject::Body::Chars(chars) => {
                let $t = chars.as_slice();
                $e
            }
            $crate::functions::regex::subject::Body::Ascii(ascii) => {
                let $t = ascii;
                $e
            }
            $crate::functions::regex::subject::Body::Lob(lob) => {
                let $t = lob;
                $e
            }
        }
    };
}
pub(super) use on_text;

/// A subject string, and the units its positions and lengths count.
pub(super) struct Subject<'v> {
    pub(super) body: Body<'v>,
    octets: bool,
}

impl<'v> Subject<'v> {
    /// `value` counted in `units`, a `<char length units>`; none if it is
    /// null.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 of a large object that cannot be read as characters.
    pub(super) fn new(value: &'v Value, units: &Value) -> Result<Option<Subject<'v>>> {
        let octets = octets(units)?;
        let body = match value {
            Value::Null => return Ok(None),
            Value::Lob(lob) => Body::Lob(LobText::new(lob)?),
            Value::Char(s) => body_of(s),
            Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Json(_)
            | Value::Extension(_)
            | Value::Xml(_) => Body::Chars(text(value)?.unwrap_or_default().chars().collect()),
        };
        Ok(Some(Subject { body, octets }))
    }

    /// \[DESIGN\] (RULES.md D1188) `text`, a character string of a vector
    /// of them, counted in `units`: as `new` of its value, no value made.
    ///
    /// # Errors
    ///
    /// As `new`.
    pub(super) fn of_text(text: &'v str, units: &Value) -> Result<Subject<'v>> {
        Ok(Subject { body: body_of(text), octets: octets(units)? })
    }

    /// Its characters.
    pub(super) fn len(&self) -> usize {
        on_text!(self, t => t.len())
    }

    /// The units before character `at`.
    fn units_before(&self, at: usize) -> Result<u64> {
        let counted = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
        if !self.octets {
            return Ok(counted(at));
        }
        match &self.body {
            Body::Chars(chars) => Ok(counted(chars.iter().take(at).map(|c| c.len_utf8()).sum())),
            Body::Ascii(_) => Ok(counted(at)),
            Body::Lob(lob) => lob.octet_of_char(at),
        }
    }

    /// The position, from 1, of character `at`.
    ///
    /// # Errors
    ///
    /// As reading the chunk it is in fails.
    pub(super) fn position(&self, at: usize) -> Result<i128> {
        Ok(i128::from(self.units_before(at)?).saturating_add(1))
    }

    /// The character `sp`, a start position, names: none if it is less
    /// than 1 or past the string's length [§6.31 GR18b ii]. An octet
    /// within a character names the next character (UV074).
    ///
    /// # Errors
    ///
    /// As reading the chunk it is in fails.
    pub(super) fn start(&self, sp: i128) -> Result<Option<usize>> {
        let length = self.units_before(self.len())?;
        let Some(offset) = sp.checked_sub(1).and_then(|o| u64::try_from(o).ok()).filter(|o| *o < length) else { return Ok(None) };
        if !self.octets {
            return Ok(usize::try_from(offset).ok());
        }
        match &self.body {
            Body::Chars(chars) => {
                let mut before = 0_u64;
                // Within the last character names the string's end.
                let found = chars.iter().position(|c| {
                    let reached = before >= offset;
                    before = before.saturating_add(u64::try_from(c.len_utf8()).unwrap_or(u64::MAX));
                    reached
                });
                Ok(Some(found.unwrap_or(chars.len())))
            }
            Body::Ascii(_) => Ok(usize::try_from(offset).ok()),
            Body::Lob(lob) => lob.char_of_octet(offset).map(Some),
        }
    }

    /// Its characters `start` to `end`: a string, or a slice of the large
    /// object, which copies none of its octets.
    ///
    /// # Errors
    ///
    /// As reading the chunks its ends are in fails.
    pub(super) fn substring(&self, start: usize, end: usize) -> Result<Value> {
        match &self.body {
            Body::Chars(chars) => Ok(Value::Char(chars.get(start..end).unwrap_or_default().iter().collect())),
            Body::Ascii(ascii) => Ok(Value::Char(ascii.span(start, end).into())),
            Body::Lob(lob) => slice(lob.lob(), start, end).map(Value::Lob),
        }
    }
}

/// Whether `units`, a `<char length units>`, are octets.
fn octets(units: &Value) -> Result<bool> {
    match text(units)? {
        Some(OCTETS) => Ok(true),
        Some(CHARACTERS) => Ok(false),
        Some(_) | None => fail(Code::Api, "[DESIGN] a regular expression function was given units that are none"),
    }
}

/// The characters of the character string `s`: read where they lie if
/// all are ASCII's (RULES.md D1188).
fn body_of(s: &str) -> Body<'_> {
    Ascii::of(s).map_or_else(|| Body::Chars(s.chars().collect()), Body::Ascii)
}

/// Characters `start` to `end` of `lob`.
///
/// # Errors
///
/// As reading the chunks its ends are in fails.
pub(super) fn slice(lob: &Lob, start: usize, end: usize) -> Result<Lob> {
    let at = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
    lob.slice_chars(at(start), at(end.saturating_sub(start)))
}

/// The text of a pattern, flag or replacement `v`; none if it is null. A
/// character large object's is read whole, being short.
///
/// # Errors
///
/// SQLCODE -900 of a large object longer than `SHORT_OCTETS`; -910 if
/// it cannot be read.
pub(super) fn short(v: &Value) -> Result<Option<Cow<'_, str>>> {
    let Value::Lob(lob) = v else { return Ok(text(v)?.map(Cow::Borrowed)) };
    let most = u64::try_from(SHORT_OCTETS).unwrap_or(u64::MAX);
    let Some(octets) = lob.bounded(most)? else {
        return Err(SqlError::new(
            Code::Unsupported,
            format!(
                "[DESIGN] a regular expression's pattern, flag or replacement of more than {SHORT_OCTETS} octets is beyond turborel's limits"
            ),
        ));
    };
    let s = String::from_utf8(octets).map_err(|_| crate::value::lob::broken("a character large object's octets are not UTF-8"))?;
    Ok(Some(Cow::Owned(s)))
}
