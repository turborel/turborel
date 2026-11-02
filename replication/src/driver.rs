//! \[EXTENSION\] The driver of a replica (RULES.md D55, D57): content-blind,
//! it joins a [`Consensus`], a [`Journal`], a [`Transport`] and the core's
//! `Standby` seam of the replica's database, carrying out the consensus's
//! outputs in order as the crate's contract says.
//!
//! It is a struct stepped without threads: a turn (`Driver::turn`) takes
//! the frames arrived (`Inbox`), the ticks due (`Ticker`) and the journal
//! writes made durable, steps the consensus with each, and then carries
//! out what it asked of the database, in order: records delivered,
//! replayed; a role, the database promoted over the driver's log
//! (`GroupLog`), or deposed and opened again in place, the position it
//! then holds the log through told back (`Input::Opened`). A job of the
//! thread manager runs the turns (`DriverJob`, `turborel-replica`); the
//! simulator of `turborel-test-support` calls the same turn.
//!
//! The database's own calls come through its log: an append steps the
//! consensus, and a sync waits (`Wait`) until the consensus commits the
//! position, or the service it was appended in ends, the commit then in
//! doubt (RULES.md D25). What the database is asked to do is carried out
//! with no lock held, as it calls back through its log (RULES.md
//! R-ERR-4); a thread carrying it out that waits on a sync, as a
//! promotion syncing its abort records does, moves the inputs on itself.

mod job;
mod log;
mod seams;

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use turborel::db::Standby;
use turborel::error::{Code, Result, SqlError};

pub use self::job::{DriverJob, JOB_LABEL};
pub use self::log::{GroupLog, GroupLogs, Joined, Make, Retention};
pub use self::seams::{Blocking, Inbox, Ticker, Ticks, Unwatched, Wait, Watch};
use crate::{Consensus, Input, Journal, Lsn, Output, Role, Transport};

/// \[EXTENSION\] What a driver is made of.
pub struct Parts {
    /// The replica's consensus, new or made again from its journal.
    pub consensus: Box<dyn Consensus>,
    /// Its journal.
    pub journal: Box<dyn Journal>,
    /// The position before the first record its journal holds, those
    /// through it let go by retention (`Recovered::first`).
    pub first: Lsn,
    /// The records its journal holds, from the position after `first` on.
    pub records: Vec<Vec<u8>>,
    /// \[EXTENSION\] How many bytes of records before the database's last
    /// checkpoint the journal keeps at least (RULES.md D57, log retention):
    /// a backup behind by less catches up from the log, by more from a
    /// state transfer.
    pub retain_bytes: u64,
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

/// What the database is to do, as the consensus asked, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    /// Replay the records committed through this position it lacks.
    Deliver(Lsn),
    /// Serve as primary of the view.
    Serve(u64),
    /// Stop serving: deposed, and opened again.
    Stop,
}

/// \[EXTENSION\] A replica's driver: see the module's documentation.
pub struct Driver {
    core: Mutex<Core>,
    /// Signalled as the driver moves on.
    changed: Condvar,
    inbox: Arc<Inbox>,
    ticker: Arc<dyn Ticker>,
    wait: Arc<dyn Wait>,
    /// The thread carrying out what the database is asked, while one is.
    applying: Mutex<Option<ThreadId>>,
    durable: bool,
}

/// The consensus, its journal and transport, and what the driver keeps.
struct Core {
    consensus: Box<dyn Consensus>,
    journal: Box<dyn Journal>,
    transport: Arc<dyn Transport>,
    watch: Arc<dyn Watch>,
    /// The position before the first record of the log held: those through
    /// it let go by retention.
    first: Lsn,
    /// The log: the record at position n at index n − first − 1.
    records: Vec<Vec<u8>>,
    /// The bytes of records kept before a checkpoint at least.
    retain_bytes: u64,
    /// Through where the consensus said the log is committed, serving.
    committed: Lsn,
    /// The view the consensus serves as primary of, if it does.
    serving: Option<u64>,
    /// The first position of its service: a sync of one before, not
    /// committed, is of a service ended.
    service_start: Lsn,
    /// Whether the database serves: promoted in this service.
    promoted: bool,
    /// The records it appended in its service, not yet committed.
    own: BTreeMap<u64, Vec<u8>>,
    /// Those of a service ended, in doubt until an outcome.
    doubted: BTreeMap<u64, Vec<u8>>,
    pending: VecDeque<Effect>,
    /// Through where the database holds the log.
    holds: Lsn,
    /// The position the last append was given, if one was.
    assigned: Option<Lsn>,
    /// Its failure, after which it takes nothing: the consensus counted on
    /// what failed.
    failed: Option<SqlError>,
}

