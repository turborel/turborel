//! \[EXTENSION\] What the driver asks its database, carried out in a turn
//! (RULES.md D57): the records committed delivered, the database
//! promoted to serve a view, or deposed and opened again; a state
//! transfer fetched imported (`driver/transfer.rs`). Split from driver.rs
//! (RULES.md R-SIZE-2).

use std::sync::Arc;

use turborel::db::Standby;
use turborel::error::Result;

use super::{Driver, Effect, misuse, position};
use crate::{Input, Lsn};

impl Driver {
    /// Carries out what the database is asked, in order, until nothing is.
    pub(super) fn apply(self: &Arc<Self>, db: &dyn Standby) -> Result<()> {
        loop {
            let Some(effect) = self.core()?.pending.pop_front() else { return Ok(()) };
            match effect {
                Effect::Deliver(through) => self.deliver(db, through)?,
                Effect::Serve(view) => self.serve(db, view)?,
                Effect::Stop => self.stop(db)?,
                Effect::Answer(from, frame) => self.answer(db, from, &frame)?,
                Effect::Install => self.install(db)?,
                Effect::Retire => {
                    self.core()?.retired = true;
                    db.retire()?;
                }
            }
            self.changed.notify_all();
        }
    }

    /// The records committed through `through` the database lacks,
    /// replayed.
    fn deliver(&self, db: &dyn Standby, through: Lsn) -> Result<()> {
        let records: Vec<(Lsn, Vec<u8>)> = {
            let core = self.core()?;
            if core.holds < core.first {
                return Err(misuse(&format!(
                    "the database holds the log through {}, before the window begins after {}: the records between cannot be given it",
                    core.holds.0, core.first.0
                )));
            }
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
                let (holds, op) = (core.holds.0, core.op().0);
                return Err(misuse(&format!(
                    "a primary serves once its database holds the log it took over: it holds through {holds} of {op}"
                )));
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
    /// holds the log through told to the consensus. The export served is
    /// let go first: the database opened again is not the one it is of,
    /// and a checkpoint waits on it no more; its fetchers begin again.
    fn stop(&self, db: &dyn Standby) -> Result<()> {
        self.core()?.source = None;
        db.depose()?;
        let holds = db.reopen()?;
        let mut core = self.core()?;
        core.promoted = false;
        core.watch.stopped();
        core.holds = holds;
        core.step(Input::Opened(holds))
    }
}
