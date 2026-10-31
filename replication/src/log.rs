//! \[EXTENSION\] The replicated log (RULES.md D25, D57): a core `Log`
//! whose records a [`Consensus`] orders, and a [`Journal`] keeps. The
//! database writes to it as to any log: `append` gives the record to the
//! consensus, whose position for it is the log's, and `sync(p)` returns
//! once the consensus has committed `p`, so commit and the write-ahead
//! rule are the core's as they are (RULES.md D25).
//!
//! Of one replica, for now: a group of one, whose quorum is itself ([VRR
//! §2.2]: "the threshold is the largest f such that 2f + 1 is less than or
//! equal to the group size, K, and a quorum is of size K − f", so f = 0
//! and a quorum is 1). It tolerates no failure, below what [VRR §7.1] calls
//! "the minimum group size needed for VR", 3: what it commits survives a
//! crash as its journal does. It drives its consensus itself, on the
//! threads that call it, under one lock: with no other replica, no frame
//! is sent, and a record commits once its own journal holds it durably.
//! The driver that joins replicas, a job of the thread manager (D55), is a
//! later stage.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use turborel::error::{Code, Result, SqlError};
use turborel::log::{Log, LogCapabilities, Records};

use crate::{Consensus, Input, Journal, LogJournal, Lsn, Output, Recovered, ReplicaEvent, Role};

/// \[EXTENSION\] The most ticks a replica alone is stepped, as its log is
/// made, before it serves as primary: a new VR replica serves after one;
/// one made again from its journal after its view change's timeout
/// (`VrOptions::view_timeout_ticks`, by default 10), as it does not serve
/// again in the view it served in.
pub const TICKS_ALONE: u32 = 10_000;

/// \[EXTENSION\] A replicated log: see the module's documentation.
pub struct ReplicatedLog {
    driven: Mutex<Driven>,
    durable: bool,
}

/// The consensus, its journal, and what the log keeps of what they did.
struct Driven {
    consensus: Box<dyn Consensus>,
    journal: Box<dyn Journal>,
    /// The log's records: the one at position n at index n − 1, as the
    /// consensus had them persisted.
    records: Vec<Vec<u8>>,
    /// Through where the consensus said the log is committed.
    committed: Lsn,
    /// Whether the consensus serves as primary, so takes appends.
    serving: bool,
    /// The journal's failure, after which the log takes nothing: the
    /// consensus counted on what the journal could not keep.
    failed: Option<SqlError>,
}

/// What a step of the consensus told the log.
#[derive(Default)]
struct Told {
    /// The position of the record appended, if one was.
    assigned: Option<Lsn>,
    /// Whether an append was refused.
    refused: bool,
}

/// \[EXTENSION\] Makes the consensus of a replica alone: new, for `None`,
/// or again from what its journal held.
pub type MakeAlone = Arc<dyn Fn(Option<Recovered>) -> Result<Box<dyn Consensus>> + Send + Sync>;

impl ReplicatedLog {
    /// \[EXTENSION\] The log of a new replica alone, of the consensus `make`
    /// makes, its journal kept in `log` (`LogJournal`), which holds nothing.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `log` holds an entry; as `alone`.
    pub fn create(log: Arc<dyn Log>, make: &MakeAlone) -> Result<ReplicatedLog> {
        let (journal, recovered) = LogJournal::open(log)?;
        if recovered != Recovered::default() {
            return Err(SqlError::new(Code::Api, "[EXTENSION] RULES.md D57: a new replica's journal holds nothing"));
        }
        ReplicatedLog::alone(make(None)?, Box::new(journal), Vec::new())
    }

    /// \[EXTENSION\] The log of a replica alone made again after a stop,
    /// from its journal, kept in `log`, of the consensus `make` makes.
    ///
    /// # Errors
    ///
    /// `LogJournal::open`'s; `make`'s; as `alone`.
    pub fn open(log: Arc<dyn Log>, make: &MakeAlone) -> Result<ReplicatedLog> {
        let (journal, recovered) = LogJournal::open(log)?;
        let records = recovered.records.clone();
        ReplicatedLog::alone(make(Some(recovered))?, Box::new(journal), records)
    }

    /// \[EXTENSION\] The log of a replica alone, of `consensus`, made new or
    /// again from what `journal` held, whose records are `records`: its
    /// consensus stepped until it serves as primary.
    ///
    /// # Errors
    ///
    /// The journal's; SQLCODE -910 if the consensus does not serve within
    /// `TICKS_ALONE` ticks: it is not of a group of one.
    pub fn alone(consensus: Box<dyn Consensus>, journal: Box<dyn Journal>, records: Vec<Vec<u8>>) -> Result<ReplicatedLog> {
        let durable = journal.durable();
        let mut driven = Driven { consensus, journal, records, committed: Lsn(0), serving: false, failed: None };
        for _ in 0..TICKS_ALONE {
            if driven.serving {
                break;
            }
            driven.step(Input::Tick)?;
            driven.answer()?;
        }
        if !driven.serving {
            return Err(SqlError::new(Code::Api, "[EXTENSION] RULES.md D57: a log of one replica whose consensus never serves alone"));
        }
        Ok(ReplicatedLog { driven: Mutex::new(driven), durable })
    }

