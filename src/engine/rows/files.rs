//! \[DESIGN\] Where a row store keeps its tables (RULES.md D31): a pager file
//! of a buffer pool, shadowed or not (`disk/shadow.rs`), and the undo store
//! its tables share.

use crate::engine::columns::cache::HeaderCache;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, RwLock, Weak};

use super::UndoStore;
use super::counted::Counted;
use super::mvcc::Statuses;
use super::stats::Kept;
use super::zones::Zones;
use crate::disk::buffer::{BufferPool, Capture, FileId};
use crate::disk::shadow::ShadowFileSystem;
use crate::error::SqlError;
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
    /// The kernels its tables decode and judge their vectors on, once its
    /// store is told them (RULES.md D570); until then, the portable ones.
    pub(super) kernels: Kernels,
    /// The statuses of the transactions writing its tables (RULES.md D440).
    pub(super) statuses: Arc<Statuses>,
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
    /// Where the statistics of its tables open are saved, at each
    /// checkpoint (`rows/stats/saved.rs`).
    pub(super) statistics: Arc<Mutex<Vec<Weak<Kept>>>>,
    /// The tables opened of its files that counted their rows by a pass.
    pub(super) passes: Arc<AtomicU64>,
    /// The bytes of its file the system's cache is taken to hold beyond
    /// the pool, as its store is told (STORAGE.SYSTEM_CACHE_BYTES); until
    /// then, all of it.
    pub(super) system_cache: Arc<AtomicU64>,
    /// The capture of the checkpoint whose batch is sealed, while pages
    /// it owes are not all in it (RULES.md D590).
    pub(super) capture: Arc<Mutex<Option<Capture>>>,
    /// Why a capture failed, if one did: its batch lacks a page, so no
    /// checkpoint is taken until the database is opened again.
    pub(super) spoiled: Arc<Mutex<Option<SqlError>>>,
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
            kernels: Arc::default(),
            statuses: Arc::default(),
            freezes: Arc::default(),
            headers: Arc::default(),
            zones: Arc::default(),
            counts: Arc::default(),
            statistics: Arc::default(),
            passes: Arc::default(),
            system_cache: Arc::new(AtomicU64::new(u64::MAX)),
            capture: Arc::default(),
            spoiled: Arc::default(),
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

    /// The kernels its tables run on now: those its store was told, else
    /// the portable ones.
    pub(super) fn kernels(&self) -> Arc<dyn crate::kernels::Kernels> {
        let told = self.kernels.read().map_or_else(|e| e.into_inner().clone(), |held| held.clone());
        told.unwrap_or_else(|| Arc::new(crate::kernels::Portable))
    }
}

/// The kernels a store's tables run on, once it is told them, shared by
/// the store and its files.
pub(super) type Kernels = Arc<RwLock<Option<Arc<dyn crate::kernels::Kernels>>>>;
