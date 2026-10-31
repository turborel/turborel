//! The simulator's world: a group's replicas as replication sees them,
//! their consensus, journals and links, and the network between them,
//! moved on moment by moment, with faults at random from the run's seed.
//! What a replica's database is to do the world queues as [`Effect`]s, for
//! whatever models the databases to carry out (`sim.rs` a model of one,
//! `sim/databases.rs` real turborel databases); so a database's own calls
//! (an append, a sync waiting for its commit) may move the world on, and
//! the world never calls into a database.

use std::collections::{BTreeMap, VecDeque};

use turborel_replication::{Consensus, Input, Lsn, Output, ReplicaEvent, ReplicaId, Role, Transport};

use super::super::Journal;
use super::super::group::Make;
use super::{Checker, Link, Network, PER_MILLE, Plan, Report, Rng, Violation, Weather};

/// What a replica's database is to do, as its replica's consensus said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Apply these records, committed, the next after those it holds.
    Deliver(Vec<(Lsn, Vec<u8>)>),
    /// Serve as primary of the view.
    Serve(u64),
    /// Stop serving: opened again, it holds these records, the log known
    /// committed from its start, and those it appended after are gone.
    Stop(Vec<(Lsn, Vec<u8>)>),
    /// The replica crashed: its database is lost, and starts again empty.
    Crash,
}

/// One replica of the run, as replication sees it.
struct Node {
    id: ReplicaId,
    /// Its consensus, while it is up.
    consensus: Option<Box<dyn Consensus>>,
    journal: Journal,
    link: Link,
    /// The moments each write waiting in the journal becomes durable.
    due: VecDeque<u64>,
    /// The view it serves as primary, if it does.
    serving: Option<u64>,
    /// The records it appended in its service, by position, not yet
    /// committed.
    appended: BTreeMap<u64, Vec<u8>>,
    /// The records it appended in a service ended, by position, in doubt.
    doubted: BTreeMap<u64, Vec<u8>>,
    /// The last `Committed` it gave.
    committed: u64,
    /// Through where its database holds the log known committed.
    certain: u64,
    /// Through where its database holds the log: delivered, appended, or
    /// kept as it stopped.
    held: u64,
    /// How many times it crashed: a database's log of an earlier life is
    /// refused.
    lives: u64,
    /// While down, the moment it is made again.
    up_at: u64,
    /// When in each round of moments it ticks.
    phase: u64,
}

/// The group, its network, and what the run has shown.
pub struct World {
    seed: u64,
    plan: Plan,
    weather: Weather,
    make: Make,
    rng: Rng,
    now: u64,
    members: Vec<ReplicaId>,
    nodes: Vec<Node>,
    network: Network,
    checker: Checker,
    report: Report,
    /// The position assigned to the record being appended.
    assigned: Option<Lsn>,
    effects: VecDeque<(usize, Effect)>,
    /// The first violation found, once one was.
    failed: Option<Violation>,
}

