//! Replaying what recovery keeps beside rows (RULES.md D25, D79): the
//! index entries and large objects a checkpoint saw awaiting collection,
//! taken back or freed as recovery from that checkpoint ends, and the
//! objects of large objects, kept as the log has them.

use super::Definitions;
use crate::catalog::QualifiedName;
use crate::db::Database;
use crate::error::Result;
use crate::index::Key;
use crate::log::Lsn;
use crate::log::record::Change;
use crate::storage::RowId;
use crate::txn::Persistence;

impl Database {
    /// Replays `change`, logged at `lsn`: a stale index entry or large
    /// object, only of the checkpoint reopened from, as `definitions` says,
    /// since one before it, or one not recorded, saw garbage its tables
    /// need not have; or an object of a large object.
    pub(super) fn replay_kept(&self, lsn: Lsn, change: Change, definitions: &Definitions) -> Result<()> {
        let from_checkpoint = |through: Lsn| matches!(definitions, Definitions::From(at) if *at == through);
        match change {
            Change::Stale { through, index, row, key } if from_checkpoint(through) => self.take_back_stale(&index, row, &Key(key)),
            Change::LobStale { through, table, ids } if from_checkpoint(through) => {
                let st = self.snapshot()?;
                if let Ok(storage) = st.storage_of(&table) {
                    storage.note_stale_lobs(&ids);
                }
                Ok(())
            }
            Change::LobPut { table, generation, id, body } => self.replay_lob(lsn, (&table, generation), id, &body),
            Change::Stale { .. }
            | Change::LobStale { .. }
            | Change::Insert { .. }
            | Change::Update { .. }
            | Change::Delete { .. }
            | Change::Commit { .. }
            | Change::Abort
            | Change::Checkpoint
            | Change::StatementUndone { .. }
            | Change::Schema { .. }
            | Change::Defined { .. }
            | Change::Sequence { .. }
            | Change::CreateIndex { .. }
            | Change::AlterIndex { .. }
            | Change::DropIndex { .. }
            | Change::Drop { .. }
            | Change::AlterTable { .. }
            | Change::Setting { .. }
            | Change::User { .. }
            | Change::Owner { .. }
            | Change::RoleCreator { .. }
            | Change::Spatial(_)
            | Change::Reconfiguration { .. } => Ok(()),
        }
    }

    /// Takes back one entry of `row` under `key` in index `name`, if it is
    /// there: an entry of a version a checkpoint saw awaiting collection,
    /// which the crash lost. Before `mend_indexes`, which then sets right
    /// the rows recovery touched; an index to be built anew is built
    /// whatever it held.
    fn take_back_stale(&self, name: &str, row: RowId, key: &Key) -> Result<()> {
        let st = self.snapshot()?;
        let Ok(index) = st.index_of(name) else { return Ok(()) };
        if index.rows(key)?.contains(&row) {
            index.remove(key, row)?;
        }
        Ok(())
    }

    /// Keeps object `id` of `body` of a large object of `table`, logged at
    /// `lsn` while its storage was of `generation`, if its engine is
    /// recovered from the log and its pages may lack it, whether its
    /// transaction commits or not: the sweep at recovery's end frees what
    /// no row names (`TableStorage::recovered`).
    fn replay_lob(&self, lsn: Lsn, (table, generation): (&QualifiedName, u32), id: u64, body: &[u8]) -> Result<()> {
        let st = self.snapshot()?;
        if st.catalog.table(table).is_none_or(|t| t.generation != generation) {
            return Ok(());
        }
        let storage = st.storage_of(table)?;
        let lacks = match storage.capabilities().persistence {
            Persistence::Logged => lsn >= storage.durable_through(),
            Persistence::Forced => true,
            Persistence::External | Persistence::None => false,
        };
        if lacks { storage.apply_lob(id, body) } else { Ok(()) }
    }
}
