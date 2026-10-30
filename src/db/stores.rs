//! \[EXTENSION\] Where a database kept in a directory keeps its log and
//! its catalog (RULES.md D25, D39): plug-ins, as its engines, index methods
//! and spill areas are, each given the place the database is kept
//! (`StorePlace`), which one keeping its part elsewhere, replicated or
//! remote, need not use.
//!
//! A `LogStorage` makes a database's log, a `Log` (`log.rs`), or opens the
//! one it made: the built-in, a segmented file log in the directory's
//! `log/` (`FileLogs`). A log opened holds every record synced before; one
//! a kept database may be recovered from is durable
//! (`LogCapabilities::durable`).
//!
//! A `CatalogStorage` makes a database's `CatalogStore`, or
//! opens the one it made; the store keeps records, each an image of the
//! database as of a checkpoint (`db/image.rs`), opaque to it, and gives back
//! the last whole one, which says what state the database's files are in.
//! The built-in keeps them in two files of the directory, by turns
//! (`db/catalog_file.rs`); another may keep them elsewhere, replicated, or
//! remote, using none of the place it is given.
//!
//! A catalog store's contract (RULES.md R-SAFE-3; checked for the built-in
//! by `test/integration/contracts.rs`):
//! * `record` returns once the record is durable: `last` gives it from then
//!   on, and after the store is opened again, until another is recorded.
//! * A record a crash cut short is never given: the one before stands.
//! * A store may forget every record but the last.
//! * `open` gives none for a place where no store was made, and `create`
//!   refuses a place where one was.
//! * Methods may be called from several threads at once (`Send + Sync`).
//! * No method may call back into the `Database` (RULES.md R-ERR-4).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use std::sync::{PoisonError, RwLock};

use super::Database;
use super::catalog_file::CatalogFiles;
use super::formats::FileFormats;
use crate::diagnostics::{Diagnostics, Event, Extension};
use crate::disk::io::FileSystem;
use crate::error::Result;
use crate::error::{Code, SqlError};
use crate::log::{FileLog, FileLogOptions, Log, Lsn};

/// \[EXTENSION\] Where a database kept in a directory is, for a plug-in
/// keeping a part of it: the file system and directory, and the formats
/// of its files (`db/formats.rs`).
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct StorePlace<'a> {
    /// The database's file system.
    pub fs: &'a Arc<dyn FileSystem>,
    /// Its directory.
    pub dir: &'a Path,
    /// What its files are checksummed with and encoded through.
    pub formats: &'a FileFormats,
    /// The position its log is known durable through, as its last image
    /// records it: a log need not check what it holds before it.
    pub through: Lsn,
    /// Where the database's events go, for a failure its store goes on
    /// past, if it is told one (RULES.md D40, D56).
    pub events: Option<&'a Arc<dyn Diagnostics>>,
}

