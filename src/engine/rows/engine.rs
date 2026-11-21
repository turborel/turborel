//! The row store as a storage engine: attached to a database, its tables
//! made and opened, and its files checkpointed.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::meta::parse_locator;
use super::{COLUMN_STORE, RowStore, RowTable, corrupt, options};
use crate::catalog::TableDesc;
use crate::disk::io::Durability;
use crate::error::{Code, Result, SqlError};
use crate::log::Lsn;
use crate::memory::Host;
use crate::settings::Configurable;
use crate::storage::{StorageEngine, TableStorage};
use crate::sync::{lock_builtin, write};
use crate::value::DataType;
use crate::value::types::composite;

impl StorageEngine for RowStore {
    fn name(&self) -> &'static str {
        self.name
    }
    fn seals(&self) -> bool {
        true
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.use_budget(Arc::clone(&host.memory));
        self.use_work(Arc::clone(&host.work));
        self.use_encodings(Arc::clone(&host.encodings));
        Ok(())
    }
    fn use_kernels(&self, kernels: Arc<dyn crate::kernels::Kernels>) {
        *self.kernels.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(kernels);
    }
    fn table_options(&self) -> Vec<crate::storage::TableOption> {
        options::declared(self.defaults)
    }
    /// It reads its settings (`Configurable`).
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(self)
    }
    fn create_table(&self, desc: &TableDesc) -> Result<Box<dyn TableStorage>> {
        let options = options::resolved(desc, lock_builtin(&self.tables).get(&desc.name).copied().unwrap_or(self.defaults));
        let types: Vec<DataType> = desc.columns.iter().map(|c| c.data_type.clone()).collect();
        // [DESIGN] RULES.md D79, D106: a frozen row group holds no large
        // object, and no XML value, which one holds, nor any inside an
        // array, a multiset or a row.
        let held = |t: &DataType| composite::lob_ordered(t) || composite::xml_ordered(t);
        if self.name == COLUMN_STORE && types.iter().any(held) {
            return Err(SqlError::new(
                Code::Unsupported,
                format!("[DESIGN] the column store keeps no large object or XML value, as {} has", desc.name),
            ));
        }
        Ok(Box::new(RowTable::create(&self.files()?, &types, options)?.sealed(self.sealing(desc))))
    }

    /// The table whose meta page the locator names; its pages hold every
    /// change through the position the locator records (`locator`).
    fn open_table(&self, desc: &TableDesc, locator: &[u8]) -> Result<Option<Box<dyn TableStorage>>> {
        let (meta, durable) = parse_locator(locator).ok_or_else(|| corrupt("a table's locator is not the row store's"))?;
        let options = options::resolved(desc, lock_builtin(&self.tables).get(&desc.name).copied().unwrap_or(self.defaults));
        let types: Vec<DataType> = desc.columns.iter().map(|c| c.data_type.clone()).collect();
        let files = self.files()?;
        files.durable.fetch_max(durable, Ordering::AcqRel);
        Ok(Some(Box::new(RowTable::open_with(&files, (meta, &types), options, self.sealing(desc))?)))
    }

    /// Saves its tables' zone maps and counts of rows
    /// (`rows/zones/saved.rs`, `rows/counted.rs`); writes back every page and the pager's free list; a file shadowed
    /// (`TableFiles::shadowed`) holds them in memory and seals that batch,
    /// its commit left to `checkpoint_finish`; one not shadowed is synced
    /// now.
    fn checkpoint(&self, at: Lsn) -> Result<()> {
        if !self.checkpoints {
            return Ok(());
        }
        let files = self.files()?;
        // No table's rows half frozen (`rows/frozen.rs`).
        let _frozen = write(&files.freezes)?;
        if let Some(shadows) = &files.shadows {
            // What is written now is kept in memory: no I/O with writers held.
            shadows.hold()?;
            let captured =
                files.save_zones().and_then(|()| files.save_counts()).and_then(|()| files.pool.sync(files.file, Durability::Full));
            if let Err(e) = captured.and_then(|()| shadows.seal()) {
                let _ = shadows.release();
                return Err(e);
            }
            files.sealed.store(at.0, Ordering::Release);
            return Ok(());
        }
        files.save_zones()?;
        files.save_counts()?;
        files.pool.sync(files.file, Durability::Full)?;
        files.durable.fetch_max(at.0, Ordering::AcqRel);
        Ok(())
    }

    /// Commits the batch sealed, so after a crash the file holds exactly
    /// what the checkpoint captured.
    fn checkpoint_finish(&self) -> Result<()> {
        if !self.checkpoints {
            return Ok(());
        }
        let files = self.files()?;
        if let Some(shadows) = &files.shadows {
            shadows.commit_sealed()?;
            files.durable.fetch_max(files.sealed.load(Ordering::Acquire), Ordering::AcqRel);
        }
        Ok(())
    }
}
