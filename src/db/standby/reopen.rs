//! \[DESIGN\] A replica's database opened from its files to follow, and
//! opened again in place once deposed (RULES.md D57). Its files' log is
//! the replica's, whose records after the last checkpoint may not be
//! committed: a view change may yet replace them. So it recovers from its
//! last checkpoint alone ([VRR §5.1]: "we need to know exactly what log
//! prefix is captured on disk"): the records before the checkpoint's
//! position, and the entries of versions awaiting collection the
//! checkpoint logged after it (`Change::Stale`); a transaction open there
//! is undone, as recovery undoes one. It then follows, from the first
//! record of the oldest transaction open at the checkpoint: given again,
//! the records before the checkpoint's position are applied already, but
//! those of the transactions it holds again (`db/standby.rs`).
//!
//! Opened again in place, it is the same handle, its jobs, settings,
//! sinks and the system tables registered since kept: its state made
//! again as it was before it was first opened from its files, and its
//! files opened again into it. A session begun before is refused from
//! then on (`database/service.rs`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::super::files::{Mode, OpenOptions};
use super::super::formats::FileFormats;
use super::super::image::Image;
use super::super::recovery::{Definitions, Touched};
use super::super::stores::StorePlace;
use super::super::{Database, RecoveryReport, State, api};
use crate::disk::encoding::Pipeline;
use crate::disk::io::FileSystem;
use crate::error::Result;
use crate::log::record::{Change, Record};
use crate::log::{Log, Lsn, sealed};
use crate::sync::{lock, lock_builtin};
use crate::txn::{Following, Held, Transactions};

/// \[DESIGN\] How a database was first opened from its files: its file
/// manager, its directory, its options, and its state before, from which
/// it is opened again in place.
pub(in crate::db) struct Opening {
    pub(in crate::db) fs: Arc<dyn FileSystem>,
    pub(in crate::db) dir: PathBuf,
    pub(in crate::db) options: OpenOptions,
    pub(in crate::db) state: Arc<State>,
}

impl Database {
    /// \[DESIGN\] Opens the database kept in `dir` of `fs` into this one, as
    /// `open_files` does, to follow the log a group of replicas keeps, its
    /// files' log being its replica's (RULES.md D57): recovered from its
    /// last checkpoint alone, see the module's documentation. Through
    /// where it holds the log: the records after, once committed, are
    /// given it (`Standby::replay`).
    ///
    /// # Errors
    ///
    /// As `open_files`.
    pub fn open_following(&self, fs: &Arc<dyn FileSystem>, dir: &Path, options: &OpenOptions) -> Result<Lsn> {
        if self.log_lost(fs, dir)? {
            super::import::clear_state_files(&**fs, dir)?;
        }
        self.open_files_to(fs, dir, options, Mode::Follow)?;
        self.holds_through()
    }

    /// \[DESIGN\] Whether the files in `dir` hold a checkpoint while the
    /// log its replica keeps holds nothing: the replica's journal was
    /// lost, and with it the records the checkpoint is recovered with, so
    /// the files are of no use; the database is made again from nothing,
    /// and takes the group's state (RULES.md D57). Only a database kept
    /// by a group, whose log storage is set, is so.
    fn log_lost(&self, fs: &Arc<dyn FileSystem>, dir: &Path) -> Result<bool> {
        let Some(logs) = self.log_storage_set() else { return Ok(false) };
        let snapshot = self.snapshot()?;
        let reading = FileFormats {
            hash: snapshot.checksum_function(&snapshot.settings)?,
            pipeline: Pipeline::none(),
            hashes: snapshot.hashes.clone(),
            encodings: snapshot.encodings.all()?,
        };
        let place = StorePlace::new(fs, dir, &reading);
        let Some(catalog) = self.catalog_storage().open(&place)? else { return Ok(false) };
        let Some(last) = catalog.last() else { return Ok(false) };
        let image = Image::decode(&last, &snapshot.types)?;
        if image.through == Lsn(0) {
            return Ok(false);
        }
        let Some(log) = logs.open(&place)? else { return Ok(false) };
        // The position after the last record is 1 of a log holding none,
        // never let go of any.
        Ok(log.log.end() <= Lsn(1))
    }

