//! \[EXTENSION\] How a transaction reads [ISO/IEC 9075:1992 §4.28, §14.1
//! GR5] (RULES.md D29): at which snapshot, and whether its reads are
//! checked at commit, as its participants are told
//! (`TableStorage::read_mode`). Split from txn.rs (RULES.md R-SIZE-2).

use std::sync::atomic::Ordering;

use super::{Timestamp, Txn};
use crate::error::Result;

/// \[EXTENSION\] How a transaction reads [ISO/IEC 9075:1992 §4.28, §14.1
/// GR5] (RULES.md D29).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reading {
    /// SERIALIZABLE: at one snapshot, every read checked at commit.
    #[default]
    Serializable,
    /// REPEATABLE READ: at one snapshot, its reads not checked at commit.
    Snapshot,
    /// READ COMMITTED and READ UNCOMMITTED: as `Snapshot`, at a snapshot
    /// taken as each statement begins.
    Statement,
}

/// \[EXTENSION\] How a transaction reads from now, as an engine is told
/// (`TableStorage::read_mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadMode {
    /// The timestamp it reads at.
    pub snapshot: Timestamp,
    /// Whether what it reads is checked at commit.
    pub checked: bool,
}

impl Txn {
    /// The timestamp it reads at now.
    pub(crate) fn snapshot(&self) -> Timestamp {
        self.snapshot.load(Ordering::SeqCst)
    }

    /// How it reads now, as its participants are told.
    pub(super) fn read_mode(&self) -> ReadMode {
        ReadMode { snapshot: self.snapshot(), checked: self.reading == Reading::Serializable || self.checking.load(Ordering::SeqCst) }
    }

    /// Tells each participant how it reads now. Its bookkeeping is not
    /// locked across the engines' calls (RULES.md R-ERR-4).
    pub(super) fn tell_participants(&self) -> Result<()> {
        let joined = self.state()?.joined.clone();
        let mode = self.read_mode();
        joined.iter().try_for_each(|j| j.storage.read_mode(self.id, mode))
    }

    /// \[EXTENSION\] Below SERIALIZABLE, makes its reads checked at commit
    /// from now if `on`, else no longer: constraints are checked, and
    /// referential actions run, serializably at every level [ISO/IEC
    /// 9075:1992 §4.28].
    ///
    /// # Errors
    ///
    /// A participant's, told.
    pub(crate) fn check_reads(&self, on: bool) -> Result<()> {
        if self.reading == Reading::Serializable || self.checking.swap(on, Ordering::SeqCst) == on {
            return Ok(());
        }
        self.tell_participants()
    }
}
