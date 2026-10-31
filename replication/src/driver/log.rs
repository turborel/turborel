//! \[EXTENSION\] The driver's log (RULES.md D25, D57): the core `Log` a
//! replica's database writes to and recovers from, over its driver. Its
//! positions are the consensus's: `append` steps the consensus, and
//! `sync(p)` waits (`Wait`) until the consensus commits `p`; a record
//! appended in a service that ended before it was committed is in doubt
//! (RULES.md D25), its sync failing. Its records are the journal's as
//! written, which the database, opened to follow, reads only through its
//! last checkpoint (`Database::open_following`). And `GroupLogs`, the log
//! storage giving the database this log as it is opened from its files.

use std::sync::{Arc, PoisonError};

use turborel::db::{LogStorage, OpenedLog, StorePlace};
use turborel::error::{Code, Result, SqlError};
use turborel::log::{Log, LogCapabilities, Records};

use super::{Driver, index, misuse, position};
use crate::{Input, Lsn};

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
        if index(through) > core.records.len() {
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

    fn read_from(&self, from: Lsn) -> Records<'_> {
        let core = self.driver.core.lock().unwrap_or_else(PoisonError::into_inner);
        let skip = index(from).saturating_sub(1);
        let records: Vec<(Lsn, Vec<u8>)> = core.records.iter().enumerate().skip(skip).map(|(i, r)| (position(i), r.clone())).collect();
        Box::new(records.into_iter().map(Ok))
    }

    fn read_at(&self, at: Lsn) -> Result<Option<Vec<u8>>> {
        let core = self.driver.core.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(index(at).checked_sub(1).and_then(|i| core.records.get(i)).cloned())
    }

    /// Removes nothing: the consensus keeps its whole log, which a view
    /// change may send (log retention is a later stage).
    fn truncate_before(&self, _before: Lsn) -> Result<()> {
        Ok(())
    }

    fn end(&self) -> Lsn {
        position(self.driver.core.lock().unwrap_or_else(PoisonError::into_inner).records.len())
    }
}

/// \[EXTENSION\] The log storage of a replica's database: its log is its
/// driver's (`GroupLog`), kept in the driver's journal, not in the
/// database's directory, which so holds no log of its own.
pub struct GroupLogs {
    driver: Arc<Driver>,
}

impl GroupLogs {
    /// The log storage giving `driver`'s log.
    pub fn of(driver: Arc<Driver>) -> GroupLogs {
        GroupLogs { driver }
    }
}

/// The name of `GroupLogs`, for messages and events.
pub const GROUP_LOGS: &str = "group";

impl LogStorage for GroupLogs {
    fn name(&self) -> &'static str {
        GROUP_LOGS
    }

    fn open(&self, _place: &StorePlace<'_>) -> Result<Option<OpenedLog>> {
        Ok(Some(OpenedLog { log: self.driver.log(), cut: 0 }))
    }

    /// None in the directory: the journal is the driver's.
    fn exists(&self, _place: &StorePlace<'_>) -> Result<bool> {
        Ok(false)
    }

    fn create(&self, _place: &StorePlace<'_>) -> Result<Arc<dyn Log>> {
        Ok(self.driver.log())
    }
}
