//! \[DESIGN\] Making a large object a piece at a time (RULES.md D79): what
//! makes one from others, UPPER, TRANSLATE or a regular expression's
//! replacement, gives its octets in order, and the builder keeps a chunk's
//! worth in memory, writing each full chunk to a temporary large object of
//! the database's (`TempLobs`); octets of other large objects it is given
//! whole it takes as parts, copying none.

use std::fmt;
use std::sync::Arc;

use super::{CHUNK_OCTETS, INLINE_OCTETS, Lob, LobKind, LobSource, Part, Slice, count_chars, len};
use crate::error::{Code, Result, SqlError};

/// \[DESIGN\] Where a database keeps its temporary large objects (RULES.md
/// D79): chunks in a scratch file, each object's freed when the last value
/// holding it is dropped.
pub trait TempLobs: Send + Sync + fmt::Debug {
    /// A writer of a new temporary large object.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the scratch file cannot be made, -904 if memory for
    /// it cannot be reserved.
    fn writer(&self) -> Result<Box<dyn TempWriter>>;
}

/// A temporary large object being written, a chunk at a time.
pub trait TempWriter: Send {
    /// Adds `chunk`, of `chars` characters, at most `CHUNK_OCTETS` octets.
    ///
    /// # Errors
    ///
    /// As writing to the scratch file fails.
    fn push(&mut self, chunk: &[u8], chars: u32) -> Result<()>;
    /// The temporary large object of the chunks added.
    ///
    /// # Errors
    ///
    /// As writing to the scratch file fails.
    fn finish(self: Box<Self>) -> Result<Arc<dyn LobSource>>;
}

/// A large object being made.
pub struct LobBuilder<'s> {
    kind: LobKind,
    temp: Option<&'s dyn TempLobs>,
    writer: Option<Box<dyn TempWriter>>,
    pending: Vec<u8>,
    parts: Vec<Part>,
}

impl fmt::Debug for LobBuilder<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LobBuilder")
            .field("kind", &self.kind)
            .field("pending", &self.pending.len())
            .field("parts", &self.parts.len())
            .finish()
    }
}

impl<'s> LobBuilder<'s> {
    /// A builder of a large object of `kind`, keeping what is longer than a
    /// chunk in `temp`, if given.
    pub fn new(kind: LobKind, temp: Option<&'s dyn TempLobs>) -> LobBuilder<'s> {
        LobBuilder { kind, temp, writer: None, pending: Vec::new(), parts: Vec::new() }
    }

    /// Adds `text`, of a character large object.
    ///
    /// # Errors
    ///
    /// As writing a chunk fails; SQLCODE -910 if there is no temporary
    /// store to keep what is past a chunk in.
    pub fn push_str(&mut self, text: &str) -> Result<()> {
        self.push_bytes(text.as_bytes())
    }

    /// Adds `bytes`: octets of a binary large object, or UTF-8 of a
    /// character one, ending at a character's boundary.
    ///
    /// # Errors
    ///
    /// As `push_str`.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.pending.extend_from_slice(bytes);
        while self.pending.len() >= CHUNK_OCTETS {
            let cut = if self.kind == LobKind::Character { crate::value::lob::char_cut(&self.pending, CHUNK_OCTETS) } else { CHUNK_OCTETS };
            let chunk: Vec<u8> = self.pending.drain(..cut.max(1)).collect();
            self.write_chunk(&chunk)?;
        }
        Ok(())
    }

    /// Writes `chunk` to the temporary large object being written.
    fn write_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        if self.writer.is_none() {
            let temp = self.temp.ok_or_else(|| {
                SqlError::new(Code::Unsupported, "[DESIGN] a large object longer than a chunk is made where no temporary store keeps it")
            })?;
            self.writer = Some(temp.writer()?);
        }
        let chars = if self.kind == LobKind::Character { count_chars(chunk) } else { len(chunk) };
        let chars = u32::try_from(chars).map_err(|_| super::broken("a chunk of more characters than a chunk holds octets"))?;
        match self.writer.as_mut() {
            Some(w) => w.push(chunk, chars),
            None => Err(super::broken("a temporary large object's writer went missing")),
        }
    }

    /// Ends the temporary large object being written, and what is pending,
    /// as parts.
    fn close(&mut self) -> Result<()> {
        if self.writer.is_some() && !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.write_chunk(&rest)?;
        }
        if let Some(w) = self.writer.take() {
            let source = w.finish()?;
            let (octets, chars) = source.totals();
            self.parts.push(Part::Slice(Slice { source, octet: 0, char: 0, octets, chars }));
        }
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            let chars = if self.kind == LobKind::Character { count_chars(&rest) } else { len(&rest) };
            self.parts.push(Part::Inline(Arc::from(rest), chars));
        }
        Ok(())
    }

    /// Adds the whole of `lob`, of its kind, by its parts, copying no octets
    /// of a long one.
    ///
    /// # Errors
    ///
    /// As `push_str`.
    pub fn push_lob(&mut self, lob: &Lob) -> Result<()> {
        if let Some(bytes) = lob.inline()
            && self.pending.len().saturating_add(bytes.len()) <= INLINE_OCTETS
        {
            return self.push_bytes(bytes);
        }
        self.close()?;
        self.parts.extend(lob.parts().iter().cloned());
        Ok(())
    }

    /// The large object made.
    ///
    /// # Errors
    ///
    /// As writing the last chunk fails.
    pub fn finish(mut self) -> Result<Lob> {
        self.close()?;
        let parts = std::mem::take(&mut self.parts);
        Ok(Lob::of_parts(self.kind, parts))
    }
}
