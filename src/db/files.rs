//! \[DESIGN\] A database kept in a directory (RULES.md D25):
//! opened with `Database::open`, checkpointed as it runs
//! (`db/checkpoint.rs`), closed with `Database::close`.
//!
//! ```text
//! catalog.0, .1      the catalog file (`db/catalog_file.rs`)
//! data               the pager file of the row store's tables and the
//!                    B-link indexes, shadowed (`disk/shadow.rs`)
//! data.shadow0, 1    what was written to it since the last checkpoint
//! log/               the log (`log/file.rs`)
//! tmp/               the files of spill areas (`spill.rs`), removed when
//!                    the database opens
//! lock               locked while a process has the database open
//!                    (`FileSystem::lock`)
//! undo/              the row store's older row versions
//!                    (`engine/rows/undo.rs`), removed when the database
//!                    opens
//! ```
//!
//! Older row versions go to a log of their own on disk, so a long-running
//! reader under heavy updates holds disk, not memory, for them. It is never
//! synced, and begun anew whenever the database opens: no transaction
//! outlives the process, so no reader needs one after. The data file is shadowed, so after a crash its tables and
//! indexes are exactly as the last checkpoint committed them, together.
//!
//! * **Created**: a directory holding no catalog gets new files.
//! * **Reopened**: if the catalog file's last image is of a clean close at
//!   the log's end, the definitions are made again from its snapshot, each table and
//!   index reopened in place by its locator (`StorageEngine::open_table`,
//!   `IndexMethod::open`).
//! * **Recovered**: otherwise the database stopped without closing. The
//!   last image's definitions are made again, its tables and indexes
//!   reopened as the checkpoint left them; the log is read from the image's
//!   start (`db/recovery.rs`), definitions made since replayed, changes
//!   undone and redone; then the indexes, whose entries the log does not
//!   hold, are mended for every row recovery touched
//!   (`State::mend_indexes`), and those defined since the checkpoint built.
//!
//! Either way, a table its engine cannot reopen is made anew and its rows
//! replayed from the log, and an index its method cannot reopen is built
//! from its table. Before anything else runs, the catalog file records the
//! image again, not clean, so a crash from then on is recovered.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::checkpoint::Checkpointer;
use super::formats::{FileFormats, FileLayout};
use super::image::Image;
use super::recovery::{Definitions, Touched};
use super::snapshot::Snapshot;
use super::stores::{CatalogStore, StorePlace};
use super::{Database, RecoveryReport, State, api};
use crate::catalog::{IndexDesc, QualifiedName, TableDesc};
use crate::diagnostics::Event;
use crate::disk::buffer::BufferPool;
use crate::disk::encoding::Pipeline;
use crate::disk::io::{Durability, FileLock, FileSystem, OsFileSystem};
use crate::disk::pager::Pager;
use crate::disk::shadow::ShadowFileSystem;
use crate::engine::rows::{RowStore, TableFiles, TableOptions, UndoStore};
use crate::error::Result;
use crate::index::{BTreeMethod, HashMethod, Index, IndexMethod};
use crate::log::{FileLog, FileLogOptions, Log, Lsn};
use crate::spill::TempFiles;
use crate::storage::{StorageEngine, TableStorage};
use crate::txn::Persistence;
use crate::value::{DataType, Value};

const DATA: &str = "data";
/// The directory of the spill areas' files (`spill.rs`).
const TEMP: &str = "tmp";
/// The directory of the log of older row versions.
const UNDO: &str = "undo";
/// The file locked while the database is open.
const LOCK: &str = "lock";

/// \[DESIGN\] The row changes after which a checkpoint is taken, by default.
pub const CHECKPOINT_AFTER: u64 = 100_000;
/// \[DESIGN\] The bytes written to the tables file's side file after which
/// a checkpoint is taken, by default: 64 MiB.
pub const CHECKPOINT_BYTES: u64 = 64 << 20;

/// \[DESIGN\] The bytes of a database's buffer pool, by default: 32 MiB.
pub const POOL_BYTES: usize = 32 << 20;