impl<'a> StorePlace<'a> {
    /// The place in `dir` of `fs`, its files of `formats`, nothing of its
    /// log known.
    pub fn new(fs: &'a Arc<dyn FileSystem>, dir: &'a Path, formats: &'a FileFormats) -> StorePlace<'a> {
        StorePlace { fs, dir, formats, through: Lsn(0), events: None }
    }

    /// The same place, its stores telling `events` of the failures they go
    /// on past.
    #[must_use]
    pub fn telling(self, events: &'a Arc<dyn Diagnostics>) -> StorePlace<'a> {
        StorePlace { events: Some(events), ..self }
    }

    /// The same place, its log known durable through `through`.
    #[must_use]
    pub fn through(self, through: Lsn) -> StorePlace<'a> {
        StorePlace { through, ..self }
    }
}

/// \[EXTENSION\] A log a `LogStorage` opened, and what opening it did.
pub struct OpenedLog {
    /// The log.
    pub log: Arc<dyn Log>,
    /// Bytes cut from its end: what writes a crash interrupted left, of
    /// records never synced. The database reports them
    /// (`RecoveryReport::cut`).
    pub cut: u64,
}

/// \[EXTENSION\] A kind of log, given to a database before it is opened
/// from its files (`Database::set_log_storage`).
pub trait LogStorage: Send + Sync {
    /// Its name, for messages and events.
    fn name(&self) -> &'static str;
    /// The log at `place`, if one was made there, holding every record
    /// synced before, and what opening it cut.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot be read; -916 if what is there is not a
    /// log of this kind, or is damaged: a record synced may be lost, and
    /// nothing is cut.
    fn open(&self, place: &StorePlace<'_>) -> Result<Option<OpenedLog>>;
    /// Whether a log was made at `place`, nothing there changed: no tail
    /// cut nor segment removed, as opening it may.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if that cannot be found out.
    fn exists(&self, place: &StorePlace<'_>) -> Result<bool>;
    /// A new log at `place`, holding no record.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one is there already; -915 if it cannot be made.
    fn create(&self, place: &StorePlace<'_>) -> Result<Arc<dyn Log>>;
}

/// \[EXTENSION\] The built-in kind of log: a segmented file log in the
/// directory's `log/`, of `options` (`log/file.rs`), its segments of the
/// place's formats.
#[derive(Clone, Default)]
pub struct FileLogs {
    /// Its segments' size and durability.
    pub options: FileLogOptions,
}

/// The directory of a `FileLogs` log at `place`.
fn log_dir(place: &StorePlace<'_>) -> PathBuf {
    place.dir.join("log")
}

impl LogStorage for FileLogs {
    fn name(&self) -> &'static str {
        "files"
    }

    fn exists(&self, place: &StorePlace<'_>) -> Result<bool> {
        FileLog::exists(&**place.fs, &log_dir(place))
    }

    fn open(&self, place: &StorePlace<'_>) -> Result<Option<OpenedLog>> {
        let dir = log_dir(place);
        if !FileLog::exists(&**place.fs, &dir)? {
            return Ok(None);
        }
        let formats = (place.formats.hashes.as_slice(), place.formats.encodings.as_slice());
        let (log, recovery) = FileLog::open_through(Arc::clone(place.fs), &dir, formats, self.options.clone(), place.through)?;
        Ok(Some(OpenedLog { log: Arc::new(told(log, place)), cut: recovery.cut }))
    }

    fn create(&self, place: &StorePlace<'_>) -> Result<Arc<dyn Log>> {
        let options = FileLogOptions { pipeline: place.formats.pipeline.clone(), ..self.options.clone() };
        let log = FileLog::create(Arc::clone(place.fs), &log_dir(place), Arc::clone(&place.formats.hash), options)?;
        Ok(Arc::new(told(log, place)))
    }
}

/// `log`, telling the events of `place`, if it has any.
fn told(log: FileLog, place: &StorePlace<'_>) -> FileLog {
    match place.events {
        Some(events) => log.telling(Arc::clone(events)),
        None => log,
    }
}

/// \[EXTENSION\] A kind of catalog store, given to a database before it is
/// opened from its files (`Database::set_catalog_storage`).
pub trait CatalogStorage: Send + Sync {
    /// Its name, for messages and events.
    fn name(&self) -> &'static str;
    /// The store at `place`, if one was made there.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot be read; -916 if what is there is not a
    /// store of this kind.
    fn open(&self, place: &StorePlace<'_>) -> Result<Option<Box<dyn CatalogStore>>>;
    /// A new store at `place`, holding no record.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if one is there already, or it cannot be made.
    fn create(&self, place: &StorePlace<'_>) -> Result<Box<dyn CatalogStore>>;
}

/// \[EXTENSION\] A database's catalog store: see the module's
/// documentation.
pub trait CatalogStore: Send + Sync {
    /// Records `record`, durably. `compact_after`, the setting
    /// CATALOG.COMPACT_AFTER, is how many records the database would have
    /// it hold before it keeps only the last: a store that keeps only the
    /// last anyway may ignore it.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot be made durable; the record then stands
    /// or not, as a crash would have left it.
    fn record(&self, record: &[u8], compact_after: usize) -> Result<()>;
    /// The last record, if one was recorded.
    fn last(&self) -> Option<Vec<u8>>;
}

/// Where a database keeps its catalog and its log, once it is kept in a
/// directory.
pub(super) struct Storages {
    catalog: RwLock<Arc<dyn CatalogStorage>>,
    /// None for the built-in, of `OpenOptions::log`.
    log: RwLock<Option<Arc<dyn LogStorage>>>,
}

impl Default for Storages {
    fn default() -> Storages {
        Storages { catalog: RwLock::new(Arc::new(CatalogFiles)), log: RwLock::new(None) }
    }
}

impl Database {
    /// \[EXTENSION\] Keeps its catalog with `storage`, once it is opened from
    /// its files (`Database::open_files`), instead of in the built-in
    /// catalog files (RULES.md D25, D39). A database opened again must be
    /// given the storage that made its catalog.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it is already opened from its files.
    pub fn set_catalog_storage(&self, storage: Box<dyn CatalogStorage>) -> Result<()> {
        if self.files()?.is_some() {
            return Err(SqlError::new(Code::Api, "[DESIGN] a catalog's storage is chosen before its database is opened from its files"));
        }
        let name = storage.name().to_owned();
        *self.storages().catalog.write().unwrap_or_else(PoisonError::into_inner) = Arc::from(storage);
        self.emit(&Event::Registered { kind: Extension::CatalogStorage, name });
        Ok(())
    }

    /// \[EXTENSION\] Keeps its log with `storage`, once it is opened from
    /// its files (`Database::open_files`), instead of in the built-in file
    /// log of `OpenOptions::log` (RULES.md D25, D39). A database opened again
    /// must be given the storage that made its log.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it is already opened from its files.
    pub fn set_log_storage(&self, storage: Box<dyn LogStorage>) -> Result<()> {
        if self.files()?.is_some() {
            return Err(SqlError::new(Code::Api, "[DESIGN] a log's storage is chosen before its database is opened from its files"));
        }
        let name = storage.name().to_owned();
        *self.storages().log.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::from(storage));
        self.emit(&Event::Registered { kind: Extension::LogStorage, name });
        Ok(())
    }

    /// The kind of log it is kept with: the one set, else the built-in, of
    /// `options`.
    pub(super) fn log_storage(&self, options: &FileLogOptions) -> Arc<dyn LogStorage> {
        let set = self.storages().log.read().unwrap_or_else(PoisonError::into_inner).clone();
        set.unwrap_or_else(|| Arc::new(FileLogs { options: options.clone() }))
    }

    /// The kind of store its catalog is kept in.
    pub(super) fn catalog_storage(&self) -> Arc<dyn CatalogStorage> {
        Arc::clone(&self.storages().catalog.read().unwrap_or_else(PoisonError::into_inner))
    }
}
