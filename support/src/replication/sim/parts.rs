//! The parts of a simulated replica its driver is made of, as the world
//! keeps them (`sim/world.rs`): its disk, its journal on it, its ticks,
//! and its watch, through which the world checks what the driver carries
//! out. Split from world.rs (RULES.md R-SIZE-2).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use turborel::error::{Code, Result, SqlError};
use turborel_replication::driver::{Ticker, Watch};
use turborel_replication::{Configuration, Lsn, Mark, ReplicaEvent, ReplicaId};

use super::super::Journal;
use super::world::World;
use super::{Rng, Violation};

/// A replica's disk: its journal, the moments its writes waiting become
/// durable, and its own chance of failing.
pub struct Disk {
    pub(super) journal: Journal,
    pub(super) due: VecDeque<u64>,
    pub(super) rng: Rng,
    pub(super) now: u64,
    pub(super) write_moments: u64,
    pub(super) error: u32,
    /// Whether a write failed since the replica last started.
    pub(super) failed: bool,
}

impl Disk {
    /// A write given: when it is durable, and whether the disk failed.
    fn write(&mut self) -> Result<()> {
        let due = self.now + self.rng.between(1, self.write_moments);
        self.due.push_back(due);
        if self.rng.chance(self.error) {
            self.failed = true;
            return Err(SqlError::new(Code::Io, "[TOOL] the simulated disk failed"));
        }
        Ok(())
    }
}

/// A replica's journal, as its driver writes it, on its disk; the marks,
/// truncations and retentions written noted in the world's trail.
pub struct SimJournal {
    disk: Arc<Mutex<Disk>>,
    id: ReplicaId,
    world: Arc<Mutex<World>>,
}

impl turborel_replication::Journal for SimJournal {
    fn durable(&self) -> bool {
        true
    }

    fn persist(&mut self, at: Lsn, record: &[u8]) -> Result<()> {
        let mut disk = self.disk.lock().unwrap();
        disk.journal.persist(at, record.to_vec())?;
        disk.write()
    }

    fn truncate(&mut self, after: Lsn) -> Result<()> {
        self.world.lock().unwrap().note(self.id, &format!("truncated after {}", after.0));
        let mut disk = self.disk.lock().unwrap();
        disk.journal.truncate(after);
        disk.write()
    }
    fn retain(&mut self, through: Lsn) -> Result<()> {
        self.world.lock().unwrap().note(self.id, &format!("retained through {}", through.0));
        let mut disk = self.disk.lock().unwrap();
        disk.journal.retain(through);
        disk.write()
    }

    fn mark(&mut self, at: Lsn, mark: &Mark) -> Result<()> {
        let members: Vec<u32> = mark.configuration.iter().map(|m| m.id.0).collect();
        self.world.lock().unwrap().note(self.id, &format!("marked {} as epoch {} of {members:?}", at.0, mark.epoch));
        let mut disk = self.disk.lock().unwrap();
        disk.journal.mark(at, mark);
        disk.write()
    }

    fn persist_state(&mut self, state: &[u8]) -> Result<()> {
        let mut disk = self.disk.lock().unwrap();
        disk.journal.persist_state(state.to_vec());
        disk.due.clear();
        Ok(())
    }

    /// The writes the world made durable, as their moments came: a disk
    /// slower than the driver.
    fn sync(&mut self) -> Result<Vec<Lsn>> {
        Ok(self.disk.lock().unwrap().journal.take_owed())
    }

    fn flush(&mut self) -> Result<Vec<Lsn>> {
        let mut disk = self.disk.lock().unwrap();
        disk.due.clear();
        Ok(disk.journal.flush())
    }
}

/// The ticks due to a replica's driver, counted by the world.
#[derive(Default)]
pub struct SimTicks(pub(super) AtomicU64);

impl Ticker for SimTicks {
    fn take(&self) -> u64 {
        self.0.swap(0, Ordering::AcqRel)
    }
}

/// What a replica's driver tells, checked by the world.
pub struct SimWatch {
    id: ReplicaId,
    world: Arc<Mutex<World>>,
}

impl SimWatch {
    /// The world, locked, a violation found noted.
    fn noted(&self, found: std::result::Result<(), Violation>) {
        if let Err(why) = found {
            self.world.lock().unwrap().fail(&why);
        }
    }
}

impl Watch for SimWatch {
    fn committed(&self, (first, through): (Lsn, Lsn), log: &[Vec<u8>]) {
        let mut world = self.world.lock().unwrap();
        world.note(self.id, &format!("committed through {} after {}", through.0, first.0));
        world.report.committed = world.report.committed.max(usize::try_from(through.0).unwrap());
        let found = world.checker.commit(self.id, first.0, log);
        drop(world);
        self.noted(found);
    }

    fn serving(&self, view: u64) {
        let mut world = self.world.lock().unwrap();
        world.note(self.id, &format!("serves view {view}"));
        let epoch = world.node(self.id).epoch;
        let members = world.epochs.members_of(epoch).unwrap_or_default().to_vec();
        let found = world.checker.serves(self.id, (epoch, view), &members);
        drop(world);
        self.noted(found);
    }

    fn stopped(&self) {
        self.world.lock().unwrap().note(self.id, "stopped");
    }

    fn outcome(&self, at: Lsn, record: &[u8], kept: Option<bool>) {
        self.world.lock().unwrap().checker.outcome(self.id, at.0, record.to_vec(), kept);
    }

    fn event(&self, event: &ReplicaEvent) {
        self.world.lock().unwrap().note(self.id, &format!("{event:?}"));
        let found = match event {
            ReplicaEvent::ViewChanged { .. } => {
                self.world.lock().unwrap().recovered(self.id);
                Ok(())
            }
            ReplicaEvent::Removed => {
                self.world.lock().unwrap().removed(self.id);
                Ok(())
            }
            ReplicaEvent::Recovering | ReplicaEvent::Recovered { .. } | ReplicaEvent::Reconfigured { .. } => Ok(()),
            ReplicaEvent::FrameRefused { from, why } => Err(format!("replica {} refused a frame from {}: {why}", self.id.0, from.0)),
            ReplicaEvent::AppendRefused => Err(format!("replica {} refused an append while serving", self.id.0)),
        };
        self.noted(found);
    }
    fn installed(&self, at: Lsn) {
        let mut world = self.world.lock().unwrap();
        world.note(self.id, &format!("installed a state transfer holding through {}", at.0));
        world.report.transfers += 1;
    }

    fn fetching(&self, from: ReplicaId) {
        self.world.lock().unwrap().note(self.id, &format!("fetching the state of replica {}", from.0));
    }

    fn answering(&self, from: ReplicaId) {
        self.world.lock().unwrap().note(self.id, &format!("answering a state transfer's request from replica {}", from.0));
    }

    fn epoch(&self, epoch: u64, configuration: &Configuration) {
        let members: Vec<u32> = configuration.iter().map(|m| m.id.0).collect();
        self.world.lock().unwrap().note(self.id, &format!("in epoch {epoch} of {members:?}"));
        let found = self.world.lock().unwrap().entered(self.id, epoch, configuration);
        self.noted(found);
    }
}

/// The watch of replica `id` in `world`.
pub fn watch(world: &Arc<Mutex<World>>, id: ReplicaId) -> SimWatch {
    SimWatch { id, world: Arc::clone(world) }
}

/// A journal handle on `disk`, replica `id`'s in `world`.
pub fn journal(disk: Arc<Mutex<Disk>>, world: &Arc<Mutex<World>>, id: ReplicaId) -> SimJournal {
    SimJournal { disk, id, world: Arc::clone(world) }
}
