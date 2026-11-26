//! Replaying what recovery keeps beside rows (RULES.md D25, D79): the
//! index entries and large objects a checkpoint saw awaiting collection,
//! taken back or freed as recovery from that checkpoint ends, and the
//! objects of large objects, kept as the log has them.

use super::changes::RowChange;
use super::{Definitions, Replaying, take_back};
use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::Database;
use crate::error::Result;
use crate::index::Key;
use crate::log::Lsn;
use crate::log::record::Change;
use crate::storage::RowId;
use crate::txn::{NO_TRANSACTION, Persistence, TxId};

impl Database {
    /// Replays `kept`, logged at `lsn` by `tx`, now; but an object of a
    /// large object of a transaction whose definitions wait for its commit
    /// at that commit, after them, as its table may be one they make
    /// (RULES.md D33, D79).
    pub(super) fn kept_at(&self, (lsn, tx): (Lsn, TxId), kept: Change, replaying: &mut Replaying) -> Result<()> {
        if matches!(kept, Change::LobPut { .. }) && replaying.pending.contains_key(&tx) {
            replaying.lobs.entry(tx).or_default().push((lsn, kept));
            return Ok(());
        }
        self.replay_kept(lsn, kept, &replaying.definitions)
    }

    /// Replays `change`, logged at `lsn`: a stale index entry or large
    /// object, only of the checkpoint reopened from, as `definitions` says,
    /// since one before it, or one not recorded, saw garbage its tables
    /// need not have; or an object of a large object.
    pub(super) fn replay_kept(&self, lsn: Lsn, change: Change, definitions: &Definitions) -> Result<()> {
        let from_checkpoint = |through: Lsn| of_image(*definitions, through);
        match change {
            Change::Stale { through, index, row, key } => self.stale_entry(*definitions, through, (&index, row, &Key(key))),
            Change::LobStale { through, table, ids } if from_checkpoint(through) => {
                let st = self.snapshot()?;
                if let Ok(storage) = st.storage_of(&table) {
                    storage.note_stale_lobs(&ids);
                }
                Ok(())
            }
            Change::LobPut { table, generation, id, body } => self.replay_lob(lsn, (&table, generation), id, &body),
            Change::LobStale { .. }
            | Change::Insert { .. }
            | Change::InsertRows { .. }
            | Change::GroupLoaded(_)
            | Change::Update { .. }
            | Change::Delete { .. }
            | Change::Commit { .. }
            | Change::Abort
            | Change::Checkpoint
            | Change::RestorePoint { .. }
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

    /// The entry of `row` under `key` in index `name`, of a version the
    /// checkpoint holding every record before `through` saw awaiting
    /// collection: taken back by recovery from that checkpoint, mended by
    /// a database following from it, left by any other reading.
    fn stale_entry(&self, definitions: Definitions, through: Lsn, (name, row, key): (&str, RowId, &Key)) -> Result<()> {
        match definitions {
            Definitions::From(at) => {
                if at == through {
                    self.take_back_stale(name, row, key)
                } else {
                    Ok(())
                }
            }
            Definitions::Following(at) => {
                if at == through {
                    self.mend_stale(name, row, key)
                } else {
                    Ok(())
                }
            }
            Definitions::All => Ok(()),
        }
    }

    /// \[DESIGN\] RULES.md D57, D920: as `take_back_stale`, for a database
    /// following from the checkpoint, which meets the entry as it replays
    /// what came after, so a change of the row replayed before may have
    /// mended its entries under `key` already (`State::mend_indexes`). The
    /// row's entries under `key` in index `name` are mended as that does:
    /// every one taken back, then the row's now made again if its key is
    /// `key`.
    fn mend_stale(&self, name: &str, row: RowId, key: &Key) -> Result<()> {
        let st = self.snapshot()?;
        let (Some(ix), Ok(index)) = (st.catalog.index(name), st.index_of(name)) else { return Ok(()) };
        take_back(&**index, key, row)?;
        let change = RowChange {
            lsn: Lsn(0),
            tx: NO_TRANSACTION,
            table: ix.table.clone(),
            generation: 0,
            row,
            before: None,
            after: None,
            group: None,
        };
        st.restore_entry(&change, &|d: &IndexDesc| d.name == name)
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

/// Whether `definitions` reads from the checkpoint holding every record
/// before `through`.
fn of_image(definitions: Definitions, through: Lsn) -> bool {
    match definitions {
        Definitions::From(at) | Definitions::Following(at) => at == through,
        Definitions::All => false,
    }
}
