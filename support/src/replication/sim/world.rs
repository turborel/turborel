//! The simulator's world: a group's replicas as their drivers meet them,
//! their disks, links, inboxes and ticks (`sim/parts.rs`), and the
//! network between them, moved on moment by moment, with faults at random
//! from the run's seed. The world never calls a driver or a database: each
//! moment it puts the frames arrived in the inboxes, makes the journal
//! writes due durable and counts the ticks due, and the drivers then take
//! them in their turns (`sim/ground.rs`). What the drivers carry out it
//! checks through their watches (`SimWatch`).

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};

use turborel_replication::Transport;
use turborel_replication::driver::{Inbox, Ticker};
use turborel_replication::{Configuration, Recovered, ReplicaId};

use super::super::Journal;
use super::epochs::Epochs;
use super::parts::{Disk, SimTicks};
use super::{Checker, Link, Network, PER_MILLE, Plan, Report, Rng, Violation, Weather};

/// A replica's membership of the group, as it told it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Membership {
    /// No member yet: a spare, down until an epoch names it.
    Spare,
    /// A member, or one that was and is to be removed.
    Joined,
    /// One that told it was removed from the group.
    Removed,
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
    /// Whether it started over a journal holding no state, lost in a
    /// crash or a new member's, and has not yet entered a view since: no
    /// other loses its journal meanwhile, and no reconfiguration is
    /// asked, as a group with two members recovering at once, a new one
    /// among them or not, could not go on [VRR §4.3] [VRR §7].
    journal_lost: bool,
    /// When in each round of moments it ticks.
    phase: u64,
    /// Its membership: a spare until an epoch names it, down until then.
    membership: Membership,
    /// The epoch it last told it entered.
    pub(super) epoch: u64,
    /// Its node's setting of the members: the last configuration it
    /// entered naming it, the members as it joined for a spare.
    setting: Vec<ReplicaId>,
}

/// A replica made again: what its journal holds, and the parts its driver
/// is made of.
pub struct Started {
    /// Whether it is a new group's replica, made as the run begins: the
    /// others are made from their journals, recovering if those hold
    /// nothing, as a node started without `--owner`.
    pub new_group: bool,
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
    /// The epochs entered, the members now among them.
    pub epochs: Epochs,
    nodes: Vec<Node>,
    network: Network,
    pub(super) checker: Checker,
    pub(super) report: Report,
    /// The first violation found, once one was.
    failed: Option<Violation>,
    /// The last things the replicas told, and the crashes and starts,
    /// each at its moment: a violation found names them.
    trail: VecDeque<String>,
}

/// How much of the trail a violation names.
const TRAIL: usize = 400;

impl World {
    /// The run's plan.
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// The world of `plan` from `seed`, every replica down until made, the
    /// spares, numbered after the members, until an epoch names them.
    pub fn new(seed: u64, plan: Plan) -> World {
        let members: Vec<ReplicaId> = (1..=plan.replicas).map(ReplicaId).collect();
        let mut rng = Rng::new(seed);
        let nodes = (1..=plan.replicas.saturating_add(plan.spares))
            .map(ReplicaId)
            .map(|id| {
                let disk_rng = Rng::new(seed ^ u64::from(id.0).rotate_left(48));
                let (write_moments, error) = (plan.faults.write_moments, plan.faults.disk_error);
                let disk =
                    Disk { journal: Journal::default(), due: VecDeque::new(), rng: disk_rng, now: 0, write_moments, error, failed: false };
                let joined = members.contains(&id);
                Node {
                    id,
                    disk: Arc::new(Mutex::new(disk)),
                    link: Arc::new(Link::new(&members)),
                    inbox: Arc::default(),
                    ticks: Arc::default(),
                    up: false,
                    up_at: if joined { 0 } else { u64::MAX },
                    lives: 0,
                    journal_lost: false,
                    phase: rng.below(plan.tick_moments),
                    membership: if joined { Membership::Joined } else { Membership::Spare },
                    epoch: 0,
                    setting: members.clone(),
                }
            })
            .collect();
        let weather = plan.faults.weather;
        let (network, checker, report) = (Network::default(), Checker::default(), Report::default());
        let mut epochs = Epochs::default();
        let _ = epochs.entered(ReplicaId(0), 0, &super::super::group::configuration(&members));
        World { seed, plan, weather, rng, now: 0, epochs, nodes, network, checker, report, failed: None, trail: VecDeque::new() }
    }

