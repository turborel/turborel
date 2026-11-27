//! \[EXTENSION\] The log a restored database is opened with (RULES.md
//! D164): a seed's records, then a file log of its own, whose first
//! segment is numbered past them (`seed.rs`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::seed::{Seed, SeedRecords};
use crate::db::{FileLogs, LogStorage, OpenedLog, StorePlace};
use crate::disk::encoding::Pipeline;
use crate::disk::io::Durability;
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;
use crate::log::{FileLog, FileLogOptions, Log, LogCapabilities, Lsn, OFFSET_BITS, Records};
use crate::sync::lock_builtin;

/// \[EXTENSION\] A seed's records, then a log of the database's own: see
/// the module's documentation.
pub struct SeededLog {
    seed: Mutex<Option<Arc<Seed>>>,
    log: Arc<dyn Log>,
    /// The position before which the records are the seed's.
    start: Lsn,
}

impl SeededLog {
    /// `seed`'s records, then `log`'s, every one of which is at or after
    /// `start`.
    pub fn new(seed: Arc<Seed>, log: Arc<dyn Log>, start: Lsn) -> SeededLog {
        SeededLog { seed: Mutex::new(Some(seed)), log, start }
    }
}

impl Log for SeededLog {
    fn name(&self) -> &'static str {
        self.log.name()
    }
    fn configurable(&self) -> Option<&dyn crate::settings::Configurable> {
        self.log.configurable()
    }
    fn capabilities(&self) -> LogCapabilities {
        self.log.capabilities()
    }
    fn set_durability(&self, durability: Durability) {
        self.log.set_durability(durability);
    }
    fn set_formats(&self, hash: Arc<dyn HashFunction>, pipeline: Pipeline) {
        self.log.set_formats(hash, pipeline);
    }
    fn formats_behind(&self) -> u64 {
        self.log.formats_behind()
    }
    fn append(&self, record: &[u8]) -> Result<Lsn> {
        self.log.append(record)
    }
    fn sync(&self, through: Lsn) -> Result<()> {
        // A seed's record is in its backup or archive already.
        if through < self.start { Ok(()) } else { self.log.sync(through) }
    }
    fn buffers(&self) -> bool {
        self.log.buffers()
    }
    fn write_buffered(&self) -> Result<()> {
        self.log.write_buffered()
    }
    fn read_from(&self, from: Lsn) -> Records<'_> {
        let seed = lock_builtin(&self.seed).clone();
        match seed.filter(|_| from < self.start) {
            Some(seed) => Box::new(SeedRecords::new(seed, from).chain(self.log.read_from(Lsn(0)))),
            None if from < self.start => self.log.read_from(Lsn(0)),
            None => self.log.read_from(from),
        }
    }
    fn truncate_before(&self, before: Lsn) -> Result<()> {
        if before < self.start {
            return Ok(());
        }
        lock_builtin(&self.seed).take();
        self.log.truncate_before(before)
    }
    fn end(&self) -> Lsn {
        self.log.end()
    }

    fn bytes_between(&self, from: Lsn, to: Lsn) -> u64 {
        self.log.bytes_between(from, to)
    }
}

/// \[EXTENSION\] The kind of log a restore opens its database with: the
/// first time, a seeded log over a new file log in `log/` whose first
/// segment is `first`; after, as the built-in file logs are opened.
pub struct SeededLogs {
    seed: Mutex<Option<Arc<Seed>>>,
    first: u64,
    files: FileLogs,
}

impl SeededLogs {
    /// The log of `seed`'s records, the last at `last`, then a new file
    /// log of `options`.
    pub fn new(seed: Arc<Seed>, last: Lsn, options: FileLogOptions) -> SeededLogs {
        let first = (last.0 >> OFFSET_BITS).saturating_add(1);
        SeededLogs { seed: Mutex::new(Some(seed)), first, files: FileLogs { options } }
    }
}

/// The log directory at `place`, as the built-in file logs keep it.
fn log_dir(place: &StorePlace<'_>) -> PathBuf {
    place.dir.join(crate::db::LOG_DIR)
}

impl LogStorage for SeededLogs {
    fn name(&self) -> &'static str {
        "restored"
    }

    fn open(&self, place: &StorePlace<'_>) -> Result<Option<OpenedLog>> {
        let Some(seed) = lock_builtin(&self.seed).take() else { return self.files.open(place) };
        let options = FileLogOptions { pipeline: place.formats.pipeline.clone(), ..self.files.options.clone() };
        let log = FileLog::create_at(Arc::clone(place.fs), &log_dir(place), (Arc::clone(&place.formats.hash), options), self.first)?;
        let start = log.end();
        Ok(Some(OpenedLog { log: Arc::new(SeededLog::new(seed, Arc::new(log), start)), cut: 0 }))
    }

    fn exists(&self, place: &StorePlace<'_>) -> Result<bool> {
        self.files.exists(place)
    }

    fn create(&self, _place: &StorePlace<'_>) -> Result<Arc<dyn Log>> {
        Err(SqlError::new(Code::Api, "[EXTENSION] RULES.md D164: a restored database's log is made as it is opened, never created anew"))
    }
}
