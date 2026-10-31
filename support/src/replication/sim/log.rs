//! The log a primary's database writes to in the simulator: a core `Log`
//! over its replica in the [`World`], as the replicated log is over its
//! consensus (RULES.md D57). `append` gives the record to the replica's
//! consensus, whose position is the log's; `sync(p)` moves the world on,
//! moment by moment, until the replica is told `p` is committed, or stops
//! serving: the commit is then in doubt (RULES.md D25), as it is when a
//! real primary is deposed. Of one life of its replica: once the replica
//! crashes, the log refuses everything.

use std::sync::{Arc, Mutex, MutexGuard};

use turborel::error::{Code, Result, SqlError};
use turborel::log::{Log, LogCapabilities, Lsn, Records};

use super::World;

/// Moments past the end of the run's plan within which a sync waiting must
/// have been answered: the faults have long stopped.
const SYNC_GRACE: u64 = 10_000;

/// The log of replica `index` in its life `life`.
pub struct SimLog {
    world: Arc<Mutex<World>>,
    index: usize,
    life: u64,
}

impl SimLog {
    /// The log of replica `index`, as it is now, in `world`.
    pub fn new(world: &Arc<Mutex<World>>, index: usize) -> SimLog {
        let life = world.lock().unwrap().lives(index);
        SimLog { world: Arc::clone(world), index, life }
    }

    /// The world, locked, if its replica is still of this log's life.
    fn world(&self) -> Result<MutexGuard<'_, World>> {
        let world = self.world.lock().unwrap();
        if world.lives(self.index) != self.life {
            return Err(SqlError::new(Code::Io, "[EXTENSION] the simulated replica crashed"));
        }
        Ok(world)
    }
}

impl Log for SimLog {
    /// Not durable: the simulated database is in memory, and lost as its
    /// replica crashes.
    fn capabilities(&self) -> LogCapabilities {
        LogCapabilities { durable: false }
    }

    fn append(&self, record: &[u8]) -> Result<Lsn> {
        let mut world = self.world()?;
        let at = world.append(self.index, record.to_vec()).map_err(|why| SqlError::new(Code::Io, why))?;
        at.ok_or_else(|| SqlError::new(Code::Api, "[EXTENSION] RULES.md D57: the log's replica serves as no primary"))
    }

    fn sync(&self, through: Lsn) -> Result<()> {
        loop {
            let mut world = self.world()?;
            if world.committed(self.index) >= through.0 {
                return Ok(());
            }
            if world.serving(self.index).is_none() {
                return Err(SqlError::new(Code::Io, "[EXTENSION] RULES.md D57: the replica stopped serving"));
            }
            if world.over() && world.now() > world.end() + SYNC_GRACE {
                let why = world.fail(&format!("a sync of position {} was never answered", through.0));
                return Err(SqlError::new(Code::Io, why));
            }
            world.moment().map_err(|why| SqlError::new(Code::Io, why))?;
        }
    }

    fn read_from(&self, from: Lsn) -> Records<'_> {
        let world = self.world.lock().unwrap();
        let skip = usize::try_from(from.0.saturating_sub(1)).unwrap();
        let records: Vec<(Lsn, Vec<u8>)> = world
            .records(self.index)
            .iter()
            .enumerate()
            .skip(skip)
            .map(|(i, record)| (Lsn(u64::try_from(i).unwrap() + 1), record.clone()))
            .collect();
        Box::new(records.into_iter().map(Ok))
    }

    /// Removes nothing: the replica's consensus keeps its whole log.
    fn truncate_before(&self, _before: Lsn) -> Result<()> {
        Ok(())
    }

    fn end(&self) -> Lsn {
        Lsn(u64::try_from(self.world.lock().unwrap().records(self.index).len()).unwrap() + 1)
    }
}