    /// The run's seed.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The moment it is at.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// The members of the group, in the latest epoch entered.
    pub fn members(&self) -> &[ReplicaId] {
        self.epochs.members()
    }

    /// Which replica `index` is.
    pub fn id(&self, index: usize) -> ReplicaId {
        self.nodes[index].id
    }

    /// Replica `id`, as the world keeps it.
    pub(super) fn node(&self, id: ReplicaId) -> &Node {
        self.nodes.iter().find(|n| n.id == id).unwrap()
    }

    /// Whether replica `index` is or was a member.
    pub fn joined(&self, index: usize) -> bool {
        self.nodes[index].membership != Membership::Spare
    }

    /// Whether replica `index` was a member and is none now: it must be
    /// removed, and told so.
    pub fn left(&self, index: usize) -> bool {
        self.joined(index) && !self.members().contains(&self.nodes[index].id)
    }

    /// Whether replica `index` told it was removed from the group.
    pub fn removed_told(&self, index: usize) -> bool {
        self.nodes[index].membership == Membership::Removed
    }

    /// The members replica `index`'s node is set to start with: the
    /// members now, if it is one of them, as an operator keeps the setting
    /// current; else the last configuration it entered naming it.
    pub fn setting(&self, index: usize) -> Vec<ReplicaId> {
        let id = self.nodes[index].id;
        if self.members().contains(&id) { self.members().to_vec() } else { self.nodes[index].setting.clone() }
    }

    /// The first spare no epoch named yet, if one waits.
    pub fn spare(&self) -> Option<ReplicaId> {
        self.nodes.iter().find(|n| n.membership == Membership::Spare).map(|n| n.id)
    }

    /// Replica `id` entered `epoch` of `configuration`: its node's setting
    /// if it names it; a new epoch is the members now, the spares it names
    /// joined and made at the next moment, as an operator starts them once
    /// told the reconfiguration is done.
    pub(super) fn entered(&mut self, id: ReplicaId, epoch: u64, configuration: &Configuration) -> Result<(), Violation> {
        let new = self.epochs.entered(id, epoch, configuration)?;
        let members: Vec<ReplicaId> = configuration.iter().map(|m| m.id).collect();
        let now = self.now;
        if let Some(node) = self.nodes.iter_mut().find(|n| n.id == id) {
            node.epoch = epoch;
            if members.contains(&id) {
                node.setting.clone_from(&members);
            }
        }
        if new && epoch == self.epochs.latest() {
            self.report.epochs = epoch;
            for node in self.nodes.iter_mut().filter(|n| n.membership == Membership::Spare && members.contains(&n.id)) {
                node.membership = Membership::Joined;
                node.up_at = now.saturating_add(1);
                node.setting.clone_from(&members);
            }
        }
        Ok(())
    }

    /// The first spare no epoch named yet, to name in a reconfiguration,
    /// if one waits and no replica is recovering: an operator adds a
    /// node to a group whole, as a group with two members recovering at
    /// once, a new one and one whose journal was lost, could not go on
    /// [VRR §4.3] [VRR §7].
    pub fn spare_to_add(&self) -> Option<ReplicaId> {
        if self.recovering_any() {
            return None;
        }
        self.spare()
    }

    /// Whether any replica is recovering, or is to: one that started over
    /// a journal holding no state and has not entered a view since, or a
    /// member of the epoch not started yet, a new one.
    fn recovering_any(&self) -> bool {
        let members = self.epochs.members();
        self.nodes.iter().any(|n| n.journal_lost || (members.contains(&n.id) && !n.disk.lock().unwrap().journal.holds_state()))
    }

