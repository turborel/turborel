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
//!    and seals that batch; what plug-ins write meanwhile to their
//!    shadowed directories (RULES.md D56) is held in memory, then sealed;
//!    and holds back collection of every table with indexes, noting the
//!    last commit captured;
//!
//! and opens the gate again. Then, writers going on,
//!
//! 3. logs, after L, each index entry of the versions awaiting collection
//!    as of that commit (`TableStorage::garbage`), which a crash would
//!    leave behind, as it is read, naming the checkpoint by L, for recovery
//!    from it to take back (`Change::Stale`); then gives the holds up;
//! 4. logs and syncs a checkpoint record, so every record before L is
//!    durable before the pages holding what they record are, and the
//!    stale entries before the image naming them; the checkpoints' turn
//!    let go for the sync and taken again after: on a replicated log the
//!    sync waits for the group, which may wait on a backup fetching this
//!    database's state, an export holding the turn (RULES.md D57), and
//!    the files are as the last image left them until step 5;
//! 5. commits the plug-ins' sealed batch, then has every engine make its
//!    capture durable (`StorageEngine::checkpoint_finish`): the row store
//!    commits the sealed batch, so after a crash its tables are as they
//!    were at L;
//! 6. records the image in the catalog file: the definitions, each table's
//!    and index's locator, L, and the position of the first record of the oldest
//!    transaction open at L, from which recovery reads the log;
//! 7. truncates the log before that position, or before the oldest
//!    position any table's engine still needs, whichever is earlier.
//!
//! A crash after step 5 and before step 6 leaves tables ahead of the last
//! image: recovery from it redoes and undoes what they already hold, which
//! changes nothing, as both are idempotent. Readers do not take the gate,
//! and go on throughout; writers wait for steps 1 and 2 only. Checkpoints
//! take turns, for their whole lives, but for a close, which takes only
//! the turn step 4 lets go: one in its sync then fails, the files closed
//! under it. One is taken when asked
//! (`Database::checkpoint`), when the database is closed, and when one
//! comes due: when a transaction ends after CHECKPOINT.AFTER_CHANGES row
//! changes were logged since the last taken (a failed one leaves them
//! counted), or CHECKPOINT.AFTER_BYTES written to
//! the tables file's side file, the ending thread asks for the checkpoint
//! job, which runs as JOBS.POLICY says (`db/background.rs`), so no
//! transaction waits for it. Its thread holds the database weakly: it ends
//! when the database is closed or dropped. Checkpoints overdue, the log growing
//! still, are told (`overdue.rs`).
//!
//! Closing, with no transaction open, is a checkpoint that also collects
//! every table's garbage first and checkpoints every index method, logs
//! and syncs its checkpoint record before step 1, and records the image as
//! clean, unless index entries collection could not take back are still
//! owed: they are logged as a checkpoint logs them, and the image is not
//! clean, so opening the database recovers it (`txn/owed.rs`). Every call
//! after is refused. Then, before its directory's lock
//! is given up, its spill files are removed and every file's descriptor
//! closed, none opened after (RULES.md D56); a file left is told as an
//! event.

mod following;
mod overdue;

