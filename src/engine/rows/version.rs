//! \[DESIGN\] A version of a row (RULES.md D29, D31): who made it, and where
//! the version before it is. The newest version of a row is its entry in
//! the table's tree; older committed versions are in the undo store, each
//! naming the one before it, so a reader goes back from the newest to the
//! one its snapshot sees.
//!
//! ```text
//! flags u8:  1 committed, 2 deleted (no record follows), 4 has an older
//!            version
//! stamp u64: the commit timestamp if committed, else the id of the
//!            transaction that made it
//! older u64: the undo store's position of the version before, if any
//! record:    the row (`record.rs`), unless deleted
//! ```
//!
//! Integers are little-endian. A version takes 9 bytes before its record,
//! 17 with an older one.

use crate::error::{Code, Result, SqlError};
use crate::log::Lsn;
use crate::txn::{Timestamp, TxId};

const COMMITTED: u8 = 1;
const DELETED: u8 = 2;
const OLDER: u8 = 4;
const KNOWN: u8 = COMMITTED | DELETED | OLDER;
const STAMP_AT: usize = 1;
const OLDER_AT: usize = 9;
const WORD: usize = 8;

fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a stored row version is malformed: {why}"))
}

/// Who made a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Maker {
    /// A transaction that committed, at this timestamp.
    Committed(Timestamp),
    /// This open transaction.
    Open(TxId),
}

/// A version, read in place.
#[derive(Debug, Clone, Copy)]
pub(super) struct Version<'a> {
    pub(super) maker: Maker,
    /// Whether it is a deletion: the row is absent from it on.
    pub(super) deleted: bool,
    /// Where the version before it is.
    pub(super) older: Option<Lsn>,
    /// The row, unless deleted.
    pub(super) record: &'a [u8],
}

fn word(bytes: &[u8], at: usize) -> Result<u64> {
    at.checked_add(WORD)
        .and_then(|end| bytes.get(at..end))
        .and_then(|b| <[u8; WORD]>::try_from(b).ok())
        .map(u64::from_le_bytes)
        .ok_or_else(|| corrupt("it is too short"))
}

impl<'a> Version<'a> {
    /// The version `bytes` hold.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not one.
    pub(super) fn parse(bytes: &'a [u8]) -> Result<Version<'a>> {
        let flags = bytes.first().copied().ok_or_else(|| corrupt("it is empty"))?;
        if flags & !KNOWN != 0 {
            return Err(corrupt("it has flags of another format"));
        }
        let stamp = word(bytes, STAMP_AT)?;
        let maker = if flags & COMMITTED == 0 { Maker::Open(stamp) } else { Maker::Committed(stamp) };
        let (older, record_at) =
            if flags & OLDER == 0 { (None, OLDER_AT) } else { (Some(Lsn(word(bytes, OLDER_AT)?)), OLDER_AT.saturating_add(WORD)) };
        let record = bytes.get(record_at..).unwrap_or_default();
        let deleted = flags & DELETED != 0;
        if deleted && !record.is_empty() {
            return Err(corrupt("a deletion holds a row"));
        }
        Ok(Version { maker, deleted, older, record })
    }

    /// Its bytes.
    pub(super) fn encode(&self) -> Vec<u8> {
        let (committed, stamp) = match self.maker {
            Maker::Committed(at) => (COMMITTED, at),
            Maker::Open(tx) => (0, tx),
        };
        let flags = committed | if self.deleted { DELETED } else { 0 } | if self.older.is_some() { OLDER } else { 0 };
        let mut out = Vec::with_capacity(OLDER_AT.saturating_add(WORD).saturating_add(self.record.len()));
        out.push(flags);
        out.extend_from_slice(&stamp.to_le_bytes());
        if let Some(older) = self.older {
            out.extend_from_slice(&older.0.to_le_bytes());
        }
        out.extend_from_slice(self.record);
        out
    }
}

// Unit tests: versions read back as written, and malformed ones are refused
// (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D29, D31, row versions: a committed
    /// version, an open one with an older version, and a deletion read back
    /// as written, in 9 bytes before the record, 17 with an older version.
    #[test]
    fn versions_read_back() {
        let committed = Version { maker: Maker::Committed(7), deleted: false, older: None, record: b"row" };
        let bytes = committed.encode();
        assert_eq!(bytes.len(), 9 + 3);
        let back = Version::parse(&bytes).unwrap();
        assert_eq!((back.maker, back.deleted, back.older, back.record), (Maker::Committed(7), false, None, &b"row"[..]));
        let open = Version { maker: Maker::Open(9), deleted: false, older: Some(Lsn(42)), record: b"new" };
        let open_bytes = open.encode();
        assert_eq!(open_bytes.len(), 17 + 3);
        let open_back = Version::parse(&open_bytes).unwrap();
        assert_eq!((open_back.maker, open_back.older, open_back.record), (Maker::Open(9), Some(Lsn(42)), &b"new"[..]));
        let deletion = Version { maker: Maker::Committed(3), deleted: true, older: Some(Lsn(1)), record: b"" };
        assert!(Version::parse(&deletion.encode()).unwrap().deleted);
    }

    /// Verifies: [DESIGN] RULES.md D29, D31, row versions: bytes that are not
    /// a version are refused with SQLCODE -916: empty, too short for their
    /// stamp or older version, of unknown flags, or a deletion with a row.
    #[test]
    fn malformed_versions_are_refused() {
        let code = |bytes: &[u8]| Version::parse(bytes).err().map(|e| e.code);
        assert_eq!(code(b""), Some(Code::Corrupt));
        assert_eq!(code(&[COMMITTED, 1, 2]), Some(Code::Corrupt));
        assert_eq!(code(&[OLDER, 0, 0, 0, 0, 0, 0, 0, 0, 1]), Some(Code::Corrupt));
        assert_eq!(code(&[0x80, 0, 0, 0, 0, 0, 0, 0, 0]), Some(Code::Corrupt));
        assert_eq!(code(&[DELETED, 0, 0, 0, 0, 0, 0, 0, 0, b'x']), Some(Code::Corrupt));
    }
}