    /// The consensus and its journal, the lock taken. Each step leaves them
    /// whole, so a lock poisoned by a panicking thread still guards a log
    /// that its `failed` describes.
    fn driven(&self) -> MutexGuard<'_, Driven> {
        self.driven.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Driven {
    /// Refuses everything once the journal failed.
    fn check(&self) -> Result<()> {
        self.failed.clone().map_or(Ok(()), Err)
    }

    /// Steps the consensus with `input`, and carries out what it answers.
    fn step(&mut self, input: Input) -> Result<Told> {
        let mut out = Vec::new();
        self.consensus.step(input, &mut out);
        let mut told = Told::default();
        for output in out {
            if let Err(e) = self.carry(output, &mut told) {
                self.failed = Some(e.clone());
                return Err(e);
            }
        }
        Ok(told)
    }

    /// Carries out `output`.
    fn carry(&mut self, output: Output, told: &mut Told) -> Result<()> {
        match output {
            Output::Assigned(at) => told.assigned = Some(at),
            Output::Persist { at, record } => {
                self.journal.persist(at, &record)?;
                self.records.truncate(before(at));
                self.records.push(record);
            }
            Output::Truncate { after } => {
                self.journal.truncate(after)?;
                self.records.truncate(usize::try_from(after.0).unwrap_or(usize::MAX));
            }
            Output::PersistState(state) => self.journal.persist_state(&state)?,
            Output::Committed(through) => self.committed = self.committed.max(through),
            Output::Role(Role::Primary { .. }) => {
                // Serving, it has committed every record it holds (the
                // consensus contract), those of a journal recovered too.
                self.serving = true;
                self.committed = self.committed.max(Lsn(u64::try_from(self.records.len()).unwrap_or(u64::MAX)));
            }
            Output::Role(Role::Backup) => self.serving = false,
            Output::Event(ReplicaEvent::AppendRefused) => told.refused = true,
            // Alone: no frame is sent, no record is in doubt, and none is
            // delivered that the database has not: it appended each, or
            // recovered it from the log as it opened.
            Output::Send { .. }
            | Output::Deliver { .. }
            | Output::Outcome { .. }
            | Output::FetchState { .. }
            | Output::Event(ReplicaEvent::ViewChanged { .. })
            | Output::Event(ReplicaEvent::FrameRefused { .. }) => {}
        }
        Ok(())
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

    /// The position after the last record.
    fn end(&self) -> Lsn {
        position(self.records.len())
    }
}

/// How many records of a log come before the one at `at`.
fn before(at: Lsn) -> usize {
    usize::try_from(at.0.saturating_sub(1)).unwrap_or(usize::MAX)
}

/// The position of the record at `index` among a log's records.
fn position(index: usize) -> Lsn {
    Lsn(u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1))
}

impl Log for ReplicatedLog {
    fn capabilities(&self) -> LogCapabilities {
        LogCapabilities { durable: self.durable }
    }

    fn append(&self, record: &[u8]) -> Result<Lsn> {
        let mut driven = self.driven();
        driven.check()?;
        let told = driven.step(Input::Append(record.to_vec()))?;
        let refused = || SqlError::new(Code::Api, "[EXTENSION] RULES.md D57: the log's replica serves as no primary");
        if told.refused {
            return Err(refused());
        }
        told.assigned.ok_or_else(refused)
    }

    fn sync(&self, through: Lsn) -> Result<()> {
        let mut driven = self.driven();
        driven.check()?;
        if through >= driven.end() {
            return Err(SqlError::new(Code::Api, format!("[EXTENSION] the log has no position {} yet", through.0)));
        }
        if driven.committed < through {
            driven.answer()?;
        }
        if driven.committed < through {
            return Err(SqlError::new(Code::Io, format!("[EXTENSION] RULES.md D57: the group has not committed position {}", through.0)));
        }
        Ok(())
    }

    fn read_from(&self, from: Lsn) -> Records<'_> {
        let driven = self.driven();
        let records: Vec<(Lsn, Vec<u8>)> =
            driven.records.iter().enumerate().skip(before(from)).map(|(i, record)| (position(i), record.clone())).collect();
        Box::new(records.into_iter().map(Ok))
    }

    fn read_at(&self, at: Lsn) -> Result<Option<Vec<u8>>> {
        let driven = self.driven();
        Ok(at.0.checked_sub(1).and_then(|i| usize::try_from(i).ok()).and_then(|i| driven.records.get(i)).cloned())
    }

    /// Removes nothing: the consensus keeps its whole log, which a view
    /// change may send (log retention is a later stage).
    fn truncate_before(&self, _before: Lsn) -> Result<()> {
        Ok(())
    }

    fn end(&self) -> Lsn {
        self.driven().end()
    }
}