    /// Once opened as `mode` says, from a checkpoint holding every record
    /// before `through`, the oldest transaction open there beginning at
    /// `start`: to follow, it follows from `start`.
    pub(in crate::db) fn follow_if(&self, mode: Mode, (through, start): (Lsn, Lsn)) -> Result<()> {
        if mode == Mode::Follow {
            let tx = Arc::clone(&self.snapshot()?.tx);
            *lock_builtin(&tx.following) = Following { image: through, replayed: Lsn(start.0.saturating_sub(1)) };
            tx.hold(Some(Held::Following));
        }
        Ok(())
    }

    /// Through where it holds the log it follows.
    pub(in crate::db) fn holds_through(&self) -> Result<Lsn> {
        Ok(lock_builtin(&self.snapshot()?.tx.following).replayed)
    }

    /// Recovery from the checkpoint holding every record before `through`
    /// alone: `log` read from `from`, its records before `through`, and the
    /// entries the checkpoint logged after it, up to the checkpoint record
    /// that ends it.
    pub(in crate::db) fn replay_to_image(&self, log: &dyn Log, from: Lsn, through: Lsn) -> Result<(RecoveryReport, Touched)> {
        let (encodings, types) = self.openers()?;
        let opened = log.read_from(from).map(|read| read.and_then(|(at, bytes)| Ok((at, sealed::open(&bytes, &encodings, &types)?))));
        let mut kept = opened.take_while(|read| !ends_image(read, through)).filter(|read| of_image(read, through));
        self.replay_records(&mut kept, Definitions::From(through))
    }

    /// Opens the database, deposed or following, again in place: see the
    /// module's documentation. Through where it then holds the log.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was not opened from its files, or serves; as
    /// `open_following`.
    pub(in crate::db) fn reopen_in_place(&self) -> Result<Lsn> {
        let opening = self.opening()?.ok_or_else(|| api("[DESIGN] RULES.md D57: only a database opened from its files is opened again"))?;
        let files = self.files()?.ok_or_else(|| api("[DESIGN] RULES.md D57: only a database opened from its files is opened again"))?;
        let current = self.snapshot()?;
        if current.tx.held().is_none() {
            return Err(api("[DESIGN] RULES.md D57: a database serving is deposed before it is opened again"));
        }
        let retired = current.tx.held() == Some(Held::Retired);
        {
            // No checkpoint nor maintenance of the service ending meanwhile.
            let _checkpoint = files.checkpointer.taking.take("a checkpoint")?;
            let _maintenance = self.background().maintenance_turn.take("maintenance")?;
            lock(&files.held)?.take();
            let fresh = State {
                generation: current.generation.saturating_add(1),
                tx: Arc::new(Transactions::default()),
                // Its counters as the log replayed anew sets them.
                sequences: Arc::default(),
                // The system tables registered since it was first opened
                // (a replica's REPLICAS) are the handle's, kept.
                system: current.system.clone(),
                ..(*opening.state).clone()
            };
            drop(current);
            self.restart_service(fresh)?;
        }
        self.open_managed(&opening.fs, &opening.dir, &opening.options, Mode::Follow)?;
        self.stay_retired(retired)?;
        self.holds_through()
    }

    /// Retired before, the database is retired still: its replica was
    /// removed from its group, and it serves nothing until the server is
    /// stopped, opened again or not (RULES.md D57).
    pub(in crate::db) fn stay_retired(&self, retired: bool) -> Result<()> {
        if retired {
            self.snapshot()?.tx.hold(Some(Held::Retired));
        }
        Ok(())
    }
}

/// Whether `read` is the checkpoint record ending the checkpoint holding
/// every record before `through`, or one after it.
fn ends_image(read: &Result<(Lsn, Record)>, through: Lsn) -> bool {
    let Ok((at, record)) = read else { return false };
    *at >= through && matches!(record.change, Change::Checkpoint)
}

/// Whether recovery from the checkpoint holding every record before
/// `through` reads `read`: a record before it, an entry it logged, or an
/// error.
fn of_image(read: &Result<(Lsn, Record)>, through: Lsn) -> bool {
    let Ok((at, record)) = read else { return true };
    if *at < through {
        return true;
    }
    if let Change::Stale { through: named, .. } = &record.change {
        return *named == through;
    }
    false
}