/// \[DESIGN\] How a database kept in a directory is opened: what its
/// files do not fix. What they do is its settings fixed at creation
/// (`db/formats.rs`); its buffer pool, the setting STORAGE.POOL_BYTES.
#[derive(Clone)]
pub struct OpenOptions {
    /// How durable the built-in log's syncs make its records (`FileLogs`),
    /// when no other log storage is set (`Database::set_log_storage`).
    pub durability: Durability,
    /// The options of the log of older row versions (`undo/`): by default
    /// segments of `UNDO_SEGMENT_SIZE`, never synced.
    pub undo: FileLogOptions,
    /// The bytes of older row versions gathered before they are written to
    /// it as one record (`engine/rows/undo.rs`); 0 writes each alone.
    pub undo_batch: usize,
}

/// \[DESIGN\] The default `OpenOptions::undo_batch`: 32 KiB, a few pages of
/// versions to a write.
pub const UNDO_BATCH: usize = 32 << 10;

/// \[DESIGN\] The default segment size of the log of older row versions:
/// 16 MiB, so its space comes back soon after the readers needing it end.
pub const UNDO_SEGMENT_SIZE: u64 = 16 << 20;

impl Default for OpenOptions {
    fn default() -> Self {
        let undo = FileLogOptions { segment_size: UNDO_SEGMENT_SIZE, durability: Durability::None, ..FileLogOptions::default() };
        OpenOptions { durability: FileLogOptions::default().durability, undo, undo_batch: UNDO_BATCH }
    }
}

/// \[DESIGN\] What opening a database's directory found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    /// No database: one was created.
    Created,
    /// A database closed cleanly, reopened in place.
    Reopened,
    /// A database that stopped without closing, recovered from its last
    /// checkpoint and its log.
    Recovered(RecoveryReport),
}

/// What a database opened from a directory keeps to checkpoint and close.
pub(super) struct DatabaseFiles {
    pub(super) catalog: Box<dyn CatalogStore>,
    /// The shadowing file system the tables file is in.
    pub(super) shadows: Arc<ShadowFileSystem>,
    /// The thread taking checkpoints that come due.
    pub(super) checkpointer: Checkpointer,
    /// The lock of its directory, until it is closed.
    pub(super) held: Mutex<Option<Arc<FileLock>>>,
}

/// \[DESIGN\] While a database is reopened: the locators of its tables and
/// indexes, the tables made anew, and the indexes to build from their
/// tables.
#[derive(Clone, Default)]
pub(super) struct Reopening {
    /// The locators of the tables and indexes, by name and generation: the
    /// image's snapshot makes each table of the generation it is, reopened;
    /// a definition replayed from the log after it may make another, new
    /// (`db/ddl.rs`).
    tables: HashMap<(QualifiedName, u32), Vec<u8>>,
    indexes: HashMap<(String, u32), Vec<u8>>,
    made: Vec<QualifiedName>,
    unbuilt: Vec<String>,
}

/// Where a database's files are: the file system, the directory, what
/// they are made of (`db/formats.rs`), the shadowing file system of the
/// tables file, and the options they are opened with.
struct Place<'a> {
    fs: &'a Arc<dyn FileSystem>,
    dir: &'a Path,
    layout: FileLayout,
    shadows: Arc<ShadowFileSystem>,
    options: &'a OpenOptions,
    /// The lock of its directory, held while it is open.
    held: Arc<FileLock>,
}

impl<'a> Place<'a> {
    /// The place in `dir` of `fs`, of `layout`, opened with `options`, its
    /// directory locked by `held`.
    fn new(fs: &'a Arc<dyn FileSystem>, dir: &'a Path, layout: FileLayout, (options, held): (&'a OpenOptions, Arc<FileLock>)) -> Place<'a> {
        let shadows = Arc::new(ShadowFileSystem::new(Arc::clone(fs), Arc::clone(&layout.formats.hash)));
        Place { fs, dir, layout, shadows, options, held }
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.join(name)
    }

    /// It as its stores see it.
    fn store(&self) -> StorePlace<'_> {
        StorePlace::new(self.fs, self.dir, &self.layout.formats)
    }

    /// The options of the built-in log.
    fn log_options(&self) -> FileLogOptions {
        let segment_size = self.layout.segment_size;
        FileLogOptions {
            segment_size,
            durability: self.options.durability,
            pipeline: self.layout.formats.pipeline.clone(),
            ..FileLogOptions::default()
        }
    }

    /// What the database keeps of its files.
    fn files(&self, catalog: Box<dyn CatalogStore>) -> DatabaseFiles {
        let held = Mutex::new(Some(Arc::clone(&self.held)));
        DatabaseFiles { catalog, shadows: Arc::clone(&self.shadows), checkpointer: Checkpointer::default(), held }
    }
}

/// Whether a pager file is made or opened.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pagers {
    Create,
    Open,
}

