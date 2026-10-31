//! \[DESIGN\] What a database's state keeps of its directory as it is
//! opened (RULES.md D25, D56): its spill files' and plug-ins' places, and
//! its tables and indexes reopened by their locators. Split from
//! db/files.rs (RULES.md R-SIZE-2).

use std::path::Path;
use std::sync::Arc;

use super::Place;
use crate::catalog::{IndexDesc, TableDesc};
use crate::db::State;
use crate::disk::io::FileSystem;
use crate::error::Result;
use crate::files::plugins::PLUGINS;
use crate::index::{Index, IndexMethod};
use crate::spill::TempFiles;
use crate::storage::{StorageEngine, TableStorage};
use crate::value::DataType;

use super::TEMP;

impl State {
    /// Keeps its spill areas' files in `dir` of `fs` from now on, the spill
    /// files a run before left removed.
    pub(super) fn keep_in(&self, fs: &Arc<dyn FileSystem>, dir: &Path) -> Result<()> {
        self.work.set_temp(TempFiles::new(Arc::clone(fs), &dir.join(TEMP)))
    }

    /// Keeps its plug-ins' own files at `place` from now on, in its
    /// `plugins/`, their shadowed directories recovered (RULES.md D56):
    /// once its files' formats are known, before its tables are opened.
    pub(super) fn keep_plugins_in(&self, place: &Place<'_>) -> Result<()> {
        self.plugins.set_place(Arc::clone(place.fs), place.path(PLUGINS), Arc::clone(&place.layout.formats.hash))
    }

    /// The storage of the table `desc` in `engine`: reopened by its locator
    /// while the database is reopened and the engine can, else new.
    pub(in crate::db) fn table_storage(&mut self, engine: &dyn StorageEngine, desc: &TableDesc) -> Result<Arc<dyn TableStorage>> {
        if let Some(reopening) = self.reopening.as_mut() {
            let found = reopening.tables.get(&(desc.name.clone(), desc.generation));
            let reopened = found.map(|locator| engine.open_table(desc, locator)).transpose()?.flatten();
            if let Some(storage) = reopened {
                return Ok(Arc::from(storage));
            }
            reopening.made.push(desc.name.clone());
        }
        Ok(Arc::from(engine.create_table(desc)?))
    }

    /// Whether the database is being opened from its files, index `name`
    /// then left to be built once its tables are (`Database::reopen`).
    pub(in crate::db) fn leave_unbuilt(&mut self, name: &str) -> bool {
        let Some(reopening) = self.reopening.as_mut() else { return false };
        if !reopening.unbuilt.iter().any(|n| n == name) {
            reopening.unbuilt.push(name.to_owned());
        }
        true
    }

    /// The structure of index `index` of `method` on columns of `types`,
    /// of table `table` as of its generation: reopened by its locator
    /// while the database is reopened and the method can, else new; and
    /// whether it holds its entries, which a new one does not, unless it
    /// is built once the database is reopened.
    pub(in crate::db) fn index_structure(
        &mut self,
        method: &dyn IndexMethod,
        (index, table): (&IndexDesc, &TableDesc),
        types: &[DataType],
    ) -> Result<(Arc<dyn Index>, bool)> {
        if let Some(reopening) = self.reopening.as_mut() {
            let found = reopening.indexes.get(&(index.name.clone(), table.generation));
            let reopened = found.map(|locator| method.open_on(types, locator, (table, index))).transpose()?.flatten();
            if let Some(structure) = reopened {
                return Ok((Arc::from(structure), true));
            }
            reopening.unbuilt.push(index.name.clone());
            // Built once the database is reopened (`Database::reopen`).
            return Ok((Arc::from(method.create_on(types, (table, index))?), true));
        }
        // Its keys are in the clear, unless the files are encrypted.
        self.check_index_hidden(table)?;
        Ok((Arc::from(method.create_on(types, (table, index))?), false))
    }
}