impl World {
    /// The world of `plan` from `seed`, each replica made by `make`.
    pub fn new(seed: u64, plan: Plan, make: Make) -> World {
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
                serving: None,
                appended: BTreeMap::new(),
                doubted: BTreeMap::new(),
                committed: 0,
                certain: 0,
                held: 0,
                lives: 0,
                up_at: 0,
                phase: rng.below(plan.tick_moments),
            })
            .collect();
        let weather = plan.faults.weather;
        let (network, checker, report) = (Network::default(), Checker::default(), Report::default());
        let effects = VecDeque::new();
        World { seed, plan, weather, make, rng, now: 0, members, nodes, network, checker, report, assigned: None, effects, failed: None }
    }

    /// The run's seed.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The moment it is at.
    pub fn now(&self) -> u64 {
        self.now
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

    /// The view replica `index` serves as primary, if it does.
    pub fn serving(&self, index: usize) -> Option<u64> {
        self.nodes[index].serving
    }

    /// Through where replica `index` was told its log is committed, serving.
    pub fn committed(&self, index: usize) -> u64 {
        self.nodes[index].committed
    }

    /// How many times replica `index` crashed.
    pub fn lives(&self, index: usize) -> u64 {
        self.nodes[index].lives
    }

    /// The records of replica `index`'s journal, as written.
    pub fn records(&self, index: usize) -> &[Vec<u8>] {
        self.nodes[index].journal.records()
    }

    /// The effects queued, oldest first, taken.
    pub fn take_effects(&mut self) -> Vec<(usize, Effect)> {
        self.effects.drain(..).collect()
    }

    /// Notes `violation`, unless one was found before, and returns the
    /// first found.
    pub fn fail(&mut self, violation: &str) -> Violation {
        let found = format!("seed {}, moment {}: {violation}", self.seed, self.now);
        self.failed.get_or_insert(found).clone()
    }

    /// One moment: faults begun while faulty, or ended as the calm begins;
    /// replicas due made again, frames arriving, writes made durable,
    /// ticks.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn moment(&mut self) -> Result<(), Violation> {
        if let Some(failed) = &self.failed {
            return Err(failed.clone());
        }
        if self.now < self.plan.faulty {
            self.inject();
        } else if self.now == self.plan.faulty {
            self.end_faults();
        }
        self.now += 1;
        self.moved().map_err(|why| self.fail(&why))
    }

    /// The moment's work, after its faults.
    fn moved(&mut self) -> Result<(), Violation> {
        for index in 0..self.nodes.len() {
            if self.nodes[index].consensus.is_none() && self.nodes[index].up_at <= self.now {
                self.restart(index);
            }
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
    fn end_faults(&mut self) {
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
    pub fn crash(&mut self, index: usize) {
        let reached = self.rng.below(u64::try_from(self.nodes[index].due.len()).unwrap() + 1);
        let down = self.rng.between(1, self.plan.faults.down_moments);
        let node = &mut self.nodes[index];
        node.journal.crash(usize::try_from(reached).unwrap());
        node.due.clear();
        node.consensus = None;
        node.serving = None;
        node.appended.clear();
        node.doubted.clear();
        (node.committed, node.certain, node.held) = (0, 0, 0);
        node.lives += 1;
        node.link.take();
        node.up_at = self.now + down;
        self.report.crashes += 1;
        self.effects.push_back((index, Effect::Crash));
    }

    /// Replica `index` made again from its journal.
    fn restart(&mut self, index: usize) {
        let node = &mut self.nodes[index];
        node.consensus = Some((self.make)(&self.members, node.id, Some(node.journal.recovered())));
    }

    /// The database of replica `index` appends `record`: the position the
    /// consensus gives it, or `None` if the replica is down or serves as no
    /// primary.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn append(&mut self, index: usize, record: Vec<u8>) -> Result<Option<Lsn>, Violation> {
        if let Some(failed) = &self.failed {
            return Err(failed.clone());
        }
        if self.nodes[index].consensus.is_none() || self.nodes[index].serving.is_none() {
            return Ok(None);
        }
        self.report.appended += 1;
        self.step(index, Input::Append(record.clone())).map_err(|why| self.fail(&why))?;
        if self.nodes[index].consensus.is_none() {
            // Its disk failed as it persisted the record: it crashed, and
            // its database with it.
            self.assigned = None;
            return Ok(None);
        }
        let Some(at) = self.assigned.take() else { return Err(self.fail("an append taken was given no position")) };
        let node = &mut self.nodes[index];
        if at.0 != node.held + 1 {
            let why = format!("replica {} assigned {} to a record its database holds at {}", node.id.0, at.0, node.held + 1);
            return Err(self.fail(&why));
        }
        node.held = at.0;
        node.appended.insert(at.0, record);
        Ok(Some(at))
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
            Output::Assigned(at) => self.assigned = Some(at),
            Output::Committed(at) => self.commit(index, at)?,
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
            Output::Event(event) => World::event(id, event)?,
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

    /// `Committed(at)`: only to a serving primary, never going back, and
    /// its log through `at` agreeing with what was committed before.
    fn commit(&mut self, index: usize, at: Lsn) -> Result<(), Violation> {
        let node = &mut self.nodes[index];
        if node.serving.is_none() || at.0 < node.committed {
            return Err(format!("replica {} was told {} committed, serving {:?}, after {}", node.id.0, at.0, node.serving, node.committed));
        }
        node.committed = at.0;
        node.certain = node.certain.max(at.0);
        node.appended.retain(|&n, _| n > at.0);
        let through = usize::try_from(at.0).unwrap();
        self.report.committed = self.report.committed.max(through);
        self.checker.commit(node.id, &node.journal.records()[..through])
    }

    /// `Deliver { through }`: the records its database lacks queued for it
    /// from its journal, and the journal through there agreeing with what
    /// was committed.
    fn deliver(&mut self, index: usize, through: Lsn) -> Result<(), Violation> {
        let node = &mut self.nodes[index];
        let end = usize::try_from(through.0).unwrap();
        if node.journal.records().len() < end {
            return Err(format!("replica {} delivered through {end}, past its journal", node.id.0));
        }
        let from = usize::try_from(node.held).unwrap();
        if from < end {
            let records = (from..end).map(|i| (position(i), node.journal.records()[i].clone())).collect();
            self.effects.push_back((index, Effect::Deliver(records)));
            node.held = through.0;
        }
        node.certain = node.certain.max(through.0);
        self.checker.commit(node.id, &node.journal.records()[..end])
    }

    /// `Role`: a primary serves only as the one of its view, once its
    /// database holds its whole log; a deposed one is opened again from
    /// what it knew committed.
    fn role(&mut self, index: usize, role: Role) -> Result<(), Violation> {
        let node = &mut self.nodes[index];
        match role {
            Role::Primary { view } => {
                self.checker.serves(node.id, view, &self.members)?;
                if node.held != u64::try_from(node.journal.records().len()).unwrap() {
                    return Err(format!("replica {} serves view {view} before its database holds the log it took over", node.id.0));
                }
                node.serving = Some(view);
                self.effects.push_back((index, Effect::Serve(view)));
            }
            Role::Backup => {
                node.serving = None;
                node.doubted.append(&mut node.appended);
                node.held = node.certain;
                let kept = usize::try_from(node.certain).unwrap();
                let records = node.journal.records()[..kept].iter().enumerate().map(|(i, r)| (position(i), r.clone())).collect();
                self.effects.push_back((index, Effect::Stop(records)));
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

    /// The end, as replication sees it: a replica serves as primary, and
    /// the outcomes told agree with the committed log. The report then
    /// names the last view served and the outcomes told.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn finish(&mut self) -> Result<Report, Violation> {
        if let Some(failed) = &self.failed {
            return Err(failed.clone());
        }
        if !self.nodes.iter().any(|n| n.serving.is_some()) {
            return Err(self.fail("no replica serves as primary after the calm"));
        }
        self.checker.check_outcomes().map_err(|why| self.fail(&why))?;
        self.report.view = self.checker.last_view();
        self.report.outcomes = self.checker.outcomes();
        Ok(self.report)
    }
}

/// The position of the record at `index` among a log's records.
fn position(index: usize) -> Lsn {
    Lsn(u64::try_from(index).unwrap() + 1)
}
