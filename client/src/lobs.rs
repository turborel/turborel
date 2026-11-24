//! \[DESIGN\] Large objects over a connection (RULES.md D45, D79): one a
//! result sends by its locator [ISO/IEC 9075-2:2023 §4.37.5] read by
//! ranges, or as a stream (`LobReader`), a chunk a request; a new one
//! written a piece at a time (`LobWriter`) and sent as a parameter by its
//! locator; and a locator freed [§14.17] or held [§14.18]. A large object
//! a result sends whole is read from the value itself. A locator is valid
//! until the transaction it was given in ends, a held one until a
//! rollback.

use std::io::{self, Read, Write};

use turborel::error::{Code, SqlError};
use turborel::value::lob::{CHUNK_OCTETS, Lob, LobKind, char_cut};
use turborel_protocol::{Completed, Failure, LobReply, LobRequest, Reply, Request};

use crate::connection::{Connection, Result};

/// \[DESIGN\] The longest value a client sends whole for a large object
/// parameter, as the server sends one whole by default
/// (SERVER.LOB_INLINE_BYTES); a longer one an import sends by a locator.
pub const LOB_INLINE_OCTETS: usize = 64 * 1024;
/// \[DESIGN\] The octets a `LobWriter` sends in one Append.
const APPEND_OCTETS: usize = CHUNK_OCTETS;

/// A failure of the client's own use of a value: `msg`.
fn misused(msg: &str) -> Failure {
    Failure::from(&SqlError::new(Code::Data, msg))
}

impl<S: Read + Write> Connection<S> {
    /// At most a chunk of `lob` from octet `from`, from 0, at most `count`
    /// of them; of a character large object, its characters if
    /// `characters`, whole ones. Fewer than asked only at its end, or at a
    /// chunk's.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0F001 if its locator is no longer valid; a connection
    /// failure.
    pub fn read_lob(&mut self, lob: &Lob, (from, count): (u64, u64), characters: bool) -> Result<Vec<u8>> {
        if let Some(bytes) = lob.inline() {
            return Ok(held_range(lob, bytes, (from, count), characters));
        }
        let Some(locator) = lob.locator() else { return Err(misused("[DESIGN] the large object was not sent by the server")) };
        match self.lob_request(&LobRequest::Read { locator, from, count, characters })? {
            LobReply::Data(bytes) => Ok(bytes),
            LobReply::Locator(_) => Err(self.lose("[DESIGN] the server sent a locator for a Read")),
        }
    }

    /// `lob`'s octets as a stream, read a chunk at a time.
    pub fn lob_reader<'c>(&'c mut self, lob: &Lob) -> LobReader<'c, S> {
        LobReader { connection: self, lob: lob.clone(), at: 0, chunk: Vec::new(), taken: 0 }
    }

    /// A new large object of `kind`, written a piece at a time.
    ///
    /// # Errors
    ///
    /// A connection failure; the server's.
    pub fn create_lob(&mut self, kind: LobKind) -> Result<LobWriter<'_, S>> {
        match self.lob_request(&LobRequest::Create(kind))? {
            LobReply::Locator(locator) => Ok(LobWriter { connection: self, kind, locator, pending: Vec::new(), octets: 0, chars: 0 }),
            LobReply::Data(_) => Err(self.lose("[DESIGN] the server sent data for a Create")),
        }
    }

    /// [§14.17] `lob`'s locator no longer valid, nor the value it named.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0F001 if it is not valid; a connection failure.
    pub fn free_lob(&mut self, lob: &Lob) -> Result<Completed> {
        let Some(locator) = lob.locator() else { return Err(misused("[DESIGN] the large object was sent whole: it has no locator")) };
        self.lob_completed(&LobRequest::Free(locator))
    }

    /// [§14.18] `lob`'s locator valid past its transaction's commit, until
    /// freed or a transaction is rolled back.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0F001 if it is not valid; a connection failure.
    pub fn hold_lob(&mut self, lob: &Lob) -> Result<Completed> {
        let Some(locator) = lob.locator() else { return Err(misused("[DESIGN] the large object was sent whole: it has no locator")) };
        self.lob_completed(&LobRequest::Hold(locator))
    }

    /// The server's Lob reply to `request`.
    fn lob_request(&mut self, request: &LobRequest) -> Result<LobReply> {
        self.send(&Request::Lob(request.clone()))?;
        match self.receive()? {
            Reply::Lob(reply) => Ok(reply),
            Reply::Error(failure) => Err(self.failed(failure)),
            Reply::Complete(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Description(_)
            | Reply::Rows(_)
            | Reply::Suspended
            | Reply::Notice(_)
            | Reply::Status { .. }
            | Reply::Pong
            | Reply::Debug(_)
            | Reply::Event(_) => Err(self.lose("[DESIGN] the server sent no Lob reply")),
        }
    }

    /// The server's completion of `request`.
    fn lob_completed(&mut self, request: &LobRequest) -> Result<Completed> {
        self.send(&Request::Lob(request.clone()))?;
        match self.receive()? {
            Reply::Complete(completed) => Ok(completed),
            Reply::Error(failure) => Err(self.failed(failure)),
            Reply::Lob(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Description(_)
            | Reply::Rows(_)
            | Reply::Suspended
            | Reply::Notice(_)
            | Reply::Status { .. }
            | Reply::Pong
            | Reply::Debug(_)
            | Reply::Event(_) => Err(self.lose("[DESIGN] the server sent no completion")),
        }
    }
}

/// The range `(from, count)` of `bytes`, `lob`'s octets held whole, in
/// characters if `characters` and it is a character one.
fn held_range(lob: &Lob, bytes: &[u8], (from, count): (u64, u64), characters: bool) -> Vec<u8> {
    let at = |n: u64| usize::try_from(n).unwrap_or(usize::MAX);
    if !characters || lob.kind() == LobKind::Binary {
        let start = at(from).min(bytes.len());
        return bytes.get(start..start.saturating_add(at(count)).min(bytes.len())).unwrap_or_default().to_vec();
    }
    String::from_utf8_lossy(bytes).chars().skip(at(from)).take(at(count)).collect::<String>().into_bytes()
}

/// A large object's octets as a stream, a chunk read at a time.
pub struct LobReader<'c, S: Read + Write> {
    connection: &'c mut Connection<S>,
    lob: Lob,
    /// The octet the next chunk begins at.
    at: u64,
    chunk: Vec<u8>,
    /// The octets of `chunk` already read.
    taken: usize,
}

