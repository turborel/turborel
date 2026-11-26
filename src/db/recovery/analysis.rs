//! The first pass of recovery, analysis (RULES.md D25): which
//! transactions committed and where, which rolled back, and which ranges
//! of their changes undone statements cancelled.

use std::collections::{HashMap, HashSet};

use super::changes::RowChange;
use crate::log::Lsn;
use crate::log::record::{Change, Record};
use crate::txn::TxId;

/// What analysis found.
#[derive(Default)]
pub(super) struct Analysis {
    /// Each transaction committed, and where its commit record is.
    pub(super) committed: HashMap<TxId, Lsn>,
    /// Each transaction rolled back, and where its abort record is.
    pub(super) rolled_back: HashMap<TxId, Lsn>,
    /// For each transaction, the ranges of positions its undone statements
    /// cancelled: from the statement's first change to its undo record.
    pub(super) cancelled: HashMap<TxId, Vec<(Lsn, Lsn)>>,
    pub(super) transactions: HashSet<TxId>,
    /// The highest transaction id or timestamp read.
    pub(super) highest: u64,
}

impl Analysis {
    /// Whether recovery redoes or undoes every change of transaction `tx`
    /// to a table whose pages hold every change logged before `durable`:
    /// it read its changes, and it did not commit, or committed at or
    /// after `durable` (RULES.md D533).
    pub(super) fn redoes(&self, tx: TxId, durable: Lsn) -> bool {
        self.transactions.contains(&tx) && self.committed.get(&tx).is_none_or(|&at| at >= durable)
    }

    /// Notes the record `record`, logged at `lsn`.
    pub(super) fn note(&mut self, lsn: Lsn, record: &Record) {
        self.highest = self.highest.max(record.tx);
        match &record.change {
            Change::Commit { at, .. } => {
                self.highest = self.highest.max(*at);
                self.committed.insert(record.tx, lsn);
                self.transactions.insert(record.tx);
            }
            Change::StatementUndone { from } => self.cancelled.entry(record.tx).or_default().push((*from, lsn)),
            // A transaction counts once it changed a row or ended; one that
            // only defined an index does neither.
            Change::Abort => {
                self.rolled_back.insert(record.tx, lsn);
                self.transactions.insert(record.tx);
            }
            Change::Insert { .. }
            | Change::InsertRows { .. }
            | Change::GroupLoaded(_)
            | Change::Update { .. }
            | Change::Delete { .. }
            | Change::LobPut { .. } => {
                self.transactions.insert(record.tx);
            }
            Change::Schema { .. }
            | Change::CreateIndex { .. }
            | Change::DropIndex { .. }
            | Change::AlterIndex { .. }
            | Change::Drop { .. }
            | Change::AlterTable { .. }
            | Change::Defined { .. }
            | Change::Spatial(_)
            | Change::Setting { .. }
            | Change::User { .. }
            | Change::Owner { .. }
            | Change::RoleCreator { .. }
            | Change::Reconfiguration { .. }
            | Change::Sequence { .. }
            | Change::Checkpoint
            | Change::RestorePoint { .. }
            | Change::Stale { .. }
            | Change::LobStale { .. } => {}
        }
    }

    /// Where the undone statement that cancelled `change` was undone, if
    /// one did.
    fn cancelled_at(&self, change: &RowChange) -> Option<Lsn> {
        let ranges = self.cancelled.get(&change.tx)?;
        ranges.iter().find(|&&(from, to)| from <= change.lsn && change.lsn < to).map(|&(_, to)| to)
    }

    /// Whether `change` stands: its transaction committed, and no undone
    /// statement cancelled it.
    pub(super) fn stands(&self, change: &RowChange) -> bool {
        self.committed.contains_key(&change.tx) && self.cancelled_at(change).is_none()
    }

    /// Where a change that does not stand was undone as the database ran:
    /// its statement's undo record, or its transaction's abort record;
    /// `None` if a crash interrupted it.
    pub(super) fn undone_at(&self, change: &RowChange) -> Option<Lsn> {
        self.cancelled_at(change).or_else(|| self.rolled_back.get(&change.tx).copied())
    }
}
