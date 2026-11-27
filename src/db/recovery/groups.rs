//! \[EXTENSION\] Recovery's part in loads (RULES.md D533, D1143, D1146):
//! the groups of loads open as a table was stored kept or dropped, and a
//! load's group record taken as a row change of its own, its first row
//! its row, redone whole if it stands (`Loading::redo_group`). Its rows'
//! index entries are mended as a row's are, a group at a time (RULES.md
//! D1235): those of its rows, decoded from its extent, taken back, then
//! the entries of each row it holds now made, each index taking those it
//! lacks at once (`Index::insert_sorted`).

use super::changes::RowChange;
use super::{Analysis, key_of, take_back};
use crate::catalog::IndexDesc;
use crate::db::{Database, State};
use crate::error::Result;
use crate::log::Lsn;
use crate::log::record::GroupLoaded;
use crate::storage::RowId;
use crate::txn::{NO_TRANSACTION, TxId};

/// The change of the group `loaded`, logged at `lsn` by `tx`.
pub(super) fn change((lsn, tx): (Lsn, TxId), loaded: GroupLoaded) -> RowChange {
    let GroupLoaded { table, generation, first, last, extent } = loaded;
    RowChange { lsn, tx, table, generation, row: first, before: None, after: None, group: Some((last, extent)) }
}

impl State {
    /// Takes back every entry of each row of the group of `change`, whose
    /// record holds `extent`, under its key there, in each index `kept`
    /// chooses: as a row's change takes back those of its images.
    pub(super) fn take_back_group(&self, change: &RowChange, extent: &[u8], kept: &impl Fn(&IndexDesc) -> bool) -> Result<()> {
        if self.catalog.table(&change.table).is_none() {
            return Ok(());
        }
        let indexes: Vec<_> = self.table_indexes(&change.table).into_iter().filter(|(ix, _)| kept(ix)).collect();
        let storage = self.storage_of(&change.table)?;
        let Some(loading) = storage.loading().filter(|_| !indexes.is_empty()) else { return Ok(()) };
        let rows = loading.extent_rows(extent)?;
        for (ix, partition) in &indexes {
            let index = self.index_of(&ix.name)?;
            for (id, row) in &rows {
                if let Some(key) = key_of(row, ix) {
                    take_back(&**index, &key, Self::entry_id(*partition, *id)?)?;
                }
            }
        }
        Ok(())
    }

    /// Makes the entry of each row of the group of `change`, to `last`,
    /// under its key now, in each index `kept` chooses, unless there: the
    /// entries an index lacks added at once.
    pub(super) fn restore_group(&self, change: &RowChange, last: RowId, kept: &impl Fn(&IndexDesc) -> bool) -> Result<()> {
        let storage = self.storage_of(&change.table)?;
        let mut now = Vec::new();
        for id in change.row..=last {
            if let Some(row) = storage.get(NO_TRANSACTION, id)? {
                now.push((id, row));
            }
        }
        for (ix, partition) in self.table_indexes(&change.table).into_iter().filter(|(ix, _)| kept(ix)) {
            let index = self.index_of(&ix.name)?;
            let mut lacked = Vec::new();
            for (id, row) in &now {
                let id = Self::entry_id(partition, *id)?;
                if let Some(key) = key_of(row, &ix).filter(|k| index.rows(k).is_ok_and(|ids| !ids.contains(&id))) {
                    lacked.push((key, id));
                }
            }
            index.insert_sorted(&lacked, false)?;
        }
        Ok(())
    }
}

impl Database {
    /// Keeps or drops each table's groups of loads open as it was stored
    /// (`Loading::recovered_loads`), by `analysis`.
    pub(super) fn settle_loads(&self, analysis: &Analysis) -> Result<()> {
        for storage in self.snapshot()?.storage.values() {
            let durable = storage.durable_through();
            storage.loading().map_or(Ok(()), |l| l.recovered_loads(&|tx| analysis.redoes(tx, durable)))?;
        }
        Ok(())
    }

    /// \[DESIGN\] RULES.md D1143: reopened after a clean close, to serve, keeps each table's groups of loads stored
    /// as made by a transaction open as committed. A clean close is taken with no transaction open, and a load
    /// rolled back takes its groups out of the directory, so each group stored is of a load that committed.
    pub(in crate::db) fn settle_closed_loads(&self, mode: super::super::files::Mode) -> Result<()> {
        if mode != super::super::files::Mode::Serve {
            return Ok(());
        }
        for storage in self.snapshot()?.storage.values() {
            storage.loading().map_or(Ok(()), |l| l.recovered_loads(&|_| false))?;
        }
        Ok(())
    }
}
