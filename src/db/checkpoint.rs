//! \[DESIGN\] Checkpoints (RULES.md D25): what lets a
//! database kept in a directory recover from a point near its end, and
//! truncate its log behind it.
//!
//! A checkpoint shuts the gate (`sync::Gate`), which every change it writes
//! takes: a row change with its log record, a commit's record with its
//! versions stamped, an undo, garbage collection, a definition until
//! published. With no such change half done, it
//!
//! 1. takes the log's end, L: the checkpoint holds every change logged
//!    before it;
//! 2. has every engine capture what it holds through L
//!    (`StorageEngine::checkpoint`): the row store writes its pages and its
//!    pager's free list into its shadowed file (`disk/shadow.rs`), no sync,
//!    and seals that batch; and holds back collection of every table with
//!    indexes, noting the last commit captured;
//!
//! and opens the gate again. Then, writers going on,
//!
//! 3. logs, after L, each index entry of the versions awaiting collection
//!    as of that commit (`TableStorage::garbage`), which a crash would
//!    leave behind, as it is read, naming the checkpoint by L, for recovery
//!    from it to take back (`Change::Stale`); then gives the holds up;
//! 4. logs and syncs a checkpoint record, so every record before L is
//!    durable before the pages holding what they record are, and the
//!    stale entries before the image naming them;
//! 5. has every engine make its capture durable
//!    (`StorageEngine::checkpoint_finish`): the row store commits the sealed
//!    batch, so after a crash its tables are as they were at L;
//! 6. records the image in the catalog file: the definitions, each table's
//!    locator, L, and the position of the first record of the oldest
//!    transaction open at L, from which recovery reads the log;
//! 7. truncates the log before that position, or before the oldest
//!    position any table's engine still needs, whichever is earlier.
//!
//! A crash after step 5 and before step 6 leaves tables ahead of the last
//! image: recovery from it redoes and undoes what they already hold, which
//! changes nothing, as both are idempotent. Readers do not take the gate,
//! and go on throughout; writers wait for steps 1 and 2 only. Checkpoints take turns. One is taken when asked
//! (`Database::checkpoint`), when the database is closed, and when one
//! comes due: when a transaction ends after CHECKPOINT.AFTER_CHANGES row
//! changes were logged since the last, or CHECKPOINT.AFTER_BYTES written to
//! the tables file's side file, the ending thread asks the database's
//! checkpoint thread (`Checkpointer`), which takes it, so no transaction
//! waits for it. The thread holds the database weakly: it ends when the
//! database is closed or dropped.
//!
//! Closing, with no transaction open, is a checkpoint that also collects
//! every table's garbage first and checkpoints every index method, and
//! records the image as clean, with each index's locator; every call after
//! is refused.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use super::database::WeakDatabase;
use super::files::DatabaseFiles;
use super::image::Image;
use super::snapshot::Snapshot;
use super::{Database, State, api};
use crate::catalog::{IndexDesc, QualifiedName};
use crate::diagnostics::Event;
use crate::error::{Code, Result, SqlError};
use crate::log::record::{Change, Record};
use crate::log::{Log, Lsn};
use crate::settings::{CHECKPOINT_AFTER_BYTES, CHECKPOINT_AFTER_CHANGES};
use crate::storage::{Row, RowId, TableStorage, project};
use crate::sync::{lock, lock_builtin};
use crate::txn::{NO_TRANSACTION, Persistence, TableLock, Timestamp};
use crate::value::Value;

impl Database {
    /// \[DESIGN\] Takes a checkpoint now: see the module's documentation.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database was not opened from files; the errors
    /// of logging, of an engine's checkpoint, or of writing the catalog
    /// file or truncating the log: what the last checkpoint left then
    /// stands.
    pub fn checkpoint(&self) -> Result<()> {
        let taken = self.noted(self.take_checkpoint());
        if taken.is_ok() {
            self.emit(&Event::CheckpointTaken(self.checkpoints_taken()));
        }
        taken
    }

