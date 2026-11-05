//! \[EXTENSION\] The driver's part in a state transfer (RULES.md D57,
//! `transfer.rs`): the export it serves, one for every replica fetching,
//! taken as the first request comes, numbered so a fetcher tells its answers
//! from another export's, and let go once no request comes for a while, when
//! the database is deposed, and before it installs a transfer; one, as an
//! export holds the database's checkpoints off, which a second one asked of
//! the same thread would wait on, itself (RULES.md R-ERR-4); and the fetch
//! of its own, begun as the consensus asks (`Output::FetchState`), kept
//! while the consensus asks again from the same replica, its answers taken
//! until the export is whole, which the database installs
//! (`Standby::import`).

use std::collections::VecDeque;

use std::path::Path;

use turborel::db::{RECORDS_FILE, Standby, StateSource, decode_records};
use turborel::error::Result;
use turborel::log::Lsn;

use super::{Core, Driver, Effect, misuse};
use crate::transfer::stamp;
use crate::{Input, ReplicaId};

/// \[EXTENSION\] An export served to the replicas fetching it.
pub(super) struct Served {
    source: Box<dyn StateSource>,
    /// The ticks since a request asked of it.
    idle: u64,
    /// Its number, by which a fetcher tells its answers from another's.
    number: u64,
}

/// \[DESIGN\] The ticks an export served waits for the next request before
/// it is let go, so a fetcher gone frees the checkpoints: a few heartbeats.
const SOURCE_IDLE_TICKS: u64 = 20;

impl Driver {
    /// \[EXTENSION\] Answers the state transfer requests pending, and
    /// carries out nothing else: for a host whose database waits on a sync
    /// on the thread the driver turns on, as the simulator's does, which
    /// serves the replicas fetching its state meanwhile, as a node's other
    /// threads would; an export is of the last checkpoint, which no call
    /// in progress changes.
    ///
    /// # Errors
    ///
    /// As `turn`'s, of answering.
    pub fn serve_transfers(&self, db: &dyn Standby) -> Result<()> {
        let answers = {
            let mut core = self.core()?;
            let (mut kept, mut answers) = (VecDeque::new(), Vec::new());
            while let Some(effect) = core.pending.pop_front() {
                match effect {
                    Effect::Answer(from, frame) => answers.push((from, frame)),
                    other @ (Effect::Deliver(_) | Effect::Serve(_) | Effect::Stop | Effect::Install | Effect::Retire) => {
                        kept.push_back(other);
                    }
                }
            }
            core.pending = kept;
            answers
        };
        for (from, frame) in answers {
            self.answer(db, from, &frame)?;
        }
        Ok(())
    }

    /// \[EXTENSION\] Answers `frame`, a state transfer's request from
    /// `from`, out of the database's export, taken as the first request
    /// comes and kept while requests do (RULES.md D57).
    pub(super) fn answer(&self, db: &dyn Standby, from: ReplicaId, frame: &[u8]) -> Result<()> {
        let kept = {
            let mut core = self.core()?;
            core.watch.answering(from);
            core.source.take()
        };
        let mut served = kept.map_or_else(|| self.fresh_export(db), Ok)?;
        let answered = {
            let core = self.core()?;
            core.transfer.serve(&mut *served.source, (from, served.number), frame)
        };
        let mut core = self.core()?;
        served.idle = 0;
        core.source = Some(served);
        if let Some(answer) = answered? {
            core.transport.send(from, &answer)?;
        }
        Ok(())
    }

    /// A fresh export of the database, numbered.
    fn fresh_export(&self, db: &dyn Standby) -> Result<Served> {
        let source = db.export()?;
        let mut core = self.core()?;
        core.exports = core.exports.wrapping_add(1);
        Ok(Served { source, idle: 0, number: stamp().wrapping_add(core.exports) })
    }

