//! The row store's own methods: made over files, or in memory, and given
//! the database's budget and work areas. Split from engine/rows.rs
//! (RULES.md R-SIZE-2).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use super::options::encodings_of;
use super::{RowStore, Sealing, TableFiles, TableOptions};
use crate::catalog::{QualifiedName, TableDesc};
use crate::disk::encoding::Encodings;
use crate::engine::{MEMORY_POOL_BYTES, memory_file};
use crate::error::Result;
use crate::log::MemoryLog;
use crate::memory::MemoryBudget;
use crate::spill::WorkAreas;
use crate::sync::lock_builtin;

impl RowStore {
    /// A row store keeping its tables in `files`, each with `defaults`
    /// unless `configure` says otherwise.
    pub fn new(files: TableFiles, defaults: TableOptions) -> RowStore {
        let work = Arc::clone(&files.work);
        RowStore {
            files: Mutex::new(Some(files)),
            budget: Mutex::new(MemoryBudget::new(0)),
            defaults,
            tables: Mutex::new(HashMap::new()),
            work,
            encodings: Mutex::default(),
        }
    }

    /// \[DESIGN\] A row store keeping its tables in memory, in a file system
    /// of its own (`MemoryFileSystem`), older versions in a `MemoryLog`:
    /// the built-in engine of a database kept nowhere else. Its files are
    /// made when its first table is, so making it cannot fail; its buffer
    /// pool (`MEMORY_POOL_BYTES`) reserved then, of the budget of the
    /// database it is attached to, or of its own.
    pub fn in_memory(defaults: TableOptions) -> RowStore {
        let budget = Mutex::new(MemoryBudget::new(MEMORY_POOL_BYTES));
        RowStore {
            files: Mutex::new(None),
            budget,
            defaults,
            tables: Mutex::new(HashMap::new()),
            work: Arc::default(),
            encodings: Mutex::default(),
        }
    }

    /// Makes `encodings` those its tables' option ENCODINGS names.
    pub(crate) fn use_encodings(&self, encodings: Arc<Encodings>) {
        *lock_builtin(&self.encodings) = encodings;
    }

    /// How the rows of the table `desc` describes are sealed.
    pub(super) fn sealing(&self, desc: &TableDesc) -> Sealing {
        Sealing::named(Arc::clone(&lock_builtin(&self.encodings)), &encodings_of(desc))
    }

    /// Makes `budget` the one its pool in memory draws on, if made after.
    pub(crate) fn use_budget(&self, budget: Arc<MemoryBudget>) {
        *lock_builtin(&self.budget) = budget;
    }

    /// Makes `work` the work areas its transactions' changes spill to.
    pub(crate) fn use_work(&self, work: Arc<WorkAreas>) {
        *self.work.write().unwrap_or_else(PoisonError::into_inner) = Some(work);
    }

    /// Its files, made in memory if it has none yet.
    pub(super) fn files(&self) -> Result<TableFiles> {
        let mut files = lock_builtin(&self.files);
        if let Some(files) = files.as_ref() {
            return Ok(files.clone());
        }
        let (pool, file) = memory_file(&Arc::clone(&lock_builtin(&self.budget)), "the pool in memory of the row store")?;
        let made = TableFiles { work: Arc::clone(&self.work), ..TableFiles::new(pool, file, Arc::new(MemoryLog::new())) };
        *files = Some(made.clone());
        Ok(made)
    }

    /// Sets the options of `table`, for when it is created.
    pub fn configure(&self, table: QualifiedName, options: TableOptions) {
        lock_builtin(&self.tables).insert(table, options);
    }
}
