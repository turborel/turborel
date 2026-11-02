//! \[EXTENSION\] The driver's log (RULES.md D25, D57): the core `Log` a
//! replica's database writes to and recovers from, over its driver. Its
//! positions are the consensus's: `append` steps the consensus, and
//! `sync(p)` waits (`Wait`) until the consensus commits `p`; a record
//! appended in a service that ended before it was committed is in doubt
//! (RULES.md D25), its sync failing. Its records are the journal's as
//! written, which the database, opened to follow, reads only through its
//! last checkpoint (`Database::open_following`). And `GroupLogs`, the log
//! storage giving the database this log as it is opened from its files:
//! of a driver made before, or, on files, of one made as the database
//! opens, its journal the file log in the directory's `log/`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use turborel::db::{LogStorage, OpenedLog, StorePlace};
use turborel::error::{Code, Result, SqlError};
use turborel::log::{FileLog, FileLogOptions, Log, LogCapabilities, Records};

use super::{Driver, Inbox, Parts, Ticker, Wait, Watch, misuse, position};
use crate::{Consensus, Input, LogJournal, Lsn, Recovered, ReplicaId, Transport};

/// \[EXTENSION\] A replica's log, over its driver: see the module's
/// documentation.
pub struct GroupLog {
    driver: Arc<Driver>,
}

impl GroupLog {
    /// The log of `driver`.
    pub fn of(driver: Arc<Driver>) -> GroupLog {
        GroupLog { driver }
    }
}

impl Driver {
    /// Appends `record`: the position the consensus gives it.
    fn append(&self, record: &[u8]) -> Result<Lsn> {
        let mut core = self.core()?;
        core.assigned = None;
        if core.serving.is_none() {
            return Err(misuse("the log's replica serves as no primary"));
        }
        core.step(Input::Append(record.to_vec()))?;
        let at = core.assigned.take().ok_or_else(|| misuse("the log's replica serves as no primary"))?;
        core.own.insert(at.0, record.to_vec());
        Ok(at)
    }

    /// Whether `through` is committed, or its sync fails; `None` while it
    /// waits.
    fn synced(&self, through: Lsn) -> Option<Result<()>> {
        let core = match self.core() {
            Ok(core) => core,
            Err(e) => return Some(Err(e)),
        };
        if through > core.op() {
            return Some(Err(misuse(&format!("the log has no position {} yet", through.0))));
        }
        if core.committed >= through {
            return Some(Ok(()));
        }
        if core.serving.is_none() || through < core.service_start {
            let why = format!("[EXTENSION] RULES.md D25, D57: the replica stopped serving before position {} was committed", through.0);
            return Some(Err(SqlError::new(Code::Io, why)));
        }
        None
    }
}

impl Log for GroupLog {
    fn capabilities(&self) -> LogCapabilities {
        LogCapabilities { durable: self.driver.durable }
    }

    fn append(&self, record: &[u8]) -> Result<Lsn> {
        self.driver.append(record)
    }

    fn sync(&self, through: Lsn) -> Result<()> {
        loop {
            if let Some(synced) = self.driver.synced(through) {
                return synced;
            }
            self.driver.wait.wait(&self.driver)?;
        }
    }

    /// The records from `from`, or from the first held if `from` was let
    /// go.
    fn read_from(&self, from: Lsn) -> Records<'_> {
        let core = self.driver.core.lock().unwrap_or_else(PoisonError::into_inner);
        let (first, skip) = (core.first, core.index(from).unwrap_or(0));
        let records: Vec<(Lsn, Vec<u8>)> =
            core.records.iter().enumerate().skip(skip).map(|(i, r)| (position(first, i), r.clone())).collect();
        Box::new(records.into_iter().map(Ok))
    }

    fn read_at(&self, at: Lsn) -> Result<Option<Vec<u8>>> {
        let core = self.driver.core.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(core.index(at).and_then(|i| core.records.get(i)).cloned())
    }

    /// \[EXTENSION\] The database holds every record before `before` in its
    /// files: the records before it may go, but the bytes of them the
    /// retention keeps (RULES.md D57); the consensus lets go what is
    /// committed of them, and the journal follows.
    fn truncate_before(&self, before: Lsn) -> Result<()> {
        self.driver.core()?.retain_before(before)
    }

    fn end(&self) -> Lsn {
        self.driver.core.lock().unwrap_or_else(PoisonError::into_inner).end()
    }
}

/// \[EXTENSION\] What makes a replica's consensus: of the group's
/// members, the replica among them, and what its journal held, if it is
/// made again (`Recovered`); none, new.
pub type Make = Arc<dyn Fn(&[ReplicaId], ReplicaId, Option<Recovered>) -> Result<Box<dyn Consensus>> + Send + Sync>;

/// \[EXTENSION\] What a driver on files is joined to beside its consensus
/// and journal (`Parts`): its transport, where its frames arrive, its
/// ticks, how its syncs wait, and what it tells.
pub struct Joined {
    /// What sends its frames.
    pub transport: Arc<dyn Transport>,
    /// Where its frames arrive.
    pub inbox: Arc<Inbox>,
    /// How its time passes.
    pub ticker: Arc<dyn Ticker>,
    /// How a sync of its log waits.
    pub wait: Arc<dyn Wait>,
    /// What it tells of what it does.
    pub watch: Arc<dyn Watch>,
}