    /// \[EXTENSION\] Installs the state transfer fetched: the records the
    /// export carries, with which its checkpoint is recovered, are made
    /// the log after the position the export's database holds it through
    /// first (`Input::Installed`), and made durable (`Journal::flush`), so
    /// the database, which takes the export then, recovers from them
    /// whatever a crash keeps: a journal behind the files has the replica
    /// fetch again, files behind the journal never happen; an export
    /// carrying none, a modelled database's, is taken first, and the
    /// position it then holds the log through told after. A database
    /// serving, which took the log over meanwhile, takes none, nor one
    /// retired, its replica removed meanwhile: the fetch is let go. The
    /// export served is let go first, as the database it is of is
    /// replaced: its fetchers begin again.
    pub(super) fn install(&self, db: &dyn Standby) -> Result<()> {
        let mut fetched = {
            let mut core = self.core()?;
            let Some((_, fetching)) = core.fetching.take() else { return Ok(()) };
            if core.serving.is_some() || core.retired {
                return Ok(());
            }
            core.source = None;
            fetching.fetched()?
        };
        let carried = records_of(&mut *fetched)?;
        if let Some((holds, records)) = carried {
            {
                let mut core = self.core()?;
                core.step(Input::Installed { holds, records })?;
                let durable = core.journal.flush()?;
                for at in durable {
                    core.step(Input::Persisted(at))?;
                }
            }
            let held = db.import(fetched)?;
            if held != holds {
                return Err(misuse(&format!(
                    "a state transfer installed holds the log through {}, its export through {}",
                    held.0, holds.0
                )));
            }
            let mut core = self.core()?;
            core.holds = holds;
            core.watch.installed(holds);
            return Ok(());
        }
        let holds = db.import(fetched)?;
        let mut core = self.core()?;
        core.holds = holds;
        core.watch.installed(holds);
        core.step(Input::Installed { holds, records: Vec::new() })
    }
}

/// The records `source` carries (`RECORDS_FILE`), if it carries any: the
/// position the export's database holds the log through, and the records
/// after it.
fn records_of(source: &mut dyn StateSource) -> Result<Option<(Lsn, Vec<Vec<u8>>)>> {
    let Some(index) = source.files().iter().position(|f| f.within == Path::new(RECORDS_FILE)) else { return Ok(None) };
    let length = source.files().get(index).map_or(0, |f| f.length);
    let mut bytes = Vec::new();
    let mut piece = vec![0; 1 << 16];
    while u64::try_from(bytes.len()).unwrap_or(u64::MAX) < length {
        let read = source.read(index, u64::try_from(bytes.len()).unwrap_or(u64::MAX), &mut piece)?;
        if read == 0 {
            return Err(misuse("an export's records end before their length"));
        }
        bytes.extend_from_slice(piece.get(..read).unwrap_or_default());
    }
    decode_records(&bytes).map(Some)
}

impl Core {
    /// \[EXTENSION\] An answer frame from `from` for the state transfer being
    /// fetched, if one is from it: the fetch done, the transfer is installed.
    pub(super) fn took_answer(&mut self, from: ReplicaId, frame: &[u8]) -> Result<()> {
        let Some((serving, fetching)) = self.fetching.as_mut() else { return Ok(()) };
        if *serving != from {
            return Ok(());
        }
        if fetching.take(frame)? {
            self.pending.push_back(Effect::Install);
        } else if let Some(request) = fetching.request() {
            self.transport.send(from, &request)?;
        }
        Ok(())
    }

    /// \[EXTENSION\] `FetchState`: a fetch begun from `from`, but one from it
    /// under way, which the consensus asking again does not disturb, goes
    /// on; one from another replica is let go, with its install if one is
    /// pending.
    pub(super) fn fetch_from(&mut self, from: ReplicaId) -> Result<()> {
        if self.fetching.as_ref().is_some_and(|(source, _)| *source == from) {
            return Ok(());
        }
        self.pending.retain(|effect| !matches!(effect, Effect::Install));
        self.watch.fetching(from);
        let mut fetching = self.transfer.begin(from)?;
        if let Some(request) = fetching.request() {
            self.transport.send(from, &request)?;
        }
        self.fetching = Some((from, fetching));
        Ok(())
    }

    /// \[EXTENSION\] A tick of a state transfer: a request waited on asked
    /// again, and the export served, if no request asked of it for a
    /// while, let go, so checkpoints go on.
    pub(super) fn tick_transfer(&mut self) -> Result<()> {
        if let Some((from, fetching)) = self.fetching.as_mut() {
            fetching.waited();
            if let Some(request) = fetching.request() {
                self.transport.send(*from, &request)?;
            }
        }
        if let Some(served) = self.source.as_mut() {
            served.idle = served.idle.saturating_add(1);
            if served.idle >= SOURCE_IDLE_TICKS {
                self.source = None;
            }
        }
        Ok(())
    }
}
