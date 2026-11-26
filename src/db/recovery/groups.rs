//! \[EXTENSION\] Recovery's part in loads (RULES.md D533, D1143, D1146):
//! the groups of loads open as a table was stored kept or dropped, and a
//! load's group record taken as a row change of its own, its first row
//! its row, redone whole if it stands (`Loading::redo_group`), its rows'
//! index entries made one row at a time as the others' are.

use super::Analysis;
use super::changes::RowChange;
use crate::db::Database;
use crate::error::Result;
use crate::log::Lsn;
use crate::log::record::GroupLoaded;
use crate::storage::RowId;
use crate::txn::TxId;

/// The change of the group `loaded`, logged at `lsn` by `tx`.
pub(super) fn change((lsn, tx): (Lsn, TxId), loaded: GroupLoaded) -> RowChange {
    let GroupLoaded { table, generation, first, last, extent } = loaded;
    RowChange { lsn, tx, table, generation, row: first, before: None, after: None, group: Some((last, extent)) }
}

/// Of `change`, a group's, its row `row`, as a change of its own.
pub(super) fn row_of(change: &RowChange, row: RowId) -> RowChange {
    let (lsn, tx, table, generation) = (change.lsn, change.tx, change.table.clone(), change.generation);
    RowChange { lsn, tx, table, generation, row, before: None, after: None, group: None }
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
