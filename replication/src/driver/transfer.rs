//! \[EXTENSION\] The driver's part in a state transfer (RULES.md D57,
//! `transfer.rs`): the exports it serves, one to each replica fetching,
//! taken as the first request comes, numbered so the fetcher tells its
//! answers from another export's, and let go once no request comes for
//! a while, when the database is deposed, and before it installs a
//! transfer; and the fetch of its own, begun as the consensus asks
//! (`Output::FetchState`), kept while the consensus asks again from the
//! same replica, its answers taken until the export is whole, which the
//! database installs (`Standby::import`). Split from driver.rs (RULES.md
//! R-SIZE-2).

use turborel::db::{Standby, StateSource};
use turborel::error::Result;

use super::{Core, Driver, Effect, misuse};
use crate::transfer::stamp;
use crate::{Input, ReplicaId};

/// \[EXTENSION\] An export served to a replica fetching it.
pub(super) struct Served {
    source: Box<dyn StateSource>,
    /// The ticks since a request asked of it.
    idle: u64,
    /// Its number, by which the fetcher tells its answers from another's.
    number: u64,
}

/// \[DESIGN\] The ticks an export served waits for the next request before
/// it is let go, so a fetcher gone frees the checkpoints: a few heartbeats.
const SOURCE_IDLE_TICKS: u64 = 20;

impl Driver {
    /// \[EXTENSION\] Answers `frame`, a state transfer's request from
    /// `from`, out of the database's export, taken as the first request
    /// comes and kept while requests do (RULES.md D57).
    pub(super) fn answer(&self, db: &dyn Standby, from: ReplicaId, frame: &[u8]) -> Result<()> {
        let kept = self.core()?.sources.remove(&from);
        let mut served = kept.map_or_else(|| self.fresh_export(db), Ok)?;
        let answered = {
            let core = self.core()?;
            core.transfer.serve(&mut *served.source, (from, served.number), frame)
        };
        let mut core = self.core()?;
        served.idle = 0;
        core.sources.insert(from, served);
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

    /// \[EXTENSION\] Installs the state transfer fetched: the database
    /// takes it, and the position it then holds the log through is told to
    /// the consensus (`Input::Installed`); but a database serving, which
    /// took the log over meanwhile, takes none: the fetch is let go. The
    /// exports served are let go first, as the database they are of is
    /// replaced: their fetchers begin again.
    pub(super) fn install(&self, db: &dyn Standby) -> Result<()> {
        let fetched = {
            let mut core = self.core()?;
            let Some((_, fetching)) = core.fetching.take() else { return Ok(()) };
            if core.serving.is_some() {
                return Ok(());
            }
            core.sources.clear();
            fetching.fetched()?
        };
        let holds = db.import(fetched)?;
        let mut core = self.core()?;
        if holds < core.first {
            return Err(misuse("a state transfer installed from before the log's first position"));
        }
        core.holds = holds;
        core.watch.installed(holds);
        core.step(Input::Installed(holds))
    }
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
        let mut fetching = self.transfer.begin(from)?;
        if let Some(request) = fetching.request() {
            self.transport.send(from, &request)?;
        }
        self.fetching = Some((from, fetching));
        Ok(())
    }

    /// \[EXTENSION\] A tick of a state transfer: a request waited on asked
    /// again, and an export served that no request asked of for a while
    /// let go, so checkpoints go on.
    pub(super) fn tick_transfer(&mut self) -> Result<()> {
        if let Some((from, fetching)) = self.fetching.as_mut() {
            fetching.waited();
            if let Some(request) = fetching.request() {
                self.transport.send(*from, &request)?;
            }
        }
        for served in self.sources.values_mut() {
            served.idle = served.idle.saturating_add(1);
        }
        self.sources.retain(|_, served| served.idle < SOURCE_IDLE_TICKS);
        Ok(())
    }
}