pub use overdue::OVERDUE;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::files::DatabaseFiles;
use super::image::Image;
use super::snapshot::Snapshot;
use super::{Database, State, api};
use crate::catalog::{IndexDesc, QualifiedName, TableDesc};
use crate::diagnostics::Event;
use crate::error::{Code, Result, SqlError};
use crate::log::record::{Change, Record};
use crate::log::{Log, Lsn};
use crate::settings::{CHECKPOINT_AFTER_BYTES, CHECKPOINT_AFTER_CHANGES};
use crate::storage::{Row, RowId, TableStorage};
use crate::sync::{lock, lock_builtin};
use crate::threads::{Turn, Turned};
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
            // Taken, it may still give up no log: a transaction holds it.
            self.tell_if_overdue();
            self.tell_if_settled();
        }
        taken
    }

    /// `checkpoint`'s work.
    fn take_checkpoint(&self) -> Result<()> {
        let files = self.files()?.ok_or_else(|| api("[DESIGN] only a database opened from its files takes checkpoints"))?;
        let _whole = files.checkpointer.whole.take("a checkpoint")?;
        let turn = files.checkpointer.taking.take("a checkpoint")?;
        let current = self.snapshot()?;
        if matches!(current.tx.held(), Some(crate::txn::Held::Following)) {
            return self.take_following_checkpoint(&files);
        }
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
        let (image, counted) = st.capture_checkpoint(false)?;
        let stale = st.hold_stale();
        drop(shut);
        // Writers going on: the entries of what awaited collection as the
        // pages were captured, logged before the record that syncs them.
        let logged = st.log_stale(&stale, image.through);
        drop(stale);
        // The turn let go while the record is synced (step 4), then taken
        // again to record the image.
        let synced = logged.and_then(|()| st.sync_checkpoint_record(turn, &image));
        let recorded = synced.and_then(|()| {
            let _turn = files.checkpointer.taking.take("a checkpoint")?;
            st.record_and_truncate(&files, image)
        });
        st.counted_if(recorded, counted)?;
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
        i128::from(current.tx.since_checkpoint()) >= changes || i128::from(files.shadows.pending_bytes()) >= bytes
    }

    /// Asks for the checkpoint job if a checkpoint is due; tells, then, if
    /// checkpoints are overdue (`overdue.rs`), as each checkpoint taken
    /// does.
    pub(super) fn checkpoint_if_due(&self) {
        if self.checkpoint_due() {
            self.request_checkpoint();
            self.tell_if_overdue();
        }
    }

    /// \[DESIGN\] Closes the database cleanly: see the module's
    /// documentation. Opening its directory again reopens it in place. A
    /// database not closed is recovered from its last checkpoint when
    /// opened.
    ///
    /// A replica's database, following, takes a following checkpoint
    /// instead, logging nothing; deposed, none (RULES.md D57, `close_held`).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was not opened from files, or a transaction is
    /// open; -915 once a sync of its log failed (RULES.md D25); the errors
    /// of a checkpoint, or of an index method's: the database is then left
    /// open.
    pub fn close(&self) -> Result<()> {
        let closed = self.noted(self.close_files());
        if closed.is_ok() {
            self.emit(&Event::Closed);
        }
        closed
    }

    /// `close`'s work: its background work's threads stopped first, and
    /// let start again if it fails (RULES.md D55).
    fn close_files(&self) -> Result<()> {
        self.background_closing();
        let closed = self.close_closing();
        if closed.is_err() {
            self.background_reopened();
        }
        closed
    }

    /// `close_files`' work, no background work's thread running.
    fn close_closing(&self) -> Result<()> {
        let files = self.files()?.ok_or_else(|| api("[DESIGN] only a database opened from its files is closed"))?;
        let _turn = files.checkpointer.taking.take("a checkpoint")?;
        let current = self.snapshot()?;
        if matches!(current.tx.held(), Some(crate::txn::Held::Following | crate::txn::Held::Deposed)) {
            return self.close_held(&files, &current);
        }
        // Shut before the turn: a definition holds the gate while it waits
        // for the turn.
        let _shut = current.tx.gate.shut();
        self.change(|st| {
            // After a failed sync of the log, never cleanly: opened again,
            // it is recovered (RULES.md D25).
            st.tx.taking_changes()?;
            if st.tx.active() > 0 {
                return Err(api("[DESIGN] a database is closed with no transaction open"));
            }
            let tables = st.catalog.tables();
            st.gc(tables.iter().map(|t| &t.name));
            for method in st.methods.values() {
                method.checkpoint()?;
            }
            // Index entries still owed, the close is not clean: they are
            // logged, so opening the database recovers it, taking them back.
            let clean = st.owing().is_none();
            let (image, counted) = st.capture_checkpoint(clean)?;
            if !clean {
                st.log_stale(&st.hold_stale(), image.through)?;
            }
            st.counted_if(st.complete_checkpoint(&files, image), counted)
        })?;
        self.mark_closed();
        self.close_every_file(&current);
        // Another may open it now.
        lock(&files.held)?.take();
        Ok(())
    }

    /// \[DESIGN\] A replica's database closed (RULES.md D57), its changes
    /// held, so it logs no checkpoint record of a clean close. Following,
    /// every record it holds is committed: a following checkpoint is taken,
    /// which logs nothing, unless it replayed nothing since it was opened
    /// from its last. Deposed, it holds records it wrote as primary that
    /// the group may not have committed: no checkpoint is taken, and its
    /// last stands. Then its files are closed. Opened again, it recovers
    /// from its last checkpoint alone, and follows.
    fn close_held(&self, files: &DatabaseFiles, current: &State) -> Result<()> {
        let _shut = current.tx.gate.shut();
        if current.tx.active() > 0 {
            return Err(api("[DESIGN] a database is closed with no transaction open"));
        }
        let following = *lock_builtin(&current.tx.following);
        let replayed_since = Lsn(following.replayed.0.saturating_add(1)) >= following.image;
        if current.tx.held() == Some(crate::txn::Held::Following) && replayed_since {
            self.take_following_checkpoint(files)?;
        }
        self.mark_closed();
        self.close_every_file(current);
        lock(&files.held)?.take();
        Ok(())
    }

    /// Removes the spill files of `st`, and closes every file's
    /// descriptor, none opened after (RULES.md D56): a spill file not
    /// removed, or a file still being read or written, told as an event.
    fn close_every_file(&self, st: &State) {
        if let Err(e) = st.work.clear_temp() {
            self.emit(&Event::Failed(e));
        }
        let open = st.files.close_all();
        if !open.is_empty() {
            let paths: Vec<String> = open.iter().map(|p| p.display().to_string()).collect();
            let why = format!("[DESIGN] files left open as the database closed: {}", paths.join(", "));
            self.emit(&Event::Failed(SqlError::new(Code::Io, why)));
        }
    }
}

