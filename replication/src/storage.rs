//! \[EXTENSION\] The replicated log as the log of a database kept in a
//! directory (RULES.md D25, D57): a `LogStorage`, given to the database
//! before it is opened from its files (`Database::set_log_storage`). Its
//! journal is kept in the core's file log, in the directory's `log/`, of
//! the database's checksum and encodings, so its files are of the kind the
//! file manager knows as the log's (`files/classes.rs`), carried by the
//! log as the database's own log is.

use std::path::PathBuf;
use std::sync::Arc;

use turborel::db::{LogStorage, OpenedLog, StorePlace};
use turborel::error::Result;
use turborel::log::{FileLog, FileLogOptions, Log};

use crate::{MakeAlone, ReplicaId, ReplicatedLog, Vr, VrOptions};

/// \[EXTENSION\] The one replica of a group of one: "The primary is chosen
/// round-robin, starting with replica 1" [VRR §4].
pub const ALONE: ReplicaId = ReplicaId(1);

/// \[EXTENSION\] The name of `ReplicatedLogs`, for messages and events.
pub const STORAGE_NAME: &str = "replicated";

/// \[EXTENSION\] Replicated logs of one replica: their journals in file
/// logs of the options they are made with, their consensus what they are
/// given to make.
pub struct ReplicatedLogs {
    options: FileLogOptions,
    make: MakeAlone,
}

impl ReplicatedLogs {
    /// \[EXTENSION\] Logs whose journals are file logs of `options`, of the
    /// consensus `make` makes.
    pub fn new(options: FileLogOptions, make: MakeAlone) -> ReplicatedLogs {
        ReplicatedLogs { options, make }
    }

    /// \[EXTENSION\] Logs of Viewstamped Replication of `vr`, each a group
    /// of one, `ALONE`, their journals file logs of `options`.
    pub fn vr(options: FileLogOptions, vr: VrOptions) -> ReplicatedLogs {
        ReplicatedLogs::new(options, vr_alone(vr))
    }
}

/// \[EXTENSION\] Makes VR of `options` for a replica alone, `ALONE`.
pub fn vr_alone(options: VrOptions) -> MakeAlone {
    Arc::new(move |recovered| {
        let vr = match recovered {
            None => Vr::new(&[ALONE], ALONE, options)?,
            Some(recovered) => Vr::restart(&[ALONE], ALONE, options, recovered)?,
        };
        Ok(Box::new(vr))
    })
}

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

impl LogStorage for ReplicatedLogs {
    fn name(&self) -> &'static str {
        STORAGE_NAME
    }

    fn exists(&self, place: &StorePlace<'_>) -> Result<bool> {
        FileLog::exists(&**place.fs, &journal_dir(place))
    }

    fn open(&self, place: &StorePlace<'_>) -> Result<Option<OpenedLog>> {
        let dir = journal_dir(place);
        if !FileLog::exists(&**place.fs, &dir)? {
            return Ok(None);
        }
        // `place.through` counts in the replicated log's positions, not the
        // journal's: every segment is checked.
        let (journal, recovery) =
            FileLog::open(Arc::clone(place.fs), &dir, &place.formats.hashes, &place.formats.encodings, self.options.clone())?;
        let log = ReplicatedLog::open(told(journal, place), &self.make)?;
        Ok(Some(OpenedLog { log: Arc::new(log), cut: recovery.cut }))
    }

    fn create(&self, place: &StorePlace<'_>) -> Result<Arc<dyn Log>> {
        let options = FileLogOptions { pipeline: place.formats.pipeline.clone(), ..self.options.clone() };
        let journal = FileLog::create(Arc::clone(place.fs), &journal_dir(place), Arc::clone(&place.formats.hash), options)?;
        Ok(Arc::new(ReplicatedLog::create(told(journal, place), &self.make)?))
    }
}
