//! \[EXTENSION\] A database's backups, archive, restore points and restores
//! (RULES.md D160 to D166): the parts that need the database, over the
//! formats and targets of `backup.rs`.
//!
//! * `take.rs`: an online backup, full or incremental (D160, D162).
//! * `restore.rs` and `chain.rs`: a restore from a backup and the archive,
//!   to a recovery target (D164, D165).
//! * `verify.rs`: a backup's objects checked without restoring it (D161).
//! * `archive.rs`: the job archiving the log, and the floor below which a
//!   checkpoint keeps the log until it is archived (D163).
//! * `history.rs`: the durable history INFORMATION_SCHEMA shows (D166).
//! * `points.rs`: restore points, and a target rescanned.
//! * `statements.rs`: the SQL statements, the owner's alone.

mod archive;
mod assemble;
pub mod chain;
mod history;
mod points;
mod restore;
mod statements;
mod take;
mod verify;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(super) use self::archive::archived;
pub(crate) use self::history::history_entries;
pub use self::points::Rescanned;
pub use self::restore::{RestoreOptions, RestoreReport, STAGING};
pub use self::take::{BackupOptions, BackupReport};
pub use self::verify::Verified;
use super::formats::FileFormats;
use super::{Database, State};
use crate::backup::history::BackupEntry;
use crate::backup::object::{Reading, Writing};
use crate::backup::{BackupTarget, TargetProvider, Targets, scheme_of};
use crate::error::Result;
use crate::log::Lsn;
use crate::sync::lock_builtin;

/// \[EXTENSION\] A backup under way: its entry, and its bytes copied of
/// those it has to copy.
#[derive(Debug, Clone)]
pub struct Progress {
    /// What the history will record of it.
    pub entry: BackupEntry,
    /// The bytes of its files read so far.
    pub done: u64,
    /// The bytes of its files.
    pub total: u64,
}

/// \[EXTENSION\] What a database keeps of its backups while it runs: the
/// providers of targets, the backups under way, its history once read, and
/// the archiver's place.
#[derive(Default)]
pub(crate) struct Backups {
    targets: Targets,
    pub(crate) running: Mutex<BTreeMap<String, Progress>>,
    history: Mutex<Option<history::HistoryFile>>,
    archiver: Mutex<archive::Archiver>,
    /// Ids given, so two backups begun in one microsecond differ.
    taken: AtomicU64,
}

/// \[EXTENSION\] The first position of the log a checkpoint keeps, as the
/// archive needs (RULES.md D163): every position, until the archiver
/// knows how far the archive goes; then the first not yet archived; or
/// none while archiving is off.
#[derive(Debug)]
pub(crate) struct Floor(AtomicU64);

/// A floor holding nothing back.
const FREE: u64 = u64::MAX;

impl Default for Floor {
    fn default() -> Self {
        Floor(AtomicU64::new(FREE))
    }
}

impl Floor {
    /// Holds nothing back, as archiving is off.
    pub(crate) fn free(&self) {
        self.0.store(FREE, Ordering::Release);
    }

    /// Holds every record back, if nothing is held yet, as archiving is
    /// on and how far it went is not yet known.
    pub(crate) fn hold_all(&self) {
        // Only from FREE, so a floor already set by the archiver stays.
        let _ = self.0.compare_exchange(FREE, 0, Ordering::AcqRel, Ordering::Acquire);
    }

    /// Holds back the records from `first` on, the first not archived.
    pub(crate) fn hold_from(&self, first: Lsn) {
        self.0.store(first.0, Ordering::Release);
    }

    /// Where a checkpoint that would truncate the log before `needed`
    /// truncates it.
    pub(crate) fn clamp(&self, needed: Lsn) -> Lsn {
        Lsn(needed.0.min(self.0.load(Ordering::Acquire)))
    }
}

impl Database {
    /// \[EXTENSION\] Registers `provider`, so locations of its scheme name
    /// its targets in BACKUP DATABASE, ARCHIVE.TARGET and restores (RULES.md
    /// D166). It replaces any provider of that scheme, the built-in ones
    /// among them.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn register_target_provider(&self, provider: Arc<dyn TargetProvider>) -> Result<()> {
        self.snapshot()?;
        self.backups().targets.register(provider);
        Ok(())
    }

    /// \[EXTENSION\] The target at `location`, of the provider its scheme
    /// names (RULES.md D166): one registered, else an extension's of that
    /// scheme (D169), else none.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if no provider has its scheme; the provider's.
    pub fn open_target(&self, location: &str) -> Result<Arc<dyn BackupTarget>> {
        let targets = &self.backups().targets;
        let unserved = scheme_of(location).filter(|scheme| !targets.serves(scheme));
        match unserved.map(|scheme| self.extension_target(scheme)).transpose()?.flatten() {
            Some(extension) => extension.open(location),
            None => targets.open(location),
        }
    }

    /// \[EXTENSION\] The backups under way.
    pub fn backups_running(&self) -> Vec<Progress> {
        lock_builtin(&self.backups().running).values().cloned().collect()
    }

    /// What it keeps of its backups.
    pub(super) fn backups(&self) -> &Backups {
        &self.background().backups
    }

    /// The formats its files are written and read in now: what its
    /// backup objects are written and read in (RULES.md D161).
    fn object_formats(&self) -> Result<(Writing, Reading)> {
        formats_of(&*self.snapshot()?)
    }
}

/// The formats the files of `st` are written and read in now.
fn formats_of(st: &State) -> Result<(Writing, Reading)> {
    let FileFormats { hash, pipeline, hashes, encodings } = st.layout(&st.settings)?.formats;
    Ok((Writing { hash, pipeline }, Reading { hashes, encodings }))
}
