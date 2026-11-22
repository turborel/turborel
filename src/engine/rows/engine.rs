//! The row store as a storage engine: attached to a database, its tables
//! made and opened, and its files checkpointed.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::meta::parse_locator;
use super::{COLUMN_STORE, RowStore, RowTable, TableFiles, corrupt, options};
use crate::catalog::TableDesc;
use crate::disk::buffer::Capture;
use crate::disk::io::Durability;
use crate::disk::shadow::ShadowFileSystem;
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
    /// the pages it may capture later owed to it (RULES.md D590), its
    /// commit left to `checkpoint_finish`; one not shadowed is synced now.
    fn checkpoint(&self, at: Lsn) -> Result<()> {
        if !self.checkpoints {
            return Ok(());
        }
        let files = self.files()?;
        // No table's rows half frozen (`rows/frozen.rs`).
        let _frozen = write(&files.freezes)?;
        if let Some(shadows) = &files.shadows {
            captured(&files, shadows)?;
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
            let capture = lock_builtin(&files.capture).take();
            capture.map_or(Ok(()), |capture| ended(&files, capture))?;
            shadows.commit_sealed()?;
            files.durable.fetch_max(files.sealed.load(Ordering::Acquire), Ordering::AcqRel);
        }
        Ok(())
    }
}

/// \[DESIGN\] A checkpoint's capture of the shadowed `files`, writers held
/// (RULES.md D590): what is written now kept in memory, so no I/O; its
/// pages copied, encoded and written only where they cannot be captured
/// later, the rest owed (`BufferPool::capture`); the batch sealed. A
/// capture an earlier checkpoint left, one that failed after its writers
/// went on, is ended and its batch committed first, so the one sealed now
/// is the only one. Given up if a step fails, what was held let go to the
/// side file.
fn captured(files: &TableFiles, shadows: &ShadowFileSystem) -> Result<()> {
    if let Some(why) = lock_builtin(&files.spoiled).clone() {
        return Err(why);
    }
    let earlier = lock_builtin(&files.capture).take();
    earlier.map_or(Ok(()), |capture| ended(files, capture))?;
    shadows.commit_sealed()?;
    shadows.hold()?;
    let capture = match files.save_zones().and_then(|()| files.save_counts()).and_then(|()| files.pool.capture(files.file)) {
        Ok(capture) => capture,
        Err(e) => {
            let _ = shadows.release();
            return Err(e);
        }
    };
    let synced = files.pool.sync(files.file, Durability::Full);
    let sealed = synced.and_then(|()| if capture.is_empty() { shadows.seal() } else { shadows.seal_capturing() });
    if let Err(e) = sealed {
        files.pool.abandon(capture);
        // Given up: what was held goes to the side file, newer than the
        // file; a failure here leaves it in memory, read as written.
        let _ = shadows.release();
        return Err(e);
    }
    *lock_builtin(&files.capture) = Some(capture);
    Ok(())
}

/// Ends `capture`, writers going on: every page it owes in the sealed
/// batch. A failure leaves the batch lacking a page: it is never
/// committed, and no checkpoint is taken until the database is opened
/// again, recovered from its last checkpoint and its log.
fn ended(files: &TableFiles, capture: Capture) -> Result<()> {
    files.pool.capture_rest(capture).map_err(|e| {
        let why = SqlError::new(
            Code::Io,
            format!(
                "[DESIGN] RULES.md D590: a checkpoint could not capture a page ({}): none is taken until the database is opened again",
                e.described()
            ),
        );
        lock_builtin(&files.spoiled).get_or_insert(why).clone()
    })
}
