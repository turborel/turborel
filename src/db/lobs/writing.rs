//! A large object a client makes a piece at a time through its locator
//! (RULES.md D79): its octets kept a chunk at a time among the database's
//! temporary large objects as they come, so a value much larger than
//! memory is made in little more than a chunk; and the values a client
//! sends by their locators, found.

use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::Value;
use crate::value::lob::{CHUNK_OCTETS, Lob, LobKind, MAX_LOB_LENGTH, Part, TempLobs, TempWriter};

/// A large object being made.
pub(super) struct Appender {
    kind: LobKind,
    writer: Option<Box<dyn TempWriter>>,
    /// What came after the last chunk written.
    pending: Vec<u8>,
    /// How many octets at the start of `pending` are known to be UTF-8, of
    /// a character one. What follows them is at most a character begun in
    /// one Append, to be ended in the next.
    valid: usize,
    /// Its octets so far.
    octets: u64,
    /// The error an Append was refused with, if one was, so the value is
    /// never made of what was taken before it (RULES.md D392).
    refused: Option<SqlError>,
}

/// [ISO/IEC 9075-2:2023 §4.2.1] SQLSTATE 22021: a character large
/// object's octets are not UTF-8, the form of its character set.
fn not_utf8() -> SqlError {
    SqlError::new(Code::Data, "[ISO/IEC 9075-2:2023 §4.2.1] a character large object's octets are not UTF-8")
        .with_state(SqlState::CHARACTER_NOT_IN_REPERTOIRE)
}

/// The characters of `bytes`, UTF-8 of a character large object.
///
/// # Errors
///
/// SQLSTATE 22021 if they are not UTF-8.
fn characters(bytes: &[u8]) -> Result<usize> {
    std::str::from_utf8(bytes).map(|s| s.chars().count()).map_err(|_| not_utf8())
}

/// How many octets of `bytes` from `from` are UTF-8, a character cut at
/// their end, whose octets may come in the next Append, passed over.
///
/// # Errors
///
/// SQLSTATE 22021 if an octet there is not UTF-8 as it stands.
fn checked_to(bytes: &[u8], from: usize) -> Result<usize> {
    let rest = bytes.get(from..).unwrap_or_default();
    match std::str::from_utf8(rest) {
        Ok(_) => Ok(bytes.len()),
        Err(e) => match e.error_len() {
            None => Ok(from.saturating_add(e.valid_up_to())),
            Some(_) => Err(not_utf8()),
        },
    }
}

impl Appender {
    /// An empty large object of `kind`.
    pub(super) fn new(kind: LobKind) -> Appender {
        Appender { kind, writer: None, pending: Vec::new(), valid: 0, octets: 0, refused: None }
    }

    /// Adds `bytes`, writing each full chunk to `temp`. Of a character
    /// large object they are checked as they come, with what is left of a
    /// character the last Append began, so an octet that is not UTF-8 is
    /// refused by the Append that sends it (RULES.md D392). Once one is
    /// refused, every later Append is, and so is the value, with its error.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22001 past the longest large object; 22021 if a character
    /// one is not UTF-8; an Append's before, if one was refused; writing a
    /// chunk's.
    pub(super) fn append(&mut self, bytes: &[u8], temp: &dyn TempLobs) -> Result<()> {
        if let Some(e) = &self.refused {
            return Err(e.clone());
        }
        let taken = self.taken(bytes, temp);
        if let Err(e) = &taken {
            self.refused = Some(e.clone());
            (self.pending, self.writer) = (Vec::new(), None);
        }
        taken
    }

    /// `append`'s work, before a refusal is noted.
    fn taken(&mut self, bytes: &[u8], temp: &dyn TempLobs) -> Result<()> {
        let octets = self.octets.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        if octets > MAX_LOB_LENGTH {
            return Err(SqlError::new(
                Code::Data,
                format!("[ISO/IEC 9075-2:2023 §6.1 SR10] a large object is longer than {MAX_LOB_LENGTH} octets"),
            )
            .with_state(SqlState::STRING_DATA_RIGHT_TRUNCATION));
        }
        self.octets = octets;
        self.pending.extend_from_slice(bytes);
        if self.kind == LobKind::Character {
            self.valid = checked_to(&self.pending, self.valid)?;
        }
        while self.pending.len() >= CHUNK_OCTETS {
            let cut = if self.kind == LobKind::Character {
                crate::value::lob::char_cut(&self.pending, CHUNK_OCTETS).max(1)
            } else {
                CHUNK_OCTETS
            };
            let chunk: Vec<u8> = self.pending.drain(..cut).collect();
            self.valid = self.valid.saturating_sub(cut);
            let chars = if self.kind == LobKind::Character { characters(&chunk)? } else { chunk.len() };
            let chars = u32::try_from(chars).map_err(|_| SqlError::new(Code::Api, "[DESIGN] a chunk of more than a chunk's characters"))?;
            let writer = match self.writer.as_mut() {
                Some(w) => w,
                None => self.writer.insert(temp.writer()?),
            };
            writer.push(&chunk, chars)?;
        }
        Ok(())
    }

    /// The large object made, the chunks written then what came after.
    ///
    /// # Errors
    ///
    /// An Append's, if one was refused; SQLSTATE 22021 if a character
    /// large object ends within a character; ending the chunks' writing's.
    pub(super) fn finish(&mut self) -> Result<Lob> {
        if let Some(e) = &self.refused {
            return Err(e.clone());
        }
        let rest = std::mem::take(&mut self.pending);
        let tail = match self.kind {
            LobKind::Character => {
                characters(&rest)?;
                Lob::of_text(std::str::from_utf8(&rest).unwrap_or_default())
            }
            LobKind::Binary => Lob::of_octets(&rest),
        };
        match self.writer.take() {
            Some(writer) => Ok(Lob::of_source(self.kind, writer.finish()?).concat(&tail)),
            None => Ok(tail),
        }
    }
}

/// `v`, each large object in it held by a locator the value `located`
/// finds of it, in arrays and rows too.
///
/// # Errors
///
/// `located`'s.
pub(super) fn resolved(v: &Value, located: &mut dyn FnMut(u32) -> Result<Lob>) -> Result<Value> {
    match v {
        Value::Lob(l) => match l.parts() {
            [Part::Locator(locator)] => located(*locator).map(Value::Lob),
            [] | [_, ..] => Ok(v.clone()),
        },
        Value::Array(items) => Ok(Value::Array(items.iter().map(|i| resolved(i, located)).collect::<Result<Vec<_>>>()?.into())),
        Value::Multiset(items) => Ok(Value::Multiset(items.iter().map(|i| resolved(i, located)).collect::<Result<Vec<_>>>()?.into())),
        Value::Row(fields) => Ok(Value::Row(fields.iter().map(|f| resolved(f, located)).collect::<Result<Vec<_>>>()?.into())),
        Value::Structured(s) => {
            let attributes = s.attributes.iter().map(|a| resolved(a, located)).collect::<Result<Vec<_>>>()?;
            Ok(Value::Structured(std::sync::Arc::new(crate::value::types::Structured { attributes: attributes.into(), ..(**s).clone() })))
        }
        Value::Null
        | Value::Reference(_)
        | Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Json(_)
        | Value::Extension(_)
        | Value::Xml(_) => Ok(v.clone()),
    }
}
