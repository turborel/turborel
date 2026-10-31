//! The simulator's world: a group's replicas as their drivers meet them,
//! their disks, links, inboxes and ticks, and the network between them,
//! moved on moment by moment, with faults at random from the run's seed.
//! The world never calls a driver or a database: each moment it puts the
//! frames arrived in the inboxes, makes the journal writes due durable and
//! counts the ticks due, and the drivers then take them in their turns
//! (`sim/ground.rs`). What the drivers carry out it checks through their
//! watches (`SimWatch`).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use turborel::error::{Code, Result, SqlError};
use turborel_replication::driver::{Inbox, Ticker, Watch};
use turborel_replication::{Lsn, Recovered, ReplicaEvent, ReplicaId};

use super::super::Journal;
use super::{Checker, Link, Network, PER_MILLE, Plan, Report, Rng, Violation, Weather};

/// A replica's disk: its journal, the moments its writes waiting become
/// durable, and its own chance of failing.
pub struct Disk {
    journal: Journal,
    due: VecDeque<u64>,
    rng: Rng,
    now: u64,
    write_moments: u64,
    error: u32,
    /// Whether a write failed since the replica last started.
    failed: bool,
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

/// A replica's journal, as its driver writes it, on its disk.
pub struct SimJournal(Arc<Mutex<Disk>>);

impl turborel_replication::Journal for SimJournal {
    fn durable(&self) -> bool {
        true
    }

    fn persist(&mut self, at: Lsn, record: &[u8]) -> Result<()> {
        let mut disk = self.0.lock().unwrap();
        disk.journal.persist(at, record.to_vec());
        disk.write()
    }

    fn truncate(&mut self, after: Lsn) -> Result<()> {
        let mut disk = self.0.lock().unwrap();
        disk.journal.truncate(after);
        disk.write()
    }

    fn persist_state(&mut self, state: &[u8]) -> Result<()> {
        let mut disk = self.0.lock().unwrap();
        disk.journal.persist_state(state.to_vec());
        disk.due.clear();
        Ok(())
    }

    /// The writes the world made durable, as their moments came: a disk
    /// slower than the driver.
    fn sync(&mut self) -> Result<Vec<Lsn>> {
        Ok(self.0.lock().unwrap().journal.take_owed())
    }
}

/// The ticks due to a replica's driver, counted by the world.
#[derive(Default)]
pub struct SimTicks(AtomicU64);

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
    fn committed(&self, through: Lsn, log: &[Vec<u8>]) {
        let mut world = self.world.lock().unwrap();
        world.report.committed = world.report.committed.max(usize::try_from(through.0).unwrap());
        let found = world.checker.commit(self.id, log);
        drop(world);
        self.noted(found);
    }

    fn serving(&self, view: u64) {
        let mut world = self.world.lock().unwrap();
        let members = world.members.clone();
        let found = world.checker.serves(self.id, view, &members);
        drop(world);
        self.noted(found);
    }

    fn stopped(&self) {}

    fn outcome(&self, at: Lsn, record: &[u8], kept: bool) {
        self.world.lock().unwrap().checker.outcome(self.id, at.0, record.to_vec(), kept);
    }

    fn event(&self, event: &ReplicaEvent) {
        let found = match event {
            ReplicaEvent::ViewChanged { .. } => Ok(()),
            ReplicaEvent::FrameRefused { from, why } => Err(format!("replica {} refused a frame from {}: {why}", self.id.0, from.0)),
            ReplicaEvent::AppendRefused => Err(format!("replica {} refused an append while serving", self.id.0)),
        };
        self.noted(found);
    }
}

/// One replica of the run, as the world keeps it.
pub struct Node {
    /// Which it is.
    pub id: ReplicaId,
    disk: Arc<Mutex<Disk>>,
    link: Arc<Link>,
    inbox: Arc<Inbox>,
    ticks: Arc<SimTicks>,
    up: bool,
    /// While down, the moment it is made again.
    up_at: u64,
    /// How many times it crashed.
    lives: u64,
    /// When in each round of moments it ticks.
    phase: u64,
}

/// A replica made again: what its journal holds, and the parts its driver
/// is made of.
pub struct Started {
    /// What its journal holds.
    pub recovered: Recovered,
    /// Its disk, its journal on it.
    pub disk: Arc<Mutex<Disk>>,
    /// Its end of the network.
    pub link: Arc<Link>,
    /// Where its frames arrive.
    pub inbox: Arc<Inbox>,
    /// Its ticks due.
    pub ticks: Arc<SimTicks>,
}