impl<S: Read + Write> Read for LobReader<'_, S> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.taken >= self.chunk.len() {
            if self.at >= self.lob.octets() || out.is_empty() {
                return Ok(0);
            }
            let wanted = u64::try_from(CHUNK_OCTETS).unwrap_or(u64::MAX);
            self.chunk = self.connection.read_lob(&self.lob, (self.at, wanted), false).map_err(io::Error::other)?;
            if self.chunk.is_empty() {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the server sent less of the large object than its length"));
            }
            self.at = self.at.saturating_add(u64::try_from(self.chunk.len()).unwrap_or(u64::MAX));
            self.taken = 0;
        }
        let rest = self.chunk.get(self.taken..).unwrap_or_default();
        let n = rest.len().min(out.len());
        out.get_mut(..n).unwrap_or_default().copy_from_slice(rest.get(..n).unwrap_or_default());
        self.taken = self.taken.saturating_add(n);
        Ok(n)
    }
}

/// A new large object written a piece at a time, sent in Appends of a
/// chunk; `finish` gives the value to send as a parameter.
pub struct LobWriter<'c, S: Read + Write> {
    connection: &'c mut Connection<S>,
    kind: LobKind,
    locator: u32,
    pending: Vec<u8>,
    octets: u64,
    /// Its characters, each counted at its lead octet, so one a write
    /// splits is counted once.
    chars: u64,
}

impl<S: Read + Write> LobWriter<'_, S> {
    /// Sends the first APPEND_OCTETS of what is pending, a character string
    /// cut where a character ends, so no Append is longer however much one
    /// write gave.
    fn append_piece(&mut self) -> Result<()> {
        let cut = match self.kind {
            LobKind::Character => char_cut(&self.pending, APPEND_OCTETS),
            LobKind::Binary => APPEND_OCTETS,
        };
        let rest = self.pending.split_off(cut.max(1).min(self.pending.len()));
        let bytes = std::mem::replace(&mut self.pending, rest);
        self.connection.lob_completed(&LobRequest::Append { locator: self.locator, bytes }).map(drop)
    }

    /// Sends what is written and not yet sent.
    fn flush_pending(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::take(&mut self.pending);
        self.connection.lob_completed(&LobRequest::Append { locator: self.locator, bytes }).map(drop)
    }

    /// The large object written, to send as a parameter by its locator:
    /// valid until its transaction ends, and used once.
    ///
    /// # Errors
    ///
    /// A connection failure; the server's: SQLSTATE 22021 if a character
    /// one's octets are not UTF-8, checked across the pieces sent, or a
    /// piece before was refused; 22001 if it is longer than the longest.
    /// A character one that ends within a character is refused, 22021,
    /// where it is used, as the server cannot know a piece is the last.
    pub fn finish(mut self) -> Result<Lob> {
        self.flush_pending()?;
        Ok(Lob::of_locator(self.kind, self.locator, (self.octets, self.chars)))
    }
}

impl<S: Read + Write> Write for LobWriter<'_, S> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        self.octets = self.octets.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        if self.kind == LobKind::Character {
            let leads = bytes.iter().filter(|&&b| b & 0xC0 != 0x80).count();
            self.chars = self.chars.saturating_add(u64::try_from(leads).unwrap_or(u64::MAX));
        }
        while self.pending.len() >= APPEND_OCTETS {
            self.append_piece().map_err(io::Error::other)?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_pending().map_err(io::Error::other)
    }
}
