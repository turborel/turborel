//! A deterministic simulator of a replica group (RULES.md D57, R-TEST-5),
//! after TigerBeetle's VOPR: from a seed, a run of moments in which frames
//! are delayed, dropped, duplicated and reordered, the group is
//! partitioned, replicas crash and are made again from their journals,
//! disks are slow, lose writes not yet durable and fail; and in which the
//! primary's database appends records. The [`Checker`] checks every step;
//! then, faults stopped, the group must converge: every replica's database
//! holding the committed log. A failure names its seed, so its run can be
//! made again exactly.

mod checker;
mod network;
mod rng;

use std::collections::{BTreeMap, VecDeque};

use turborel_replication::{Consensus, Input, Lsn, Output, ReplicaEvent, ReplicaId, Role, Transport};

pub use checker::{Checker, Violation};
pub use network::{Link, Network, Weather};
pub use rng::{PER_MILLE, Rng};

use super::Journal;
use super::group::Make;

/// The faults of a run, as chances.
#[derive(Debug, Clone, Copy)]
pub struct Faults {
    /// The network's.
    pub weather: Weather,
    /// Per mille, at each moment no partition stands, that one begins.
    pub partition: u32,
    /// The most moments a partition stands.
    pub partition_moments: u64,
    /// Per mille, at each moment, that a replica up crashes.
    pub crash: u32,
    /// The most moments a replica crashed stays down.
    pub down_moments: u64,
    /// Per mille of journal writes on which the disk fails, and its replica
    /// crashes (fail-stop).
    pub disk_error: u32,
    /// The most moments a journal write takes to be durable.
    pub write_moments: u64,
}

/// What a run is: its group, how long, how busy, its faults.
#[derive(Debug, Clone, Copy)]
pub struct Plan {
    /// Replicas in the group.
    pub replicas: u32,
    /// Moments with faults.
    pub faulty: u64,
    /// Moments after, with none, in the first half of which records are
    /// still appended; by their end the group must have converged.
    pub calm: u64,
    /// Per mille, at each moment, that the primary's database appends.
    pub append: u32,
    /// Moments between a replica's ticks.
    pub tick_moments: u64,
    /// The faults.
    pub faults: Faults,
}

/// What a run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// Records committed, at the end.
    pub committed: usize,
    /// The last view a primary served.
    pub view: u64,
    /// Crashes.
    pub crashes: u64,
    /// Records appended.
    pub appended: u64,
    /// Outcomes told of records in doubt.
    pub outcomes: usize,
}

/// A replica's database, as far as the simulator models it: the records
/// it applied, of which the first `certain` are known committed.
#[derive(Debug, Default)]
struct Database {
    applied: Vec<Vec<u8>>,
    certain: usize,
}

/// One replica of the run.
struct Node {
    id: ReplicaId,
    /// Its consensus, while it is up.
    consensus: Option<Box<dyn Consensus>>,
    journal: Journal,
    link: Link,
    /// The moments each write waiting in the journal becomes durable.
    due: VecDeque<u64>,
    db: Database,
    /// The view it serves as primary, if it does.
    serving: Option<u64>,
    /// The records it appended in its service, by position, not yet
    /// committed.
    appended: BTreeMap<u64, Vec<u8>>,
    /// The records it appended in a service ended, by position, in doubt.
    doubted: BTreeMap<u64, Vec<u8>>,
    /// The last `Committed` it gave.
    committed: u64,
    /// While down, the moment it is made again.
    up_at: u64,
    /// When in each round of moments it ticks.
    phase: u64,
}

/// A run in progress.
struct Simulation<'a> {
    seed: u64,
    plan: Plan,
    weather: Weather,
    make: &'a Make,
    rng: Rng,
    now: u64,
    members: Vec<ReplicaId>,
    nodes: Vec<Node>,
    network: Network,
    checker: Checker,
    report: Report,
    /// The record being appended, until its position is assigned.
    appending: Option<Vec<u8>>,
    /// Records appended when the calm began.
    calm_from: u64,
}