/// \[DESIGN\] What a database's checkpoints share.
#[derive(Default)]
pub(super) struct Checkpointer {
    /// Taken while a checkpoint is, so checkpoints take turns; refusing
    /// one asked for from within a checkpoint (RULES.md R-ERR-4). An
    /// export takes it too, so no checkpoint changes the files it lists
    /// (`db/export.rs`).
    pub(super) taking: Turn,
    /// Taken for a checkpoint's whole life, so checkpoints take turns
    /// while `taking` is let go for the sync.
    whole: Turn,
    taken: AtomicU64,
    failure: Mutex<Option<SqlError>>,
    /// When checkpoints overdue were last told (`overdue.rs`).
    overdue_told: Mutex<Option<Instant>>,
}

/// The checkpoint job's work: the checkpoint due, if one still is (one
/// taken meanwhile may have made it due no more), its outcome kept.
///
/// # Errors
///
/// The checkpoint's.
pub(super) fn checkpointed(db: &Database) -> Result<Option<Duration>> {
    if db.checkpoint_due() {
        let outcome = db.checkpoint();
        if let Ok(Some(files)) = db.files() {
            *lock_builtin(&files.checkpointer.failure) = outcome.as_ref().err().cloned();
        }
        outcome?;
    }
    Ok(None)
}

impl State {
    /// The first table that owes index entries collection could not take
    /// back, if one does (`txn/owed.rs`).
    pub(super) fn owing(&self) -> Option<QualifiedName> {
        let owes = |t: &&TableDesc| self.lock_of(&t.name).is_ok_and(|lock| !lock.owed.is_empty());
        self.catalog.tables().into_iter().find(owes).map(|t| t.name.clone())
    }