/// An error of the driver's own (SQLCODE -910).
fn misuse(why: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[EXTENSION] RULES.md D57: {why}"))
}

impl Driver {
    /// \[EXTENSION\] A driver of `parts`, not yet joined to its database.
    pub fn new(parts: Parts) -> Arc<Driver> {
        let durable = parts.journal.durable();
        let core = Core {
            consensus: parts.consensus,
            journal: parts.journal,
            transport: parts.transport,
            watch: parts.watch,
            first: parts.first,
            records: parts.records,
            retain_bytes: parts.retain_bytes,
            committed: Lsn(0),
            serving: None,
            service_start: Lsn(0),
            promoted: false,
            own: BTreeMap::new(),
            doubted: BTreeMap::new(),
            pending: VecDeque::new(),
            holds: Lsn(0),
            assigned: None,
            failed: None,
        };
        let applying = Mutex::new(None);
        Arc::new(Driver {
            core: Mutex::new(core),
            changed: Condvar::new(),
            inbox: parts.inbox,
            ticker: parts.ticker,
            wait: parts.wait,
            applying,
            durable,
        })
    }

    /// \[EXTENSION\] Joins its database, opened from its files, which holds
    /// the log through `holds` (`Database::open_following`).
    ///
    /// # Errors
    ///
    /// The driver's failure, or its journal's.
    pub fn join(&self, holds: Lsn) -> Result<()> {
        let mut core = self.core()?;
        core.holds = holds;
        core.step(Input::Opened(holds))?;
        drop(core);
        self.changed.notify_all();
        Ok(())
    }

    /// \[EXTENSION\] One turn: the inputs moved on (`step_inputs`), then what
    /// the database `db` is asked carried out, in order, unless this thread
    /// carries it out already, from within it.
    ///
    /// # Errors
    ///
    /// The driver's failure: its journal's or transport's, or the
    /// database's replay, promotion while it serves, or opening again; the
    /// driver then takes nothing.
    pub fn turn(self: &Arc<Self>, db: &dyn Standby) -> Result<()> {
        self.step_inputs()?;
        let me = thread::current().id();
        {
            let mut applying = self.applying.lock().unwrap_or_else(PoisonError::into_inner);
            if applying.is_some() {
                return Ok(());
            }
            *applying = Some(me);
        }
        let applied = self.apply(db);
        *self.applying.lock().unwrap_or_else(PoisonError::into_inner) = None;
        if let Err(e) = &applied {
            self.halt(e.clone());
        }
        applied
    }

    /// \[EXTENSION\] The frames arrived, the ticks due and the journal's
    /// writes made durable, each stepping the consensus, its outputs
    /// carried out but what the database is asked, which waits for a turn.
    ///
    /// # Errors
    ///
    /// The driver's failure, or its journal's or transport's.
    pub fn step_inputs(&self) -> Result<()> {
        let mut core = self.core()?;
        for (from, frame) in self.inbox.take() {
            core.step(Input::Received { from, frame })?;
        }
        for _ in 0..self.ticker.take() {
            core.step(Input::Tick)?;
        }
        core.answer()?;
        drop(core);
        self.changed.notify_all();
        Ok(())
    }

    /// Carries out what the database is asked, in order, until nothing is.
    fn apply(self: &Arc<Self>, db: &dyn Standby) -> Result<()> {
        loop {
            let Some(effect) = self.core()?.pending.pop_front() else { return Ok(()) };
            match effect {
                Effect::Deliver(through) => self.deliver(db, through)?,
                Effect::Serve(view) => self.serve(db, view)?,
                Effect::Stop => self.stop(db)?,
            }
            self.changed.notify_all();
        }
    }

    /// The records committed through `through` the database lacks,
    /// replayed.
    fn deliver(&self, db: &dyn Standby, through: Lsn) -> Result<()> {
        let records: Vec<(Lsn, Vec<u8>)> = {
            let core = self.core()?;
            let end = core.index(through).map_or(0, |n| n.saturating_add(1)).min(core.records.len());
            let from = core.index(core.holds.max(core.first)).map_or(0, |n| n.saturating_add(1)).min(end);
            let first = core.first;
            core.records
                .get(from..end)
                .unwrap_or_default()
                .iter()
                .enumerate()
                .map(|(i, r)| (position(first, from.saturating_add(i)), r.clone()))
                .collect()
        };
        let Some(&(last, _)) = records.last() else { return Ok(()) };
        db.replay(&mut records.into_iter().map(Ok))?;
        let mut core = self.core()?;
        core.holds = last;
        core.step(Input::Applied(last))
    }