/// What a moment did that the drivers' side carries out: replicas crashed,
/// and replicas to make again.
#[derive(Debug, Default)]
pub struct Moved {
    /// Replicas crashed: their drivers and databases are lost.
    pub crashed: Vec<usize>,
    /// Replicas down whose moment to be made again came.
    pub restart: Vec<usize>,
}

/// The group, its network, and what the run has shown.
pub struct World {
    seed: u64,
    plan: Plan,
    weather: Weather,
    rng: Rng,
    now: u64,
    members: Vec<ReplicaId>,
    nodes: Vec<Node>,
    network: Network,
    checker: Checker,
    report: Report,
    /// The first violation found, once one was.
    failed: Option<Violation>,
}

impl World {
    /// The world of `plan` from `seed`, every replica down until made.
    pub fn new(seed: u64, plan: Plan) -> World {
        let members: Vec<ReplicaId> = (1..=plan.replicas).map(ReplicaId).collect();
        let mut rng = Rng::new(seed);
        let nodes = members
            .iter()
            .map(|&id| {
                let disk_rng = Rng::new(seed ^ u64::from(id.0).rotate_left(48));
                let (write_moments, error) = (plan.faults.write_moments, plan.faults.disk_error);
                let disk =
                    Disk { journal: Journal::default(), due: VecDeque::new(), rng: disk_rng, now: 0, write_moments, error, failed: false };
                Node {
                    id,
                    disk: Arc::new(Mutex::new(disk)),
                    link: Arc::new(Link::new(&members)),
                    inbox: Arc::default(),
                    ticks: Arc::default(),
                    up: false,
                    up_at: 0,
                    lives: 0,
                    phase: rng.below(plan.tick_moments),
                }
            })
            .collect();
        let weather = plan.faults.weather;
        let (network, checker, report) = (Network::default(), Checker::default(), Report::default());
        World { seed, plan, weather, rng, now: 0, members, nodes, network, checker, report, failed: None }
    }

    /// The run's seed.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The moment it is at.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// The members of the group.
    pub fn members(&self) -> &[ReplicaId] {
        &self.members
    }

    /// Whether the run's moments, faulty and calm, are over.
    pub fn over(&self) -> bool {
        self.now >= self.end()
    }

    /// The moment the run's plan ends.
    pub fn end(&self) -> u64 {
        self.plan.faulty + self.plan.calm
    }

    /// Whether the faults have stopped.
    pub fn calm(&self) -> bool {
        self.now >= self.plan.faulty
    }

    /// How many replicas the group has.
    pub fn replicas(&self) -> usize {
        self.nodes.len()
    }

    /// The first violation found, if one was.
    pub fn failed(&self) -> Option<&Violation> {
        self.failed.as_ref()
    }

    /// What the run did so far.
    pub fn report(&mut self) -> &mut Report {
        &mut self.report
    }

    /// The committed log as far as any replica has shown it.
    pub fn committed_log(&self) -> &[Vec<u8>] {
        self.checker.committed()
    }

    /// How many times replica `index` crashed.
    pub fn lives(&self, index: usize) -> u64 {
        self.nodes[index].lives
    }

    /// Whether replica `index` is up.
    pub fn up(&self, index: usize) -> bool {
        self.nodes[index].up
    }

    /// Whether replica `index`'s disk failed since it last started.
    pub fn disk_failed(&self, index: usize) -> bool {
        self.nodes[index].disk.lock().unwrap().failed
    }

    /// The records of replica `index`'s journal, as written.
    pub fn records(&self, index: usize) -> Vec<Vec<u8>> {
        self.nodes[index].disk.lock().unwrap().journal.records().to_vec()
    }

    /// Notes `violation`, unless one was found before, and returns the
    /// first found.
    pub fn fail(&mut self, violation: &str) -> Violation {
        let found = format!("seed {}, moment {}: {violation}", self.seed, self.now);
        self.failed.get_or_insert(found).clone()
    }

    /// The first violation found, as an error.
    ///
    /// # Errors
    ///
    /// The violation.
    pub fn check(&self) -> std::result::Result<(), Violation> {
        self.failed.clone().map_or(Ok(()), Err)
    }