impl Database {
    /// \[DESIGN\] The database kept in directory `dir`, created if there is
    /// none: see the module's documentation.
    ///
    /// # Errors
    ///
    /// As `open_files`.
    pub fn open(dir: &Path) -> Result<Database> {
        let db = Database::new();
        let fs: Arc<dyn FileSystem> = Arc::new(OsFileSystem);
        db.open_files(&fs, dir, &OpenOptions::default())?;
        Ok(db)
    }

    /// \[DESIGN\] Opens the database kept in `dir` of `fs` into this one,
    /// which must hold no schema, log or files yet; engines, index methods
    /// and functions its definitions use are registered first. What it
    /// found.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if this database holds a schema, a log or files; -916 if
    /// a file is not of its kind or malformed; -915 if one cannot be read or
    /// written; the errors of recovery.
    pub fn open_files(&self, fs: &Arc<dyn FileSystem>, dir: &Path, options: &OpenOptions) -> Result<Opened> {
        let opened = self.noted(self.open_from(fs, dir, options))?;
        self.emit(&Event::Opened(opened));
        Ok(opened)
    }

    /// `open_files`' work.
    fn open_from(&self, fs: &Arc<dyn FileSystem>, dir: &Path, options: &OpenOptions) -> Result<Opened> {
        let snapshot = self.snapshot()?;
        if !snapshot.catalog.tables().is_empty()
            || !snapshot.catalog.schemas().is_empty()
            || snapshot.log.is_some()
            || self.files()?.is_some()
        {
            return Err(api("[DESIGN] a database is opened from its files before it holds a schema, a log or files"));
        }
        fs.create_dir_all(dir)?;
        // Before anything is cleared or opened: no other has it open.
        let held = Arc::new(fs.lock(&dir.join(LOCK))?);
        snapshot.work.set_temp(TempFiles::new(Arc::clone(fs), &dir.join(TEMP)))?;
        // Its settings given before it opened, of those it reads as it opens.
        let given = snapshot.settings.started();
        // Its catalog is read in whatever registered formats it names: its
        // own are recorded in it.
        let reading = FileFormats { pipeline: Pipeline::none(), ..snapshot.layout(&snapshot.settings)?.formats };
        let Some(found) = self.catalog_at(&StorePlace::new(fs, dir, &reading), options)? else {
            let place = Place::new(fs, dir, snapshot.layout(&snapshot.settings)?, (options, held));
            return self.create_files(&place, &given);
        };
        let image = found.last().map(|record| Image::decode(&record, &snapshot.types)).transpose()?;
        let recorded = |name: &str| image.as_ref().and_then(|i| i.snapshot.setting(name)).cloned();
        let values = snapshot.recorded(&recorded)?;
        let place = Place::new(fs, dir, snapshot.layout(&values)?, (options, held));
        // Read in the encodings its files name; opened again in its own
        // formats, their encodings bound to their arguments (a key's name),
        // as what it records from now on is written through them.
        drop(found);
        let catalog = self.catalog_at(&place.store(), options)?.ok_or_else(|| api("[DESIGN] the catalog went as the database opened"))?;
        let log = self.log_at(&place, image.as_ref().map_or(Lsn(0), |i| i.through))?;
        // The budget the pool is made in: the one recorded, before the
        // definitions are made again.
        snapshot.memory.set_limit(recorded("MEMORY_BUDGET").map_or(crate::memory::DEFAULT_BUDGET, |v| crate::settings::budget_bytes(&v)));
        let (image, opened) = match image {
            Some(image) if image.clean && image.through == log.end() => {
                self.attach_pager(&place, Pagers::Open)?;
                self.reopen(&image, &*log)?;
                (image, Opened::Reopened)
            }
            Some(image) => {
                self.attach_pager(&place, Pagers::Open)?;
                let report = self.reopen(&image, &*log)?.unwrap_or_default();
                (image, Opened::Recovered(report))
            }
            // Made, but stopped before its first image: made again.
            None => {
                self.attach_pager(&place, Pagers::Create)?;
                place.shadows.commit()?;
                let (report, _) = self.replay_log(&*log, Lsn(0), Definitions::All)?;
                self.change(State::rebuild_indexes)?;
                let made = Snapshot::of(&*self.snapshot()?);
                (Image { through: log.end(), start: log.end(), snapshot: made, ..Image::default() }, Opened::Recovered(report))
            }
        };
        catalog.record(&Image { clean: false, ..image }.encode(), self.snapshot()?.compact_after())?;
        self.set_log(log)?;
        self.attach_files(place.files(catalog))?;
        self.opened(&given)?;
        Ok(opened)
    }