    /// The database promoted, serving `view`, once it holds the whole log;
    /// a promotion that fails as the service ends meanwhile is let go.
    fn serve(self: &Arc<Self>, db: &dyn Standby, view: u64) -> Result<()> {
        {
            let core = self.core()?;
            if core.serving != Some(view) {
                return Ok(());
            }
            if core.holds != core.op() {
                return Err(misuse("a primary serves once its database holds the log it took over"));
            }
        }
        match db.promote(self.log()) {
            Ok(()) => {
                let core = self.core()?;
                if core.serving == Some(view) {
                    core.watch.serving(view);
                    let mut core = core;
                    core.promoted = true;
                }
                Ok(())
            }
            Err(e) => {
                let core = self.core()?;
                if core.serving == Some(view) { Err(e) } else { Ok(()) }
            }
        }
    }

    /// The database deposed and opened again in place, the position it then
    /// holds the log through told to the consensus.
    fn stop(&self, db: &dyn Standby) -> Result<()> {
        db.depose()?;
        let holds = db.reopen()?;
        let mut core = self.core()?;
        core.promoted = false;
        core.watch.stopped();
        core.holds = holds;
        core.step(Input::Opened(holds))
    }

    /// \[EXTENSION\] Stops the driver: it takes nothing from now on, failing
    /// with `why`, as its replica stopped.
    pub fn halt(&self, why: SqlError) {
        let mut core = self.core.lock().unwrap_or_else(PoisonError::into_inner);
        core.failed.get_or_insert(why);
        core.serving = None;
        drop(core);
        self.changed.notify_all();
    }

    /// \[EXTENSION\] Its failure, if it failed.
    pub fn failed(&self) -> Option<SqlError> {
        self.core.lock().unwrap_or_else(PoisonError::into_inner).failed.clone()
    }

    /// \[EXTENSION\] The view its database serves as primary of, promoted,
    /// if it does.
    pub fn serving(&self) -> Option<u64> {
        let core = self.core.lock().unwrap_or_else(PoisonError::into_inner);
        core.serving.filter(|_| core.promoted && core.failed.is_none())
    }

    /// \[EXTENSION\] Through where its database holds the log.
    pub fn holds(&self) -> Lsn {
        self.core.lock().unwrap_or_else(PoisonError::into_inner).holds
    }

    /// \[EXTENSION\] The log its database writes to, through this driver.
    pub fn log(self: &Arc<Self>) -> Arc<dyn turborel::log::Log> {
        Arc::new(GroupLog::of(Arc::clone(self)))
    }

    /// Its inbox.
    pub(crate) fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    /// Whether this thread carries out what the database is asked.
    pub(crate) fn applying_here(&self) -> bool {
        *self.applying.lock().unwrap_or_else(PoisonError::into_inner) == Some(thread::current().id())
    }

    /// Waits up to `most` for the driver to move on.
    pub(crate) fn wait_changed(&self, most: Duration) {
        let core = self.core.lock().unwrap_or_else(PoisonError::into_inner);
        drop(self.changed.wait_timeout(core, most).unwrap_or_else(PoisonError::into_inner));
    }

    /// The core, locked, unless the driver failed. Each step leaves it
    /// whole, so a lock poisoned by a panicking thread still guards it.
    fn core(&self) -> Result<MutexGuard<'_, Core>> {
        let core = self.core.lock().unwrap_or_else(PoisonError::into_inner);
        core.failed.clone().map_or(Ok(core), Err)
    }
}

impl Core {
    /// Steps the consensus with `input`, and carries out what it answers;
    /// a failure kept, after which the core takes nothing.
    fn step(&mut self, input: Input) -> Result<()> {
        let mut out = Vec::new();
        self.consensus.step(input, &mut out);
        for output in out {
            if let Err(e) = self.carry(output) {
                self.failed = Some(e.clone());
                return Err(e);
            }
        }
        Ok(())
    }