/// \[EXTENSION\] The log storage of a replica's database: its log is its
/// driver's (`GroupLog`), kept in the driver's journal. Of a driver given
/// (`of`), the journal is wherever the driver keeps it, and the directory
/// holds no log of its own; on files (`in_files`), the journal is the
/// core's file log in the directory's `log/`, of the database's checksum
/// and encodings, as `ReplicatedLogs` keeps a replica alone's, and the
/// driver is made as the database opens, from what the journal holds:
/// `driver` gives it, to join to the database once opened
/// (`Driver::join`) and run as its job (`DriverJob`).
pub struct GroupLogs {
    driver: Mutex<Option<Arc<Driver>>>,
    files: Option<OnFiles>,
}

/// \[EXTENSION\] How much of the log a driver keeps (RULES.md D57, log
/// retention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    /// The bytes of records before the database's last checkpoint the
    /// journal keeps at least: a backup behind by less catches up from the
    /// log, by more from a state transfer.
    pub bytes: u64,
}

/// What makes a driver on files.
struct OnFiles {
    options: FileLogOptions,
    members: Vec<ReplicaId>,
    me: ReplicaId,
    make: Make,
    joined: Joined,
    retention: Retention,
}

impl GroupLogs {
    /// The log storage giving `driver`'s log.
    pub fn of(driver: Arc<Driver>) -> GroupLogs {
        GroupLogs { driver: Mutex::new(Some(driver)), files: None }
    }

    /// \[EXTENSION\] The log storage of replica `me` of `members`, whose
    /// journal is a file log of `options` in the directory's `log/`, its
    /// consensus what `make` makes, its driver joined to `joined`, keeping
    /// the log as `retention` says.
    pub fn in_files(
        options: FileLogOptions,
        (members, me): (&[ReplicaId], ReplicaId),
        make: Make,
        (joined, retention): (Joined, Retention),
    ) -> GroupLogs {
        let files = OnFiles { options, members: members.to_vec(), me, make, joined, retention };
        GroupLogs { driver: Mutex::new(None), files: Some(files) }
    }

    /// \[EXTENSION\] The driver, once the database is opened.
    pub fn driver(&self) -> Option<Arc<Driver>> {
        self.driver.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The driver over `journal`, kept in a file log on files, made and
    /// kept; of a log storage of a driver given, that driver.
    fn made(&self, journal: Arc<dyn Log>) -> Result<Arc<Driver>> {
        let mut kept = self.driver.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(driver) = &*kept {
            return Ok(Arc::clone(driver));
        }
        let files = self.files.as_ref().ok_or_else(|| misuse("the log storage has no driver"))?;
        let (journal, recovered) = LogJournal::open(journal)?;
        let (first, records) = (recovered.first, recovered.records.clone());
        let again = (recovered != Recovered::default()).then_some(recovered);
        let consensus = (files.make)(&files.members, files.me, again)?;
        let joined = &files.joined;
        let parts = Parts {
            consensus,
            journal: Box::new(journal),
            first,
            records,
            retain_bytes: files.retention.bytes,
            transport: Arc::clone(&joined.transport),
            inbox: Arc::clone(&joined.inbox),
            ticker: Arc::clone(&joined.ticker),
            wait: Arc::clone(&joined.wait),
            watch: Arc::clone(&joined.watch),
        };
        let driver = Driver::new(parts);
        *kept = Some(Arc::clone(&driver));
        Ok(driver)
    }
}

/// The name of `GroupLogs`, for messages and events.
pub const GROUP_LOGS: &str = "group";

/// The directory of the journal at `place`.
fn journal_dir(place: &StorePlace<'_>) -> PathBuf {
    place.dir.join("log")
}

/// `log`, telling the events of `place`, if it has any.
fn told(log: FileLog, place: &StorePlace<'_>) -> Arc<dyn Log> {
    Arc::new(match place.events {
        Some(events) => log.telling(Arc::clone(events)),
        None => log,
    })
}

impl LogStorage for GroupLogs {
    fn name(&self) -> &'static str {
        GROUP_LOGS
    }

    fn open(&self, place: &StorePlace<'_>) -> Result<Option<OpenedLog>> {
        if let Some(driver) = self.driver() {
            return Ok(Some(OpenedLog { log: driver.log(), cut: 0 }));
        }
        let Some(files) = &self.files else { return Ok(None) };
        let dir = journal_dir(place);
        if !FileLog::exists(&**place.fs, &dir)? {
            return Ok(None);
        }
        // `place.through` counts in the group's positions, not the
        // journal's: every segment is checked.
        let (journal, recovery) =
            FileLog::open(Arc::clone(place.fs), &dir, &place.formats.hashes, &place.formats.encodings, files.options.clone())?;
        let driver = self.made(told(journal, place))?;
        Ok(Some(OpenedLog { log: driver.log(), cut: recovery.cut }))
    }

    /// On files, whether the directory holds the journal; of a driver
    /// given, none: the journal is the driver's.
    fn exists(&self, place: &StorePlace<'_>) -> Result<bool> {
        if self.files.is_none() || self.driver().is_some() {
            return Ok(false);
        }
        FileLog::exists(&**place.fs, &journal_dir(place))
    }

    fn create(&self, place: &StorePlace<'_>) -> Result<Arc<dyn Log>> {
        if let Some(driver) = self.driver() {
            return Ok(driver.log());
        }
        let files = self.files.as_ref().ok_or_else(|| misuse("the log storage has no driver"))?;
        let options = FileLogOptions { pipeline: place.formats.pipeline.clone(), ..files.options.clone() };
        let journal = FileLog::create(Arc::clone(place.fs), &journal_dir(place), Arc::clone(&place.formats.hash), options)?;
        Ok(self.made(told(journal, place))?.log())
    }
}
