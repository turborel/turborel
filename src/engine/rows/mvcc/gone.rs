//! \[DESIGN\] (RULES.md D1321) An older row version the undo store could
//! not give back, told with what the store knew, so that one failure is
//! enough to read. The message names who went back and from where, then
//! lists the versions walked. Each version shows its maker as written and
//! as its transaction's status reads it, ending at the position not found.
//! It also gives the oldest position pinned, the position the log is kept
//! from, and the latest snapshot and horizon the store was told of. It is
//! built once a read has failed, never on a read that succeeds, and holds
//! positions, ids and timestamps, never a value.

use std::fmt::Write as _;

use super::super::undo::Held;
use super::super::version::{Maker, Version};
use super::Reader;
use crate::engine::rows::{RowTable, key};
use crate::error::{Code, SqlError};
use crate::storage::RowId;
use crate::txn::{Timestamp, TxId};

/// The most versions an error lists, the newest first, so that its message
/// stays a few hundred characters however long the row's chain.
const LISTED: usize = 8;

/// Who went back through a row's versions.
#[derive(Debug, Clone, Copy)]
pub(super) enum Walker {
    /// A reader.
    Reading(Reader),
    /// The validation of this transaction's change.
    Validating(TxId),
    /// The undo of this transaction's change.
    Undoing(TxId),
    /// The collection of garbage.
    Collecting,
    /// The fill of an index from the versions a reader at or after the
    /// first timestamp may read, committed through the second.
    Filling(Timestamp, Timestamp),
}

impl RowTable {
    /// `err`, met by `walker` going back from version `from`, told with
    /// what the store knew if it is a version not found (SQLCODE -916);
    /// any other error as it is.
    pub(super) fn lost(&self, err: SqlError, from: &[u8], walker: Walker) -> SqlError {
        if err.code != Code::Corrupt {
            return err;
        }
        let mut told = String::new();
        let _ = match walker {
            Walker::Reading(Reader::At { tx, snapshot }) => write!(told, "; read by transaction {tx} at {snapshot}"),
            Walker::Reading(Reader::Latest) => write!(told, "; read as the latest committed"),
            Walker::Validating(tx) => write!(told, "; validating transaction {tx}"),
            Walker::Undoing(tx) => write!(told, "; undoing transaction {tx}"),
            Walker::Collecting => write!(told, "; collecting"),
            Walker::Filling(horizon, through) => write!(told, "; filling an index from horizon {horizon} through {through}"),
        };
        self.walked(from, &mut told);
        let (pinned, kept) = self.undo.extent();
        let (snapshot, horizon) = self.versions.statuses.bounds();
        let pinned = pinned.map_or_else(|| "none".to_owned(), |at| at.0.to_string());
        let _ = write!(told, "; oldest pinned {pinned}, log kept from {}; latest snapshot {snapshot}, horizon {horizon}", kept.0);
        SqlError { msg: err.msg + &told, ..err }
    }

    /// As `lost`, of row `id`, going back from its newest version as the
    /// tree holds it now.
    pub(super) fn lost_row(&self, err: SqlError, id: RowId, walker: Walker) -> SqlError {
        let Some(newest) = self.tree.get(&key(id)).ok().flatten() else {
            let msg = format!("{}; row {id} has no newest version", err.msg);
            return self.lost(SqlError { msg, ..err }, &[], walker);
        };
        self.lost(err, &newest, walker)
    }

    /// Writes to `told` the versions found going back from `from`: each
    /// its position, maker as written and maker as read, to the first
    /// position not found, the chain's end, or `LISTED` versions.
    fn walked(&self, from: &[u8], told: &mut String) {
        if from.is_empty() {
            return told.push_str("; no version walked");
        }
        told.push_str("; walked");
        let (mut bytes, mut place) = (Held::from(from.to_vec()), "from".to_owned());
        for _ in 0..LISTED {
            let Ok(version) = Version::parse(&bytes) else {
                told.push_str(" a malformed version");
                return;
            };
            let _ = write!(told, " {place} {} read {},", shown(version.maker), shown(self.maker(&version)));
            let Some(older) = version.older else {
                told.push_str(" the chain's end");
                return;
            };
            let Ok(read) = self.undo.read(older) else {
                let _ = write!(told, " {} not found", older.0);
                return;
            };
            (bytes, place) = (read, older.0.to_string());
        }
        told.push_str(" more");
    }
}

/// A maker, as an error tells it.
fn shown(maker: Maker) -> String {
    match maker {
        Maker::Open(tx) => format!("open {tx}"),
        Maker::Committed(at) => format!("committed {at}"),
    }
}
