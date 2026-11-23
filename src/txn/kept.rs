//! \[DESIGN\] Index entries an update keeps (RULES.md D443). An index holds
//! an entry per run of consecutive versions of a row with its key, counted
//! once (`Index`). An update whose new version holds every column the key
//! reads as the version it replaced does adds no entry, and notes those
//! columns on the change (`TableStorage::keep_entries`); collection then
//! takes back no entry of the version replaced, nor a checkpoint logs one
//! for recovery to take back, since its successor holds it.
//!
//! An index filled from the versions of a table that has some (`CREATE
//! INDEX` beside writers, a rebuild under the table's lock) counts every
//! version it finds, so a version superseded by a commit the fill may have
//! counted shares nothing in it, whatever columns it kept: the fill's
//! latest such commit is noted on the table's lock
//! (`TableLock::filled`). A write after the fill keeps the index as any
//! other, its commit later.

use super::{TableLock, Timestamp};
use crate::catalog::IndexDesc;
use crate::storage::{Kept, column_bit};

/// \[DESIGN\] How collection treats the entries of one index of a table:
/// the columns its key reads, as bits (`storage::column_bit`), none if one
/// has no bit; and the latest commit a fill of it may have counted.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sharing {
    columns: u64,
    filled: Timestamp,
}

impl Sharing {
    /// Of an index whose entries are each version's own: a global index
    /// over partitions (RULES.md D157), or a temporary table's.
    pub(crate) const NONE: Sharing = Sharing { columns: 0, filled: Timestamp::MAX };

    /// Of the index `ix` of the table `lock` guards.
    pub(crate) fn of(ix: &IndexDesc, lock: &TableLock) -> Sharing {
        Sharing { columns: key_columns(ix), filled: lock.filled_through(&ix.name) }
    }

    /// Whether a version its successor `kept` shares its entry with that
    /// successor: the columns its key reads all kept, by a commit after any
    /// fill of it.
    pub(crate) fn shares(self, kept: Kept) -> bool {
        self.columns != 0 && self.columns & !kept.unchanged == 0 && kept.at > self.filled
    }
}

/// The columns `ix`'s key reads, as bits; none if one is past the last bit.
/// Of a key of columns as they are, they are its columns, read where the
/// index holds them: every write of a row asks, and a list made of them
/// for each would be an allocation a row and an index.
pub(crate) fn key_columns(ix: &IndexDesc) -> u64 {
    let bits = |columns: &[usize]| columns.iter().try_fold(0u64, |mask, &c| column_bit(c).map(|bit| mask | bit)).unwrap_or(0);
    if ix.has_expressions() { bits(&ix.reads()) } else { bits(&ix.columns) }
}

/// Whether collection reads the row of a version its successor `kept`, to
/// take back its entries: unless every one of `indexes` shares its entry,
/// and no global index over the partition holds one of its own (RULES.md
/// D157, D750).
pub(crate) fn needs_row(globals: bool, indexes: impl IntoIterator<Item = Sharing>, kept: Kept) -> bool {
    globals || indexes.into_iter().any(|sharing| !sharing.shares(kept))
}

/// The columns the keys of `indexes` read, in order, each once: those a
/// version collected is read for (RULES.md D751).
pub(crate) fn key_reads<'a>(indexes: impl Iterator<Item = &'a IndexDesc>) -> Vec<usize> {
    let mut reads: Vec<usize> = indexes.flat_map(IndexDesc::reads).collect();
    reads.sort_unstable();
    reads.dedup();
    reads
}

// Unit tests: which versions collection reads (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D750: a version is read only when an
    /// index does not share its entry, or a global index holds one of its
    /// own; one whose every entry is shared, or of a table without
    /// indexes, is not.
    #[test]
    fn a_version_is_read_only_for_entries_of_its_own() {
        let kept = Kept { at: 5, unchanged: 0b011 };
        let shared = Sharing { columns: 0b001, filled: 0 };
        let changed = Sharing { columns: 0b100, filled: 0 };
        let filled_after = Sharing { columns: 0b001, filled: 5 };
        assert!(!needs_row(false, [shared], kept));
        assert!(!needs_row(false, [], kept));
        assert!(needs_row(true, [shared], kept), "a global index's entry is its own");
        assert!(needs_row(false, [shared, changed], kept));
        assert!(needs_row(false, [filled_after], kept));
        assert!(needs_row(false, [Sharing::NONE], kept));
    }
}