    /// What a checkpoint does with writers held (see the module's
    /// documentation): the position L, every engine's capture through it,
    /// and the image but for the tables' locators; and the row changes
    /// logged through L, which, once the checkpoint is taken, no longer
    /// count toward the next (CHECKPOINT.AFTER_CHANGES, `counted_if`). A
    /// close logs and syncs its checkpoint record first, so L is the log's
    /// end after it.
    fn capture_checkpoint(&self, clean: bool) -> Result<(Image, u64)> {
        let log = self.log.clone().ok_or_else(|| api("[DESIGN] a database opened from its files has a log"))?;
        self.checkpoint_refused()?;
        if clean {
            let at = log.append(&Record { tx: NO_TRANSACTION, change: Change::Checkpoint }.encode())?;
            self.tx.sync(&*log, at)?;
        }
        // \[DESIGN\] Following, the log's records after those replayed may
        // not be committed, a view change may yet replace them: the
        // checkpoint holds every record through the last replayed, and no
        // more (RULES.md D57).
        let through = match self.tx.held() {
            Some(crate::txn::Held::Following) => Lsn(lock_builtin(&self.tx.following).replayed.0.saturating_add(1)),
            Some(crate::txn::Held::Deposed | crate::txn::Held::Retired) | None => log.end(),
        };
        self.capture_engines(through)?;
        let counted = self.tx.logged.load(Ordering::Relaxed);
        let image = self.image_of(clean, (through, self.tx.oldest_logged().map_or(through, |first| first.min(through))))?;
        Ok((image, counted))
    }