    /// `checkpoint`'s work.
    fn take_checkpoint(&self) -> Result<()> {
        let files = self.files()?.ok_or_else(|| api("[DESIGN] only a database opened from its files takes checkpoints"))?;
        let _turn = lock(&files.checkpointer.taking)?;
        let current = self.snapshot()?;
        // Not while an index is built: a checkpoint records only indexes
        // ready to be read (`Database::create_index`).
        let shut = loop {
            current.tx.wait_for_builds();
            let shut = current.tx.gate.shut();
            if !current.tx.building() {
                break shut;
            }
        };
        // Taken shut: no definition is half published.
        let st = self.snapshot()?;
        let image = st.capture_checkpoint(false)?;
        let stale = st.hold_stale();
        drop(shut);
        // Writers going on: the entries of what awaited collection as the
        // pages were captured, logged before the record that syncs them.
        st.log_stale(&stale, image.through)?;
        drop(stale);
        st.complete_checkpoint(&files, image)?;
        files.checkpointer.taken.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// \[DESIGN\] The checkpoints taken since the database was opened.
    pub fn checkpoints_taken(&self) -> u64 {
        self.files().ok().flatten().map_or(0, |files| files.checkpointer.taken.load(Ordering::Relaxed))
    }

    /// \[DESIGN\] The failed sync of the log that stopped the database
    /// taking changes, if one did: it then takes none until it is opened
    /// again, and recovered from what its log holds (RULES.md D25).
    pub fn log_failure(&self) -> Option<SqlError> {
        self.snapshot().ok()?.tx.stopped_by()
    }

    /// \[DESIGN\] The error of the last checkpoint the checkpoint thread
    /// took, if it failed; a checkpoint that succeeds clears it.
    pub fn checkpoint_failure(&self) -> Option<SqlError> {
        let files = self.files().ok().flatten()?;
        lock_builtin(&files.checkpointer.failure).clone()
    }

    /// Whether a checkpoint is due: enough row changes logged, or bytes
    /// written to the side file, since the last.
    fn checkpoint_due(&self) -> bool {
        let (Ok(Some(files)), Ok(current)) = (self.files(), self.snapshot()) else { return false };
        let (changes, bytes) = (current.settings.integer(CHECKPOINT_AFTER_CHANGES), current.settings.integer(CHECKPOINT_AFTER_BYTES));
        i128::from(current.tx.logged.load(Ordering::Relaxed)) >= changes || i128::from(files.shadows.pending_bytes()) >= bytes
    }

    /// Asks the checkpoint thread for a checkpoint if one is due.
    pub(super) fn checkpoint_if_due(&self) {
        if self.checkpoint_due()
            && let Ok(Some(files)) = self.files()
        {
            files.checkpointer.request();
        }
    }

    /// Starts the checkpoint thread.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the thread cannot be started.
    pub(super) fn start_checkpointer(&self) -> Result<()> {
        let files = self.files()?.ok_or_else(|| api("[DESIGN] only a database opened from its files has a checkpoint thread"))?;
        let (db, signal) = (self.downgrade(), Arc::clone(&files.checkpointer.signal));
        let thread = thread::Builder::new()
            .name("turborel-checkpoint".into())
            .spawn(move || run(&db, &signal))
            .map_err(|e| SqlError::new(Code::Io, format!("[DESIGN] the checkpoint thread could not be started: {e}")))?;
        *lock(&files.checkpointer.thread)? = Some(thread);
        Ok(())
    }

    /// \[DESIGN\] Closes the database cleanly: see the module's
    /// documentation. Opening its directory again reopens it in place. A
    /// database not closed is recovered from its last checkpoint when
    /// opened.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was not opened from files, or a transaction is
    /// open; the errors of a checkpoint, or of an index method's: the
    /// database is then left open.
    pub fn close(&self) -> Result<()> {
        let closed = self.noted(self.close_files());
        if closed.is_ok() {
            self.emit(&Event::Closed);
        }
        closed
    }

    /// `close`'s work.
    fn close_files(&self) -> Result<()> {
        let files = self.files()?.ok_or_else(|| api("[DESIGN] only a database opened from its files is closed"))?;
        // Its checkpoint ends first.
        files.checkpointer.stop();
        let _turn = lock(&files.checkpointer.taking)?;
        let current = self.snapshot()?;
        // Shut before the turn: a definition holds the gate while it waits
        // for the turn.
        let _shut = current.tx.gate.shut();
        self.change(|st| {
            if st.tx.active() > 0 {
                return Err(api("[DESIGN] a database is closed with no transaction open"));
            }
            let tables: Vec<QualifiedName> = st.catalog.tables().iter().map(|t| t.name.clone()).collect();
            st.gc(&tables);
            for method in st.methods.values() {
                method.checkpoint()?;
            }
            let image = st.capture_checkpoint(true)?;
            st.complete_checkpoint(&files, image)
        })?;
        self.mark_closed();
        // Another may open it now.
        lock(&files.held)?.take();
        Ok(())
    }
}

/// \[DESIGN\] A database's checkpoint thread, and what checkpoints share.
#[derive(Default)]
pub(super) struct Checkpointer {
    signal: Arc<Signal>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// Held while a checkpoint is taken, so checkpoints take turns.
    taking: Mutex<()>,
    taken: AtomicU64,
    failure: Mutex<Option<SqlError>>,
}

/// What wakes the checkpoint thread.
#[derive(Default)]
struct Signal {
    state: Mutex<Wake>,
    wake: Condvar,
}

/// Why the checkpoint thread wakes: a checkpoint due, or to end.
#[derive(Default)]
struct Wake {
    due: bool,
    stop: bool,
}

impl Signal {
    /// Sets what `change` sets, and wakes the thread.
    fn set(&self, change: impl FnOnce(&mut Wake)) {
        change(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner));
        self.wake.notify_one();
    }
}