/// Runs `plan` from `seed`, each replica made by `make`.
///
/// # Errors
///
/// The first violation found, naming the seed and the moment.
pub fn simulate(seed: u64, plan: &Plan, make: &Make) -> Result<Report, Violation> {
    let mut sim = Simulation::new(seed, *plan, make);
    sim.run().map_err(|why| format!("seed {seed}, moment {}: {why}", sim.now))?;
    Ok(sim.report)
}

impl<'a> Simulation<'a> {
    fn new(seed: u64, plan: Plan, make: &'a Make) -> Simulation<'a> {
        let members: Vec<ReplicaId> = (1..=plan.replicas).map(ReplicaId).collect();
        let mut rng = Rng::new(seed);
        let nodes = members
            .iter()
            .map(|&id| Node {
                id,
                consensus: Some(make(&members, id, None)),
                journal: Journal::default(),
                link: Link::new(&members),
                due: VecDeque::new(),
                db: Database::default(),
                serving: None,
                appended: BTreeMap::new(),
                doubted: BTreeMap::new(),
                committed: 0,
                up_at: 0,
                phase: rng.below(plan.tick_moments),
            })
            .collect();
        let weather = plan.faults.weather;
        let (network, checker, report) = (Network::default(), Checker::default(), Report::default());
        Simulation { seed, plan, weather, make, rng, now: 0, members, nodes, network, checker, report, appending: None, calm_from: 0 }
    }

    /// The faulty moments, then the calm ones, then the checks of the end.
    fn run(&mut self) -> Result<(), Violation> {
        for _ in 0..self.plan.faulty {
            self.inject();
            self.moment(true)?;
        }
        self.calm();
        let before = self.checker.committed().len();
        for moment in 0..self.plan.calm {
            self.moment(moment < self.plan.calm / 2)?;
        }
        self.converged(before)
    }

    /// One moment: replicas due made again, a record perhaps appended,
    /// frames arriving, writes made durable, ticks.
    fn moment(&mut self, appending: bool) -> Result<(), Violation> {
        self.now += 1;
        for index in 0..self.nodes.len() {
            if self.nodes[index].consensus.is_none() && self.nodes[index].up_at <= self.now {
                self.restart(index);
            }
        }
        if appending && self.rng.chance(self.plan.append) {
            self.append()?;
        }
        for (from, to, frame) in self.network.arrivals(self.now) {
            self.step(self.index(to), Input::Received { from, frame })?;
        }
        for index in 0..self.nodes.len() {
            self.complete_writes(index)?;
        }
        for index in 0..self.nodes.len() {
            if (self.now + self.nodes[index].phase).is_multiple_of(self.plan.tick_moments) {
                self.step(index, Input::Tick)?;
            }
        }
        Ok(())
    }

    fn index(&self, id: ReplicaId) -> usize {
        self.nodes.iter().position(|n| n.id == id).unwrap()
    }

    /// Faults begun at random: a partition, crashes.
    fn inject(&mut self) {
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
        for index in 0..self.nodes.len() {
            if self.nodes[index].consensus.is_some() && self.rng.chance(faults.crash) {
                self.crash(index);
            }
        }
    }

    /// The faults ended: the partition healed, the network reliable, every
    /// replica down made again.
    fn calm(&mut self) {
        self.calm_from = self.report.appended;
        self.network.heal();
        self.weather = Weather { drop: 0, duplicate: 0, late: 0, ..self.weather };
        for index in 0..self.nodes.len() {
            if self.nodes[index].consensus.is_none() {
                self.restart(index);
            }
        }
    }

    /// Replica `index` crashes: of its writes waiting, some reached the
    /// disk; its database and its record of what it appended are lost.
    fn crash(&mut self, index: usize) {
        let reached = self.rng.below(u64::try_from(self.nodes[index].due.len()).unwrap() + 1);
        let down = self.rng.between(1, self.plan.faults.down_moments);
        let node = &mut self.nodes[index];
        node.journal.crash(usize::try_from(reached).unwrap());
        node.due.clear();
        node.consensus = None;
        node.db = Database::default();
        node.serving = None;
        node.appended.clear();
        node.doubted.clear();
        node.committed = 0;
        node.link.take();
        node.up_at = self.now + down;
        self.report.crashes += 1;
    }

    /// Replica `index` made again from its journal.
    fn restart(&mut self, index: usize) {
        let node = &mut self.nodes[index];
        node.consensus = Some((self.make)(&self.members, node.id, Some(node.journal.recovered())));
    }

    /// A record appended by the database of a replica serving as primary.
    fn append(&mut self) -> Result<(), Violation> {
        let serving: Vec<usize> = (0..self.nodes.len()).filter(|&i| self.nodes[i].serving.is_some()).collect();
        if serving.is_empty() {
            return Ok(());
        }
        let index = serving[self.rng.index(serving.len())];
        self.report.appended += 1;
        let record = format!("{}:{}", self.seed, self.report.appended).into_bytes();
        self.appending = Some(record.clone());
        self.step(index, Input::Append(record))
    }

    /// Writes of replica `index` due by now made durable, and answered.
    fn complete_writes(&mut self, index: usize) -> Result<(), Violation> {
        let node = &mut self.nodes[index];
        if node.consensus.is_none() {
            return Ok(());
        }
        while node.due.front().is_some_and(|&due| due <= self.now) {
            node.due.pop_front();
            node.journal.make_durable();
        }
        for at in node.journal.take_owed() {
            self.step(index, Input::Persisted(at))?;
        }
        Ok(())
    }

    /// Steps replica `index`, if it is up, and carries out its outputs.
    fn step(&mut self, index: usize, input: Input) -> Result<(), Violation> {
        let Some(consensus) = self.nodes[index].consensus.as_mut() else { return Ok(()) };
        let mut out = Vec::new();
        consensus.step(input, &mut out);
        let mut failed = false;
        for output in out {
            failed |= self.carry(index, output)?;
        }
        if failed {
            self.crash(index);
        }
        Ok(())
    }

    /// Carries out one output of replica `index`; true if its disk failed.
    fn carry(&mut self, index: usize, output: Output) -> Result<bool, Violation> {
        let id = self.nodes[index].id;
        match output {
            Output::Persist { at, record } => {
                self.nodes[index].journal.persist(at, record);
                return Ok(self.write(index));
            }
            Output::Truncate { after } => {
                self.nodes[index].journal.truncate(after);
                return Ok(self.write(index));
            }
            Output::PersistState(state) => {
                self.nodes[index].journal.persist_state(state);
                self.nodes[index].due.clear();
            }
            Output::Send { to, frame } => self.send(index, to, &frame)?,
            Output::Assigned(at) => self.assigned(index, at)?,
            Output::Committed(at) => self.committed(index, at)?,
            Output::Deliver { through } => self.deliver(index, through)?,
            Output::Outcome { at, kept } => {
                let Some(record) = self.nodes[index].doubted.remove(&at.0) else {
                    return Err(format!("replica {} was told of a record at {} it did not append", id.0, at.0));
                };
                self.checker.outcome(id, at.0, record, kept);
            }
            Output::Role(role) => self.role(index, role)?,
            Output::FetchState { from } => {
                return Err(format!("replica {} asked replica {} for state, which VR never needs", id.0, from.0));
            }
            Output::Event(event) => Simulation::event(id, event)?,
        }
        Ok(false)
    }

    /// A journal write given: when it is durable, and whether the disk
    /// failed.
    fn write(&mut self, index: usize) -> bool {
        let due = self.now + self.rng.between(1, self.plan.faults.write_moments);
        self.nodes[index].due.push_back(due);
        self.rng.chance(self.plan.faults.disk_error)
    }

    /// A frame sent through the replica's link, into the network.
    fn send(&mut self, index: usize, to: ReplicaId, frame: &[u8]) -> Result<(), Violation> {
        let node = &self.nodes[index];
        node.link.send(to, frame).map_err(|e| format!("replica {} could not send: {e}", node.id.0))?;
        for (receiver, bytes) in node.link.take() {
            self.network.send(&mut self.rng, self.weather, self.now, (node.id, receiver, bytes));
        }
        Ok(())
    }

    /// A position assigned to the record being appended, which the
    /// primary's database holds at once.
    fn assigned(&mut self, index: usize, at: Lsn) -> Result<(), Violation> {
        let record = self.appending.take().ok_or("a position assigned to no record appended")?;
        let node = &mut self.nodes[index];
        node.db.applied.push(record.clone());
        if u64::try_from(node.db.applied.len()).unwrap() != at.0 {
            return Err(format!("replica {} assigned {} to a record its database holds at {}", node.id.0, at.0, node.db.applied.len()));
        }
        node.appended.insert(at.0, record);
        Ok(())
    }

    /// `Committed(at)`: only to a serving primary, never going back, and
    /// its log through `at` agreeing with what was committed before.
    fn committed(&mut self, index: usize, at: Lsn) -> Result<(), Violation> {
        let node = &mut self.nodes[index];
        if node.serving.is_none() || at.0 < node.committed {
            return Err(format!("replica {} was told {} committed, serving {:?}, after {}", node.id.0, at.0, node.serving, node.committed));
        }
        node.committed = at.0;
        let through = usize::try_from(at.0).unwrap();
        node.db.certain = node.db.certain.max(through);
        node.appended.retain(|&n, _| n > at.0);
        self.report.committed = self.report.committed.max(through);
        self.checker.commit(node.id, &node.journal.records()[..through])
    }

    /// `Deliver { through }`: the records its database lacks applied from
    /// its journal, and what it holds through there agreeing with what was
    /// committed.
    fn deliver(&mut self, index: usize, through: Lsn) -> Result<(), Violation> {
        let node = &mut self.nodes[index];
        let through = usize::try_from(through.0).unwrap();
        if node.journal.records().len() < through {
            return Err(format!("replica {} delivered through {through}, past its journal", node.id.0));
        }
        while node.db.applied.len() < through {
            node.db.applied.push(node.journal.records()[node.db.applied.len()].clone());
        }
        node.db.certain = node.db.certain.max(through);
        self.checker.commit(node.id, &node.db.applied[..through])
    }

    /// `Role`: a primary serves only as the one of its view, once its
    /// database holds its whole log; a deposed one opens again from what it
    /// knew committed.
    fn role(&mut self, index: usize, role: Role) -> Result<(), Violation> {
        let node = &mut self.nodes[index];
        match role {
            Role::Primary { view } => {
                self.checker.serves(node.id, view, &self.members)?;
                if node.db.applied != node.journal.records() {
                    return Err(format!("replica {} serves view {view} before its database holds the log it took over", node.id.0));
                }
                node.serving = Some(view);
            }
            Role::Backup => {
                node.serving = None;
                node.doubted.append(&mut node.appended);
                let certain = node.db.certain;
                node.db.applied.truncate(certain);
            }
        }
        Ok(())
    }

    /// Events: a frame refused, or an append refused, never happens here.
    fn event(id: ReplicaId, event: ReplicaEvent) -> Result<(), Violation> {
        match event {
            ReplicaEvent::ViewChanged { .. } => Ok(()),
            ReplicaEvent::FrameRefused { from, why } => Err(format!("replica {} refused a frame from {}: {why}", id.0, from.0)),
            ReplicaEvent::AppendRefused => Err(format!("replica {} refused an append while serving", id.0)),
        }
    }

    /// The end: every replica's database holds the committed log, which
    /// grew while calm if records were appended, and the outcomes told
    /// agree with it.
    fn converged(&mut self, before: usize) -> Result<(), Violation> {
        let committed = self.checker.committed().to_vec();
        for node in &self.nodes {
            if node.db.applied != committed {
                let (held, all) = (node.db.applied.len(), committed.len());
                return Err(format!("replica {} did not converge: its database holds {held} records of {all} committed", node.id.0));
            }
        }
        if !self.nodes.iter().any(|n| n.serving.is_some()) {
            return Err("no replica serves as primary after the calm".to_owned());
        }
        if committed.len() == before && self.report.appended > self.calm_from {
            return Err("nothing was committed while calm".to_owned());
        }
        self.checker.check_outcomes()?;
        self.report.view = self.checker.last_view();
        self.report.outcomes = self.checker.outcomes();
        Ok(())
    }
}