    /// The image of a checkpoint holding every change before `through`,
    /// recovery reading the log from `start`, closed if `clean`, but for
    /// the tables' locators.
    ///
    /// # Errors
    ///
    /// `Snapshot::of`'s.
    fn image_of(&self, clean: bool, (through, start): (Lsn, Lsn)) -> Result<Image> {
        Ok(Image {
            clean,
            through,
            start,
            highest: self.tx.highest(),
            snapshot: Snapshot::of(self)?,
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

    /// Every engine's capture through `through`, what the plug-ins'
    /// shadowed directories write meanwhile held in memory, then sealed
    /// (RULES.md D56); given up if one fails.
    fn capture_engines(&self, through: Lsn) -> Result<()> {
        self.plugins.hold()?;
        let captured = self.engines.values().try_for_each(|engine| engine.checkpoint(through)).and_then(|()| self.plugins.seal());
        if captured.is_err() {
            let _ = self.plugins.release();
        }
        captured
    }

    /// Whether a checkpoint may be taken. Not once a sync of the log
    /// failed: its image would claim what the log may not hold
    /// (`txn/logging.rs`); nor while a table is damaged, whose state no
    /// transaction made, so that recovery from the last checkpoint and the
    /// log can repair it (RULES.md D19).
    ///
    /// # Errors
    ///
    /// SQLCODE -915 after a failed sync of the log; -914 naming the first
    /// table damaged.
    pub(super) fn checkpoint_refused(&self) -> Result<()> {
        self.tx.taking_changes()?;
        match self.damaged_tables().first() {
            Some(table) => Err(SqlError::new(
                Code::Damaged,
                format!("[DESIGN] {table} is damaged: no checkpoint is taken until it is repaired (`Database::repair_table`)"),
            )),
            None => Ok(()),
        }
    }

    /// `finished`, the outcome of a checkpoint that holds the first
    /// `counted` row changes logged: taken, they no longer count toward
    /// the next; failed, they still do, so the next is due as soon as it
    /// would have been.
    fn counted_if(&self, finished: Result<()>, counted: u64) -> Result<()> {
        if finished.is_ok() {
            self.tx.checkpointed(counted);
        }
        finished
    }

    /// What awaits collection as the pages are captured, under the gate:
    /// each table with indexes, its collection held back until the entries
    /// are logged, and the timestamp of the last commit captured. A close
    /// collects every version first, and needs it only for the index
    /// entries owed (`txn/owed.rs`).
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
    /// `TableStorage::garbage`), each as it is read, and those its tables
    /// owe (`txn/owed.rs`), in a record naming the
    /// checkpoint holding every change before `through`, for recovery from
    /// it to take back (`db/recovery.rs`). They are written after
    /// `through`, so recovery reads them, and before the checkpoint record
    /// whose sync makes them durable before the image is recorded.
    fn log_stale(&self, stale: &Stale, through: Lsn) -> Result<()> {
        let log = self.log.clone().ok_or_else(|| api("[DESIGN] a database opened from its files has a log"))?;
        for (storage, indexes, lock) in &stale.tables {
            for read in Arc::clone(storage).garbage(stale.as_of) {
                let (row, values) = read?;
                log_entries(&*log, (through, indexes), row, &values)?;
            }
            lock.owed.each(&mut |index, key, row| {
                let change = Change::Stale { through, index: index.to_owned(), row, key: key.0.clone() };
                log.append(&Record { tx: NO_TRANSACTION, change }.encode()).map(|_| ())
            })?;
        }
        Ok(())
    }

    /// The rest of a close's checkpoint, writers going on: the log made
    /// durable past L, before the pages holding what it records are (a
    /// checkpoint record logged and synced, unless the close did), every
    /// engine's capture made durable, the image recorded with the tables'
    /// locators, the log truncated.
    fn complete_checkpoint(&self, files: &DatabaseFiles, image: Image) -> Result<()> {
        if !image.clean {
            let log = self.log.clone().ok_or_else(|| api("[DESIGN] a database opened from its files has a log"))?;
            let at = log.append(&Record { tx: NO_TRANSACTION, change: Change::Checkpoint }.encode())?;
            self.tx.sync(&*log, at)?;
        }
        self.record_and_truncate(files, image)
    }

    /// Step 4: the checkpoint record logged, the checkpoints' `turn` let
    /// go, and the log synced through the record, so every record before
    /// L, and the stale entries, are durable before the image is recorded.
    fn sync_checkpoint_record(&self, turn: Turned<'_>, image: &Image) -> Result<()> {
        let log = self.log.clone().ok_or_else(|| api("[DESIGN] a database opened from its files has a log"))?;
        let at = if image.clean { None } else { Some(log.append(&Record { tx: NO_TRANSACTION, change: Change::Checkpoint }.encode())?) };
        drop(turn);
        at.map_or(Ok(()), |at| self.tx.sync(&*log, at))
    }

    /// Steps 5 to 7, the turn held: every engine's capture made durable,
    /// the image recorded with the tables' locators, the log truncated.
    fn record_and_truncate(&self, files: &DatabaseFiles, image: Image) -> Result<()> {
        let log = self.log.clone().ok_or_else(|| api("[DESIGN] a database opened from its files has a log"))?;
        let image = self.record_image(files, image)?;
        let logged = self.storage.values().filter(|s| s.capabilities().persistence == Persistence::Logged);
        let needed = logged.map(|s| s.durable_through()).fold(image.start, Ord::min);
        log.truncate_before(needed)
    }
}

impl State {
    /// Steps 5 and 6: the plug-ins' sealed batch committed, every engine's
    /// capture made durable, and `image` recorded with the tables'
    /// locators; the image recorded.
    fn record_image(&self, files: &DatabaseFiles, mut image: Image) -> Result<Image> {
        // The plug-ins' shadowed directories first, so an engine's capture
        // is durable as its `checkpoint_finish` is called (RULES.md D56).
        self.plugins.commit_sealed()?;
        for engine in self.engines.values() {
            engine.checkpoint_finish()?;
        }
        image.tables =
            self.storage.iter().map(|(name, storage)| (QualifiedName::clone(name), self.generation_of(name), storage.locator())).collect();
        files.catalog.record(&image.encode(), self.compact_after())?;
        Ok(image)
    }
}

/// Logs the entry of row `row`, of `values`, in each of `indexes` whose key
/// has no null (`Index::insert`), stale as of the checkpoint through
/// `through`.
fn log_entries(log: &dyn Log, (through, indexes): (Lsn, &[IndexDesc]), row: RowId, values: &Row) -> Result<()> {
    for ix in indexes {
        let Some(key) = ix.key_of(values).filter(|k| !k.0.iter().any(Value::is_null)) else { continue };
        let change = Change::Stale { through, index: ix.name.clone(), row, key: key.0 };
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
