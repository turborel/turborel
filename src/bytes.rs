//! \[DESIGN\] Reading and writing the fields of stored formats (RULES.md
//! D30), and of a server's messages (D42): every integer little-endian,
//! every string its length then its UTF-8 bytes, so bytes written on one
//! machine read the same on any other. A read past the end or a malformed
//! field is corruption (SQLCODE -916), or in a message received a
//! connection failure (-919, SQLSTATE 08006), never a panic.

use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::types::{SqlType, TypeRegistry};

/// Appends fields to a buffer.
pub trait Put {
    /// A byte.
    fn put_u8(&mut self, v: u8);
    /// Four bytes.
    fn put_u32(&mut self, v: u32);
    /// Eight bytes.
    fn put_u64(&mut self, v: u64);
    /// Sixteen bytes.
    fn put_i128(&mut self, v: i128);
    /// The length, as a `u32`, then the bytes.
    fn put_bytes(&mut self, v: &[u8]);
    /// As `put_bytes`, of the string's UTF-8.
    fn put_str(&mut self, v: &str);
}

impl Put for Vec<u8> {
    fn put_u8(&mut self, v: u8) {
        self.push(v);
    }
    fn put_u32(&mut self, v: u32) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn put_u64(&mut self, v: u64) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn put_i128(&mut self, v: i128) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn put_bytes(&mut self, v: &[u8]) {
        self.put_u32(u32::try_from(v.len()).unwrap_or(u32::MAX));
        self.extend_from_slice(v);
    }
    fn put_str(&mut self, v: &str) {
        self.put_bytes(v.as_bytes());
    }
}

/// Reads fields from bytes, front to back.
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    /// What is being read, for messages.
    what: &'static str,
    /// Whether they were received from a connection, not read from storage.
    received: bool,
    /// The kinds of type beyond the standard a value or type read may be
    /// of, by name, if any (`value::types`).
    types: Option<&'a TypeRegistry>,
}

impl<'a> Reader<'a> {
    /// A reader of `bytes`, which hold a `what`.
    pub fn new(bytes: &'a [u8], what: &'static str) -> Reader<'a> {
        Reader { bytes, at: 0, what, received: false, types: None }
    }

    /// A reader of `bytes` received from a connection, which hold a `what`.
    pub fn received(bytes: &'a [u8], what: &'static str) -> Reader<'a> {
        Reader { bytes, at: 0, what, received: true, types: None }
    }

    /// This reader, a value or type of a kind beyond the standard it reads
    /// found among `types`.
    #[must_use]
    pub fn with_types(self, types: &'a TypeRegistry) -> Reader<'a> {
        Reader { types: Some(types), ..self }
    }

    /// The kinds of type beyond the standard it finds values and types
    /// among, if any.
    pub fn types(&self) -> Option<&'a TypeRegistry> {
        self.types
    }

    /// The kind of type beyond the standard named `name`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if no such kind is registered where the bytes are read:
    /// its plug-in is registered before the database is opened, as a
    /// storage engine is (RULES.md D53).
    pub fn kind(&self, name: &str) -> Result<&'static dyn SqlType> {
        self.types.and_then(|t| t.get(name)).ok_or_else(|| {
            SqlError::new(
                Code::Api,
                format!("[DESIGN] {} names the type {name}, which no plug-in registered: register it before opening", self.what),
            )
        })
    }

    /// The error for a malformed `what`, at the current offset.
    pub fn corrupt(&self, why: &str) -> SqlError {
        if self.received {
            let msg = format!("[DESIGN] a {} received is malformed at byte {}: {why}", self.what, self.at);
            return SqlError::new(Code::Connection, msg).with_state(SqlState::CONNECTION_FAILURE);
        }
        SqlError::new(Code::Corrupt, format!("[DESIGN] a stored {} is malformed at byte {}: {why}", self.what, self.at))
    }

    /// The next `n` bytes.
    ///
    /// # Errors
    ///
    /// `corrupt`'s, if fewer are left.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|&end| end <= self.bytes.len()).ok_or_else(|| self.corrupt("it ends too soon"))?;
        let taken = self.bytes.get(self.at..end).ok_or_else(|| self.corrupt("it ends too soon"))?;
        self.at = end;
        Ok(taken)
    }

    /// The bytes left, all taken.
    pub fn rest(&mut self) -> &'a [u8] {
        let rest = self.bytes.get(self.at..).unwrap_or_default();
        self.at = self.bytes.len();
        rest
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let taken = self.take(N)?;
        <[u8; N]>::try_from(taken).map_err(|_| self.corrupt("it ends too soon"))
    }

    /// A byte.
    ///
    /// # Errors
    ///
    /// `corrupt`'s, if none is left; so for each read.
    pub fn u8(&mut self) -> Result<u8> {
        Ok(u8::from_le_bytes(self.array()?))
    }
    /// Four bytes.
    ///
    /// # Errors
    ///
    /// As `u8`.
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    /// Eight bytes.
    ///
    /// # Errors
    ///
    /// As `u8`.
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    /// Sixteen bytes.
    ///
    /// # Errors
    ///
    /// As `u8`.
    pub fn i128(&mut self) -> Result<i128> {
        Ok(i128::from_le_bytes(self.array()?))
    }

    /// Bytes written by `put_bytes`.
    ///
    /// # Errors
    ///
    /// As `u8`.
    pub fn bytes(&mut self) -> Result<&'a [u8]> {
        let len = usize::try_from(self.u32()?).map_err(|_| self.corrupt("a length is too long"))?;
        self.take(len)
    }

    /// A string written by `put_str`.
    ///
    /// # Errors
    ///
    /// As `u8`, or if it is not UTF-8.
    pub fn str(&mut self) -> Result<String> {
        let bytes = self.bytes()?;
        String::from_utf8(bytes.to_vec()).map_err(|_| self.corrupt("a string is not UTF-8"))
    }

    /// Checks that every byte was read.
    ///
    /// # Errors
    ///
    /// `corrupt`'s, if one was not.
    pub fn finish(&self) -> Result<()> {
        if self.at == self.bytes.len() {
            return Ok(());
        }
        Err(self.corrupt("bytes follow its end"))
    }
}
