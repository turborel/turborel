//! \[DESIGN\] Index entries shared by a row's versions (RULES.md D750).
//!
//! An engine keeping versions gives an index an entry for each version of
//! a row, which collection takes back with the version (`TableStorage::gc`).
//! An update that leaves every column an index's key reads as it was would
//! add an entry of the same key and row as the one the version it replaced
//! has, for collection to take back later. An engine that shares entries
//! instead notes, with the version an update replaces, the columns the
//! update left as they were, of those its writer asks about. Its writer adds
//! no entry for the new version to an index whose key reads only those
//! columns; collection then keeps that index's entry of the version
//! replaced, which serves the new version, and readers find the row by it
//! as before. A run of versions of one key so holds one entry, taken back
//! with the run's last version. An index build, and a checkpoint's record
//! of the entries awaiting collection, take the same entries as collection
//! does.

use crate::error::Result;
use crate::storage::{Row, RowId};
use crate::txn::{Timestamp, TxId};

/// What a storage whose versions share index entries does.
pub trait Sharing {
    /// As `TableStorage::replace` with the row of `write`, or, given its
    /// timestamp, as `write_newest` with it: the row replaced, and of `columns`, in their
    /// order, those the write left as they were, compared as the engine
    /// stores them, which it notes with the version replaced and gives
    /// with it to `gc` and `garbage`.
    ///
    /// # Errors
    ///
    /// As `replace` and `write_newest`.
    fn share(&self, tx: TxId, id: RowId, write: (Row, Option<Timestamp>), columns: &[usize]) -> Result<(Row, Vec<usize>)>;
}

/// Where collection gives the versions it removes (`TableStorage::gc`).
pub struct Removal<'a> {
    /// Whether a version whose change kept the columns given needs its row
    /// read: none does whose every entry the version after shares.
    pub needs: &'a dyn Fn(&[usize]) -> bool,
    /// Takes back the entries of a version that does, given its row id, its
    /// row and the columns kept: whether every one was.
    pub removed: &'a mut dyn FnMut(RowId, Row, &[usize]) -> bool,
}

/// A version awaiting collection: its row id, its row, and the columns the
/// change over it left as they were (`Sharing::share`).
pub type Superseded = (RowId, Row, Vec<usize>);

/// \[DESIGN\] Versions awaiting collection, read as they are pulled
/// (`TableStorage::garbage`).
pub type SupersededRows = Box<dyn Iterator<Item = Result<Superseded>> + Send>;

/// Whether an index whose key reads `reads` keeps the entry of a version
/// for the one that replaced it, which left the columns `kept` as they
/// were: so no entry is added for the new version, and none is taken back
/// with the old one. Every path that adds or takes back an entry of a
/// version asks it, so none can disagree with another.
#[must_use]
pub fn shares_entry(reads: &[usize], kept: &[usize]) -> bool {
    !reads.is_empty() && reads.iter().all(|c| kept.contains(c))
}

// Unit tests: an entry is shared only when every column the key reads was
// kept (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D750: an entry is shared when every
    /// column its key reads was kept, not when one was changed, nor when
    /// the key reads none or none was kept. MC/DC of `!reads.is_empty()`
    /// and `all`: (reads, all kept) shares; (no reads) does not; (reads,
    /// one not kept) does not.
    #[test]
    fn an_entry_is_shared_only_when_every_column_its_key_reads_was_kept() {
        assert!(shares_entry(&[1, 3], &[0, 1, 3]));
        assert!(!shares_entry(&[], &[0, 1]));
        assert!(!shares_entry(&[1, 2], &[0, 1, 3]));
        assert!(!shares_entry(&[1], &[]));
    }
}
