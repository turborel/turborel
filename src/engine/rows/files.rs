//! \[DESIGN\] Where a row store keeps its tables (RULES.md D31): a pager file
//! of a buffer pool, shadowed or not (`disk/shadow.rs`), and the undo store
//! its tables share.

use crate::engine::columns::cache::HeaderCache;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, RwLock, Weak};

use super::UndoStore;
use super::counted::Counted;
use super::zones::Zones;
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::shadow::ShadowFileSystem;
use crate::log::Log;
use crate::spill::WorkAreas;

/// Where a row store's tables are kept: a pager file of a buffer pool, and
/// the undo store, which they share; and the log position their pages
/// hold every committed change through, as of the last checkpoint.
#[derive(Clone)]
pub struct TableFiles {
    pub(super) pool: Arc<BufferPool>,
    pub(super) file: FileId,
    pub(super) undo: Arc<UndoStore>,
    pub(super) durable: Arc<AtomicU64>,
    /// The position of the checkpoint whose batch is sealed, not yet
    /// committed.
    pub(super) sealed: Arc<AtomicU64>,
    /// The shadowing file system its file is in, if it is shadowed.
    pub(super) shadows: Option<Arc<ShadowFileSystem>>,
    /// The database's work areas, once the store is attached: where its
    /// transactions' changes spill (RULES.md D38).
    pub(super) work: Arc<RwLock<Option<Arc<WorkAreas>>>>,
    /// Held shared while a table freezes rows into a column group, and
    /// exclusively while a checkpoint is captured, so none captures half
    /// a group (`rows/frozen.rs`).
    pub(super) freezes: Arc<RwLock<()>>,
    /// The headers of its tables' frozen row groups held in memory
    /// (`columns/cache.rs`): its store's.
    pub(super) headers: Arc<HeaderCache>,
    /// The zone maps of its tables open, saved at each checkpoint
    /// (`rows/zones/saved.rs`).
    pub(super) zones: Arc<Mutex<Vec<Weak<Zones>>>>,
    /// The counts of rows of its tables open, saved at each checkpoint
    /// (`rows/counted.rs`).
    pub(super) counts: Arc<Mutex<Vec<Weak<Counted>>>>,
    /// The tables opened of its files that counted their rows by a pass.
    pub(super) passes: Arc<AtomicU64>,
}

impl TableFiles {
    /// Tables in pager file `file` of `pool`, their older versions in
    /// `undo`, a log holding nothing still needed.
    pub fn new(pool: Arc<BufferPool>, file: FileId, undo: Arc<dyn Log>) -> TableFiles {
        TableFiles::over(pool, file, UndoStore::new(undo))
    }

    /// Tables in pager file `file` of `pool`, their older versions in
    /// `undo`: one that batches them, for a log on disk.
    pub fn over(pool: Arc<BufferPool>, file: FileId, undo: UndoStore) -> TableFiles {
        TableFiles {
            pool,
            file,
            undo: Arc::new(undo),
            durable: Arc::new(AtomicU64::new(0)),
            sealed: Arc::new(AtomicU64::new(0)),
            shadows: None,
            work: Arc::default(),
            freezes: Arc::default(),
            headers: Arc::default(),
            zones: Arc::default(),
            counts: Arc::default(),
            passes: Arc::default(),
        }
    }

    /// The same, its file opened through `shadows` (`disk/shadow.rs`),
    /// which a checkpoint commits.
    #[must_use]
    pub fn shadowed(self, shadows: Arc<ShadowFileSystem>) -> TableFiles {
        TableFiles { shadows: Some(shadows), ..self }
    }

    /// The undo store.
    pub fn undo(&self) -> &UndoStore {
        &self.undo
    }
}