impl Checkpointer {
    /// Asks the thread for a checkpoint.
    fn request(&self) {
        self.signal.set(|w| w.due = true);
    }

    /// Ends the thread, after the checkpoint it is taking, if any.
    fn stop(&self) {
        self.signal.set(|w| w.stop = true);
        let thread = lock_builtin(&self.thread).take();
        if let Some(thread) = thread.filter(|t| t.thread().id() != thread::current().id()) {
            let _ = thread.join();
        }
    }
}

/// Dropped with the database: the thread is told to end, and waited for,
/// unless the thread itself dropped the database's last handle.
impl Drop for Checkpointer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The checkpoint thread: waits until a checkpoint is due, takes it, and
/// keeps its outcome; ends when told, or when the database is gone.
fn run(db: &WeakDatabase, signal: &Signal) {
    loop {
        {
            let mut wake = signal.state.lock().unwrap_or_else(PoisonError::into_inner);
            while !wake.due && !wake.stop {
                wake = signal.wake.wait(wake).unwrap_or_else(PoisonError::into_inner);
            }
            if wake.stop {
                return;
            }
            wake.due = false;
        }
        let Some(db) = db.upgrade() else { return };
        // Asked before a checkpoint taken meanwhile, it may be due no more.
        if !db.checkpoint_due() {
            continue;
        }
        let outcome = db.checkpoint().err();
        if let Ok(Some(files)) = db.files() {
            *lock_builtin(&files.checkpointer.failure) = outcome;
        }
    }
}

impl State {
    /// What a checkpoint does with writers held (see the module's
    /// documentation): the position L, every engine's capture through it,
    /// and the image but for the tables' locators. A close logs and syncs
    /// its checkpoint record first, so L is the log's end after it.
    fn capture_checkpoint(&self, clean: bool) -> Result<Image> {
        let log = self.log.clone().ok_or_else(|| api("[DESIGN] a database opened from its files has a log"))?;
        // Not once a sync of the log failed: its image would claim what
        // the log may not hold (`txn/logging.rs`); nor while a table is
        // damaged, whose state no transaction made, so that recovery from
        // the last checkpoint and the log can repair it (RULES.md D19).
        self.tx.taking_changes()?;
        if let Some(table) = self.damaged_tables().first() {
            return Err(SqlError::new(
                Code::Damaged,
                format!("[DESIGN] {table} is damaged: no checkpoint is taken until it is repaired (`Database::repair_table`)"),
            ));
        }
        if clean {
            let at = log.append(&Record { tx: NO_TRANSACTION, change: Change::Checkpoint }.encode())?;
            self.tx.sync(&*log, at)?;
        }
        let through = log.end();
        for engine in self.engines.values() {
            engine.checkpoint(through)?;
        }
        self.tx.logged.store(0, Ordering::Relaxed);
        Ok(Image {
            clean,
            through,
            start: self.tx.oldest_logged().map_or(through, |first| first.min(through)),
            highest: self.tx.highest(),
            snapshot: Snapshot::of(self),
            tables: Vec::new(),
            // Those of the catalog: not one whose build failed (`Database::create_index`).
            indexes: self
                .indexes
                .iter()
                .filter(|(name, _)| self.catalog.index(name).is_some())
                .map(|(name, index)| (name.clone(), self.index_generation(name), index.locator()))
                .collect(),
        })
    }