    /// Replica `id` told it was removed from the group.
    pub(super) fn removed(&mut self, id: ReplicaId) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.id == id)
            && node.membership != Membership::Removed
        {
            node.membership = Membership::Removed;
            self.report.removed += 1;
        }
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

    /// Replica `id` entered a view: its journal, if lost, is recovered.
    pub(super) fn recovered(&mut self, id: ReplicaId) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.id == id) {
            node.journal_lost = false;
        }
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

    /// Notes that replica `id` told or did `what`, in the trail.
    pub(super) fn note(&mut self, id: ReplicaId, what: &str) {
        if self.trail.len() >= TRAIL {
            self.trail.pop_front();
        }
        self.trail.push_back(format!("  {}: replica {}: {what}", self.now, id.0));
    }

    /// Notes `violation`, unless one was found before, and returns the
    /// first found, with the trail before it.
    pub fn fail(&mut self, violation: &str) -> Violation {
        let trail: Vec<&str> = self.trail.iter().map(String::as_str).collect();
        let found = format!("seed {}, moment {}: {violation}\n{}", self.seed, self.now, trail.join("\n"));
        self.failed.get_or_insert(found).clone()
    }

    /// The first violation found, as an error.
    ///
    /// # Errors
    ///
    /// The violation.
    pub fn check(&self) -> Result<(), Violation> {
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
    pub fn moment(&mut self, top: bool) -> Result<Moved, Violation> {
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
            for &id in self.epochs.members() {
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
        for node in self.nodes.iter_mut().filter(|n| n.membership != Membership::Spare) {
            node.up_at = node.up_at.min(self.now + 1);
        }
    }

    /// Replica `index` crashes: of its writes waiting, some reached the
    /// disk; its frames waiting and ticks are lost, and it stays down a
    /// while. Its driver and database are lost with it, which the drivers'
    /// side lets go. A member's journal may be lost, while no other
    /// replica is recovering and no reconfiguration is pending, which
    /// would make two recovering at once; one that left the group keeps
    /// its, as nothing would answer its recovery.
    pub fn crash(&mut self, index: usize) {
        let down = self.rng.between(1, self.plan.faults.down_moments);
        let calm = self.calm();
        let member = self.epochs.members().contains(&self.nodes[index].id);
        let whole = !self.recovering_any() && !self.epochs.pending_any();
        let lose = self.rng.chance(self.plan.faults.lose_journal) && member && whole;
        let node = &mut self.nodes[index];
        {
            let mut disk = node.disk.lock().unwrap();
            let reached = self.rng.below(u64::try_from(disk.due.len()).unwrap() + 1);
            disk.journal.crash(usize::try_from(reached).unwrap());
            disk.due.clear();
            if lose {
                disk.journal = Journal::default();
                node.journal_lost = true;
                self.report.journals_lost += 1;
            }
        }
        node.inbox.clear();
        node.link.take();
        node.ticks.take();
        node.up = false;
        node.lives += 1;
        node.up_at = if calm { self.now + 1 } else { self.now + down };
        self.report.crashes += 1;
        let id = node.id;
        self.note(id, if lose { "crashed, its journal lost" } else { "crashed" });
    }

    /// Replica `index` made again: what its journal holds, and the parts
    /// its driver is made of, its link linking its node's setting of the
    /// members.
    pub fn start(&mut self, index: usize) -> Started {
        let setting = self.setting(index);
        let node = &mut self.nodes[index];
        node.up = true;
        let _ = node.link.reconfigure(&super::super::group::configuration(&setting));
        let recovered = {
            let mut disk = node.disk.lock().unwrap();
            disk.failed = false;
            disk.journal.recovered()
        };
        let (disk, link, inbox, ticks) = (Arc::clone(&node.disk), Arc::clone(&node.link), Arc::clone(&node.inbox), Arc::clone(&node.ticks));
        if recovered.state.is_none() {
            // Recovering, as a new member or over a journal lost, until it
            // enters a view: no journal is lost meanwhile.
            node.journal_lost = true;
        }
        let (id, held) = (node.id, (recovered.state.is_some(), recovered.first.0, recovered.records.len()));
        let marks: Vec<String> = recovered.marks.iter().map(|(at, m)| format!("{} as epoch {}", at.0, m.epoch)).collect();
        self.note(id, &format!("started, its journal holding state {}, {} records after {}, marked {marks:?}", held.0, held.2, held.1));
        let new_group = self.now == 0;
        Started { new_group, recovered, disk, link, inbox, ticks }
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
    pub fn finish(&mut self) -> Result<Report, Violation> {
        self.check()?;
        self.checker.check_outcomes().map_err(|why| self.fail(&why))?;
        self.report.view = self.checker.last_served().1;
        self.report.outcomes = self.checker.outcomes();
        Ok(self.report)
    }
}

/// `world`, locked.
pub fn locked(world: &Mutex<World>) -> MutexGuard<'_, World> {
    world.lock().unwrap()
}