    /// One moment: faults begun while faulty, if `top` (a moment no
    /// database call waits within), or ended as the calm begins; frames
    /// arriving put in the inboxes of replicas up, journal writes due made
    /// durable, ticks counted. The replicas crashed, if `top`, and those to
    /// make again.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn moment(&mut self, top: bool) -> std::result::Result<Moved, Violation> {
        self.check()?;
        let mut moved = Moved::default();
        if self.now < self.plan.faulty && top {
            moved.crashed = self.inject();
        } else if self.now == self.plan.faulty {
            self.end_faults();
        }
        self.now += 1;
        for (from, to, frame) in self.network.arrivals(self.now) {
            let node = self.nodes.iter().find(|n| n.id == to).unwrap();
            if node.up {
                node.inbox.push(from, frame);
            }
        }
        for node in &self.nodes {
            let mut disk = node.disk.lock().unwrap();
            disk.now = self.now;
            while disk.due.front().is_some_and(|&due| due <= self.now) && node.up {
                disk.due.pop_front();
                disk.journal.make_durable();
            }
            if node.up && (self.now + node.phase).is_multiple_of(self.plan.tick_moments) {
                node.ticks.0.fetch_add(1, Ordering::AcqRel);
            }
        }
        moved.restart = (0..self.nodes.len()).filter(|&i| !self.nodes[i].up && self.nodes[i].up_at <= self.now).collect();
        Ok(moved)
    }

    /// Faults begun at random: a partition, crashes; the replicas crashed.
    fn inject(&mut self) -> Vec<usize> {
        let faults = self.plan.faults;
        if !self.network.parted() && self.rng.chance(faults.partition) {
            let mut side = Vec::new();
            for &id in &self.members {
                if self.rng.chance(PER_MILLE / 2) {
                    side.push(id);
                }
            }
            let until = self.now + self.rng.between(1, faults.partition_moments);
            self.network.partition(side, until);
        }
        let mut crashed = Vec::new();
        for index in 0..self.nodes.len() {
            if self.nodes[index].up && self.rng.chance(faults.crash) {
                self.crash(index);
                crashed.push(index);
            }
        }
        crashed
    }

    /// The faults ended: the partition healed, the network reliable, every
    /// replica down made again at the next moment.
    fn end_faults(&mut self) {
        self.network.heal();
        self.weather = Weather { drop: 0, duplicate: 0, late: 0, ..self.weather };
        for node in &mut self.nodes {
            node.up_at = node.up_at.min(self.now + 1);
        }
    }

    /// Replica `index` crashes: of its writes waiting, some reached the
    /// disk; its frames waiting and ticks are lost, and it stays down a
    /// while. Its driver and database are lost with it, which the drivers'
    /// side lets go.
    pub fn crash(&mut self, index: usize) {
        let down = self.rng.between(1, self.plan.faults.down_moments);
        let calm = self.calm();
        let node = &mut self.nodes[index];
        {
            let mut disk = node.disk.lock().unwrap();
            let reached = self.rng.below(u64::try_from(disk.due.len()).unwrap() + 1);
            disk.journal.crash(usize::try_from(reached).unwrap());
            disk.due.clear();
        }
        node.inbox.clear();
        node.link.take();
        node.ticks.take();
        node.up = false;
        node.lives += 1;
        node.up_at = if calm { self.now + 1 } else { self.now + down };
        self.report.crashes += 1;
    }

    /// Replica `index` made again: what its journal holds, and the parts
    /// its driver is made of.
    pub fn start(&mut self, index: usize) -> Started {
        let node = &mut self.nodes[index];
        node.up = true;
        let recovered = {
            let mut disk = node.disk.lock().unwrap();
            disk.failed = false;
            disk.journal.recovered()
        };
        Started {
            recovered,
            disk: Arc::clone(&node.disk),
            link: Arc::clone(&node.link),
            inbox: Arc::clone(&node.inbox),
            ticks: Arc::clone(&node.ticks),
        }
    }

    /// The frames the replicas sent, put into the network.
    pub fn flush_sends(&mut self) {
        for index in 0..self.nodes.len() {
            let (id, sent) = (self.nodes[index].id, self.nodes[index].link.take());
            for (to, bytes) in sent {
                if self.nodes[index].up {
                    self.network.send(&mut self.rng, self.weather, self.now, (id, to, bytes));
                }
            }
        }
    }

    /// The end, as replication sees it: the outcomes told agree with the
    /// committed log. The report then names the last view served and the
    /// outcomes told.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn finish(&mut self) -> std::result::Result<Report, Violation> {
        self.check()?;
        self.checker.check_outcomes().map_err(|why| self.fail(&why))?;
        self.report.view = self.checker.last_view();
        self.report.outcomes = self.checker.outcomes();
        Ok(self.report)
    }
}

/// The watch of replica `id` in `world`.
pub fn watch(world: &Arc<Mutex<World>>, id: ReplicaId) -> SimWatch {
    SimWatch { id, world: Arc::clone(world) }
}

/// A journal handle on `disk`.
pub fn journal(disk: Arc<Mutex<Disk>>) -> SimJournal {
    SimJournal(disk)
}

/// `world`, locked.
pub fn locked(world: &Mutex<World>) -> MutexGuard<'_, World> {
    world.lock().unwrap()
}