    /// What awaits collection as the pages are captured, under the gate:
    /// each table with indexes, its collection held back until the entries
    /// are logged, and the timestamp of the last commit captured. A close
    /// collects every version first, and needs none.
    fn hold_stale(&self) -> Stale {
        let mut tables = Vec::new();
        for table in self.catalog.tables() {
            let indexes: Vec<IndexDesc> = self.catalog.indexes(&table.name).into_iter().cloned().collect();
            let (Ok(storage), Ok(lock)) = (self.storage_of(&table.name), self.lock_of(&table.name)) else { continue };
            if !indexes.is_empty() {
                lock.hold_collection();
                tables.push((Arc::clone(storage), indexes, Arc::clone(lock)));
            }
        }
        Stale { as_of: self.tx.stable(), tables }
    }

    /// Logs the index entries of the versions `stale` holds (its tables'
    /// `TableStorage::garbage`), each as it is read, in a record naming the
    /// checkpoint holding every change before `through`, for recovery from
    /// it to take back (`db/recovery.rs`). They are written after
    /// `through`, so recovery reads them, and before the checkpoint record
    /// whose sync makes them durable before the image is recorded.
    fn log_stale(&self, stale: &Stale, through: Lsn) -> Result<()> {
        let log = self.log.clone().ok_or_else(|| api("[DESIGN] a database opened from its files has a log"))?;
        for (storage, indexes, _) in &stale.tables {
            for read in Arc::clone(storage).garbage(stale.as_of) {
                let (row, values) = read?;
                log_entries(&*log, (through, indexes), row, &values)?;
            }
        }
        Ok(())
    }

    /// The rest, writers going on: the log made durable past L, before the
    /// pages holding what it records are (a checkpoint record logged and
    /// synced, unless a close did), every engine's capture made durable,
    /// the image recorded with the tables' locators, the log truncated.
    fn complete_checkpoint(&self, files: &DatabaseFiles, mut image: Image) -> Result<()> {
        let log = self.log.clone().ok_or_else(|| api("[DESIGN] a database opened from its files has a log"))?;
        if !image.clean {
            let at = log.append(&Record { tx: NO_TRANSACTION, change: Change::Checkpoint }.encode())?;
            self.tx.sync(&*log, at)?;
        }
        for engine in self.engines.values() {
            engine.checkpoint_finish()?;
        }
        image.tables = self.storage.iter().map(|(name, storage)| (name.clone(), self.generation_of(name), storage.locator())).collect();
        files.catalog.record(&image.encode(), self.compact_after())?;
        let logged = self.storage.values().filter(|s| s.capabilities().persistence == Persistence::Logged);
        let needed = logged.map(|s| s.durable_through()).fold(image.start, Ord::min);
        log.truncate_before(needed)
    }
}

/// Logs the entry of row `row`, of `values`, in each of `indexes` whose key
/// has no null (`Index::insert`), stale as of the checkpoint through
/// `through`.
fn log_entries(log: &dyn Log, (through, indexes): (Lsn, &[IndexDesc]), row: RowId, values: &Row) -> Result<()> {
    for ix in indexes {
        let Some(key) = project(values, &ix.columns).filter(|k| !k.iter().any(Value::is_null)) else { continue };
        let change = Change::Stale { through, index: ix.name.clone(), row, key };
        log.append(&Record { tx: NO_TRANSACTION, change }.encode())?;
    }
    Ok(())
}

/// The tables whose entries of what awaits collection a checkpoint logs,
/// each with its indexes and its lock, whose hold on collection it gives
/// up when dropped; and the timestamp of the last commit it captured.
struct Stale {
    as_of: Timestamp,
    tables: Vec<Held>,
}

/// A table whose collection a checkpoint holds back: its storage, its
/// indexes, and its lock.
type Held = (Arc<dyn TableStorage>, Vec<IndexDesc>, Arc<TableLock>);

impl Drop for Stale {
    fn drop(&mut self) {
        for (_, _, lock) in &self.tables {
            lock.release_collection();
        }
    }
}
