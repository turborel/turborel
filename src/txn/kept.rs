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
pub(crate) fn key_columns(ix: &IndexDesc) -> u64 {
    ix.reads().into_iter().try_fold(0u64, |mask, c| column_bit(c).map(|bit| mask | bit)).unwrap_or(0)
}