    /// Once it is opened: its settings fixed at creation fixed, those given
    /// before it opened, of those it reads as it does, set again over the
    /// ones its definitions held, its budget as its settings say, and its
    /// checkpoints taken as they come due.
    fn opened(&self, given: &[(String, Value)]) -> Result<()> {
        self.change(|st| {
            st.created = true;
            Ok(())
        })?;
        for (name, value) in given {
            self.set_setting(name, value)?;
        }
        self.budget_from_settings()?;
        self.start_checkpointer()
    }

    /// The log at `place`, of the database's log storage, known durable
    /// through `through`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it finds none, though a catalog is there: the log is
    /// kept by another storage. The storage's errors.
    fn log_at(&self, place: &Place<'_>, through: Lsn) -> Result<Arc<dyn Log>> {
        let logs = self.log_storage(&place.log_options());
        logs.open(&place.store().through(through))?.ok_or_else(|| {
            api(format!(
                "[DESIGN] {} holds a catalog, but its log storage, {}, finds no log: open it with the storage that keeps its log (`Database::set_log_storage`)",
                place.dir.display(),
                logs.name()
            ))
        })
    }

    /// The catalog store at `place`, of the database's catalog storage, if
    /// it finds one.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it finds none, but a database's log is there: the
    /// catalog is kept by another storage. The storage's errors.
    fn catalog_at(&self, place: &StorePlace<'_>, options: &OpenOptions) -> Result<Option<Box<dyn CatalogStore>>> {
        let storage = self.catalog_storage();
        let found = storage.open(place)?;
        let logs = self.log_storage(&FileLogOptions { durability: options.durability, ..FileLogOptions::default() });
        if found.is_none() && logs.open(place)?.is_some() {
            return Err(api(format!(
                "[DESIGN] {} holds a database's log, but its catalog storage, {}, finds no catalog: open it with the storage that keeps its catalog (`Database::set_catalog_storage`)",
                place.dir.display(),
                storage.name()
            )));
        }
        Ok(found)
    }

    /// New files for a database at `place`: its first image records its
    /// settings, those fixed at its creation among them.
    fn create_files(&self, place: &Place<'_>, given: &[(String, Value)]) -> Result<Opened> {
        let catalog = self.catalog_storage().create(&place.store())?;
        self.attach_pager(place, Pagers::Create)?;
        place.shadows.commit()?;
        let log = self.log_storage(&place.log_options()).create(&place.store())?;
        let snapshot = Snapshot::of(&*self.snapshot()?);
        catalog.record(
            &Image { through: log.end(), start: log.end(), snapshot, ..Image::default() }.encode(),
            self.snapshot()?.compact_after(),
        )?;
        self.set_log(log)?;
        self.attach_files(place.files(catalog))?;
        self.opened(given)?;
        Ok(Opened::Created)
    }

    /// Makes or opens the data file at `place`, as `how` says, in a buffer
    /// pool, and registers the row store and the B-link index method on it:
    /// tables and indexes in one shadowed file, so one commit holds both. A
    /// file made replaces any there.
    fn attach_pager(&self, place: &Place<'_>, how: Pagers) -> Result<()> {
        let (fs, path, hash) = (&*place.shadows, place.path(DATA), Arc::clone(&place.layout.formats.hash));
        let pager = match how {
            Pagers::Create => {
                if fs.exists(&path)? {
                    fs.remove(&path)?;
                }
                Pager::create(fs, &path, hash, place.layout.formats.pipeline.clone(), place.layout.page_size)?
            }
            Pagers::Open => Pager::open(fs, &path, &place.layout.formats.hashes, &place.layout.formats.encodings)?.0,
        };
        let budget = Arc::clone(&self.snapshot()?.memory);
        let pool =
            Arc::new(BufferPool::in_budget(&budget, (place.layout.pool_bytes, pager.page_size()), "the buffer pool of the data file")?);
        let file = pool.add_pager(Arc::new(pager));
        let undo = UndoStore::batched(undo_log(place)?, place.options.undo_batch);
        let files = TableFiles::over(Arc::clone(&pool), file, undo).shadowed(Arc::clone(&place.shadows));
        // The column store's tables in the row store's files, which its
        // checkpoints keep (RULES.md D31).
        self.register_engine(Box::new(RowStore::columns(files.clone(), TableOptions::default())))?;
        self.register_engine(Box::new(RowStore::new(files, TableOptions::default())))?;
        self.register_index_method(Box::new(BTreeMethod::new(Arc::clone(&pool), file)))?;
        self.register_index_method(Box::new(HashMethod::new(pool, file)))
    }