    /// Carries out `output`.
    fn carry(&mut self, output: Output) -> Result<()> {
        match output {
            Output::Assigned(at) => self.assigned = Some(at),
            Output::Persist { at, record } => {
                self.journal.persist(at, &record)?;
                let kept = self.index(at).ok_or_else(|| misuse("a record persisted at a position let go"))?;
                self.records.truncate(kept);
                self.records.push(record);
            }
            Output::Truncate { after } => {
                self.journal.truncate(after)?;
                self.records.truncate(self.index(after).map_or(0, |n| n.saturating_add(1)));
            }
            Output::Retain { through } => {
                self.journal.retain(through)?;
                if through > self.first {
                    let gone = self.index(through).map_or(self.records.len(), |n| n.saturating_add(1)).min(self.records.len());
                    self.records.drain(..gone);
                    self.first = through;
                }
            }
            Output::PersistState(state) => self.journal.persist_state(&state)?,
            Output::Send { to, frame } => self.transport.send(to, &frame)?,
            Output::Committed(through) => self.commit(through),
            Output::Deliver { through } => {
                let held = self.index(through).map_or(0, |n| n.saturating_add(1)).min(self.records.len());
                self.watch.committed((self.first, through), self.records.get(..held).unwrap_or_default());
                self.pending.push_back(Effect::Deliver(through));
            }
            Output::Outcome { at, kept } => {
                let record = self.doubted.remove(&at.0).ok_or_else(|| misuse("an outcome of a record this replica did not append"))?;
                self.watch.outcome(at, &record, kept);
            }
            Output::Role(role) => self.role(role),
            Output::FetchState { from } => {
                return Err(misuse(&format!(
                    "the replica is behind what replica {} retains of the log, so it needs a state transfer, which is replication's next stage: start it again from a copy of the primary's directory",
                    from.0
                )));
            }
            Output::Event(event) => self.watch.event(&event),
        }
        Ok(())
    }

    /// `Committed(through)`: syncs through it answered.
    fn commit(&mut self, through: Lsn) {
        self.committed = self.committed.max(through);
        self.own.retain(|&at, _| at > through.0);
        let held = self.index(through).map_or(0, |n| n.saturating_add(1)).min(self.records.len());
        self.watch.committed((self.first, through), self.records.get(..held).unwrap_or_default());
    }

    /// `Role`: a service begun, the database to be promoted; or ended, its
    /// records not committed in doubt, the database to be deposed.
    fn role(&mut self, role: Role) {
        match role {
            Role::Primary { view } => {
                self.serving = Some(view);
                self.service_start = position(self.first, self.records.len());
                self.committed = self.committed.max(Lsn(self.service_start.0.saturating_sub(1)));
                self.pending.push_back(Effect::Serve(view));
            }
            Role::Backup => {
                self.serving = None;
                self.doubted.append(&mut self.own);
                self.pending.push_back(Effect::Stop);
            }
        }
    }

    /// Makes the journal durable, and steps the consensus with a
    /// `Persisted` for each record it made so, until none is owed.
    fn answer(&mut self) -> Result<()> {
        loop {
            let durable = match self.journal.sync() {
                Ok(durable) => durable,
                Err(e) => {
                    self.failed = Some(e.clone());
                    return Err(e);
                }
            };
            if durable.is_empty() {
                return Ok(());
            }
            for at in durable {
                self.step(Input::Persisted(at))?;
            }
        }
    }
}

impl Core {
    /// The index among the records held of the record at `at`: none if
    /// `at` is let go, or is the position after the last, the index after.
    fn index(&self, at: Lsn) -> Option<usize> {
        usize::try_from(at.0.checked_sub(self.first.0)?.checked_sub(1)?).ok()
    }

    /// The op-number: the last position held.
    fn op(&self) -> Lsn {
        Lsn(self.first.0.saturating_add(u64::try_from(self.records.len()).unwrap_or(u64::MAX)))
    }

    /// The position after the last held: a log's end.
    fn end(&self) -> Lsn {
        position(self.first, self.records.len())
    }

    /// \[EXTENSION\] The database holds every record before `before` in its
    /// files: the log through the position that leaves `retain_bytes` of
    /// records before `before` may go (RULES.md D57).
    fn retain_before(&mut self, before: Lsn) -> Result<()> {
        let mut through = before.0.saturating_sub(1);
        let mut kept = 0_u64;
        while through > self.first.0 {
            let size = self.index(Lsn(through)).and_then(|i| self.records.get(i)).map_or(0, Vec::len);
            kept = kept.saturating_add(u64::try_from(size).unwrap_or(u64::MAX));
            if kept > self.retain_bytes {
                break;
            }
            through = through.saturating_sub(1);
        }
        if through <= self.first.0 {
            return Ok(());
        }
        self.step(Input::Retain(Lsn(through)))
    }
}

/// The position of the record at `index` among the records held after
/// `first`.
fn position(first: Lsn, index: usize) -> Lsn {
    Lsn(first.0.saturating_add(u64::try_from(index).unwrap_or(u64::MAX)).saturating_add(1))
}

impl std::fmt::Debug for Driver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Driver").field("durable", &self.durable).finish_non_exhaustive()
    }
}