    /// Makes the definitions of `image` again, reopening its tables and
    /// indexes; replays from `log` what the tables lack, if any lacks
    /// something, with a report of it, and mends the indexes reopened where
    /// it touched rows; builds the indexes not reopened.
    fn reopen(&self, image: &Image, log: &dyn Log) -> Result<Option<RecoveryReport>> {
        let reopening = Reopening {
            tables: image.tables.iter().map(|(name, generation, locator)| ((name.clone(), *generation), locator.clone())).collect(),
            indexes: image.indexes.iter().map(|(name, generation, locator)| ((name.clone(), *generation), locator.clone())).collect(),
            ..Reopening::default()
        };
        self.change(|st| {
            st.reopening = Some(reopening);
            st.install(&image.snapshot)
        })?;
        let snapshot = self.snapshot()?;
        let logged = |s: &&Arc<dyn TableStorage>| s.capabilities().persistence == Persistence::Logged;
        let oldest = snapshot.storage.values().filter(logged).map(|s| s.durable_through()).min().unwrap_or(image.through);
        let (report, mut touched) = if image.clean && oldest >= log.end() {
            (None, Touched::default())
        } else {
            let (report, touched) = self.replay_log(log, oldest.min(image.start), Definitions::From(image.through))?;
            (Some(report), touched)
        };
        self.change(|st| {
            let reopened = st.reopening.take().unwrap_or_default();
            let rebuilt = |ix: &IndexDesc| reopened.unbuilt.contains(&ix.name) || reopened.made.contains(&ix.table);
            // The clock first: rebuilding reads versions, which are seen
            // through the timestamps they were stamped with.
            st.tx.advance_past(image.highest);
            st.mend_indexes(&mut touched, |ix| !rebuilt(ix))?;
            st.rebuild_indexes_where(rebuilt)?;
            Ok(())
        })?;
        Ok(report)
    }
}

/// The log of older row versions at `place`, begun anew: the segments a
/// run before left removed, as no reader of it is left.
///
/// # Errors
///
/// SQLCODE -915 if they cannot be removed, or the log made.
fn undo_log(place: &Place<'_>) -> Result<Arc<dyn Log>> {
    let dir = place.path(UNDO);
    if place.fs.exists(&dir)? {
        for path in place.fs.read_dir(&dir)? {
            place.fs.remove(&path)?;
        }
    }
    let options = FileLogOptions { pipeline: place.layout.formats.pipeline.clone(), ..place.options.undo.clone() };
    Ok(Arc::new(FileLog::create(Arc::clone(place.fs), &dir, Arc::clone(&place.layout.formats.hash), options)?))
}

impl State {
    /// The storage of the table `desc` in `engine`: reopened by its locator
    /// while the database is reopened and the engine can, else new.
    pub(super) fn table_storage(&mut self, engine: &dyn StorageEngine, desc: &TableDesc) -> Result<Arc<dyn TableStorage>> {
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

    /// The index `name` of `method` on columns of `types`, of a table of
    /// generation `generation`: reopened by its
    /// locator while the database is reopened and the method can, else new;
    /// and whether it holds its entries, which a new one does not, unless
    /// it is built once the database is reopened.
    /// Whether the database is being opened from its files, index `name`
    /// then left to be built once its tables are (`Database::reopen`).
    pub(super) fn leave_unbuilt(&mut self, name: &str) -> bool {
        let Some(reopening) = self.reopening.as_mut() else { return false };
        if !reopening.unbuilt.iter().any(|n| n == name) {
            reopening.unbuilt.push(name.to_owned());
        }
        true
    }

    pub(super) fn index_structure(
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
