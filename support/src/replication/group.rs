//! A group of replicas a test drives step by step: frames wait in one queue
//! until the test delivers, drops or reorders them, and journal writes wait
//! until it makes them durable, unless its disks are instant.

use std::collections::VecDeque;

use turborel_replication::{Consensus, Input, Lsn, Output, Recovered, ReplicaId};

use super::Journal;

/// Makes a replica's consensus: of the group's members, as one of them,
/// new (`None`) or again after a crash, from what its journal recovered.
pub type Make = Box<dyn Fn(&[ReplicaId], ReplicaId, Option<Recovered>) -> Box<dyn Consensus>>;

/// A frame sent and not yet delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Its sender.
    pub from: ReplicaId,
    /// Its receiver.
    pub to: ReplicaId,
    /// Its bytes.
    pub bytes: Vec<u8>,
}

/// One replica: its consensus, journal and database.
pub struct Node {
    /// Which it is.
    pub id: ReplicaId,
    /// Its consensus.
    pub consensus: Box<dyn Consensus>,
    /// Its journal.
    pub journal: Journal,
    /// The records delivered to its database, in order.
    pub applied: Vec<Vec<u8>>,
    /// Every output it gave, in order.
    pub outputs: Vec<Output>,
}

impl Node {
    /// Gives its database the records through `through` it has not
    /// applied, from its journal.
    pub fn apply(&mut self, through: Lsn) {
        let through = usize::try_from(through.0).unwrap();
        while self.applied.len() < through {
            let next = Lsn(u64::try_from(self.applied.len()).unwrap() + 1);
            self.applied
                .push(self.journal.record(next).unwrap_or_else(|| panic!("record {next:?} delivered is not in the journal")).clone());
        }
    }
}

/// A group of replicas.
pub struct Group {
    members: Vec<ReplicaId>,
    make: Make,
    nodes: Vec<Node>,
    /// Frames sent and not yet delivered, oldest first.
    pub frames: VecDeque<Frame>,
    /// Whether each journal write is durable, and answered, as soon as it
    /// is given.
    pub instant_disks: bool,
}

/// Steps past which `settle` decides a group never settles.
const SETTLE_LIMIT: usize = 100_000;

impl Group {
    /// A new group of `members`, each made by `make`, with instant disks.
    pub fn new(members: &[ReplicaId], make: Make) -> Group {
        let nodes = members
            .iter()
            .map(|&id| Node {
                id,
                consensus: make(members, id, None),
                journal: Journal::default(),
                applied: Vec::new(),
                outputs: Vec::new(),
            })
            .collect();
        Group { members: members.to_vec(), make, nodes, frames: VecDeque::new(), instant_disks: true }
    }

    /// Replica `id`.
    pub fn node(&self, id: ReplicaId) -> &Node {
        self.nodes.iter().find(|n| n.id == id).unwrap()
    }

    fn node_mut(&mut self, id: ReplicaId) -> &mut Node {
        self.nodes.iter_mut().find(|n| n.id == id).unwrap()
    }

    /// Steps replica `id` with `input`, carries out its outputs, and returns
    /// them (those of the `Persisted` instant disks answer with after them).
    pub fn step(&mut self, id: ReplicaId, input: Input) -> Vec<Output> {
        let mut out = Vec::new();
        self.node_mut(id).consensus.step(input, &mut out);
        self.carry(id, &out);
        if self.instant_disks {
            out.extend(self.flush(id));
        }
        out
    }

    /// Carries out `outputs` of replica `id`, in order.
    fn carry(&mut self, id: ReplicaId, outputs: &[Output]) {
        for output in outputs {
            if let Output::Send { to, frame } = output {
                self.frames.push_back(Frame { from: id, to: *to, bytes: frame.clone() });
            }
            let node = self.node_mut(id);
            node.outputs.push(output.clone());
            match output {
                Output::Persist { at, record } => node.journal.persist(*at, record.clone()),
                Output::Truncate { after } => node.journal.truncate(*after),
                Output::PersistState(state) => node.journal.persist_state(state.clone()),
                Output::Deliver { through } => node.apply(*through),
                Output::Assigned(_)
                | Output::Send { .. }
                | Output::Committed(_)
                | Output::Outcome { .. }
                | Output::Role(_)
                | Output::FetchState { .. }
                | Output::Event(_) => {}
            }
        }
    }

    /// Makes every write of replica `id` durable, steps it with each
    /// `Persisted` owed, and returns its outputs.
    pub fn flush(&mut self, id: ReplicaId) -> Vec<Output> {
        let node = self.node_mut(id);
        while node.journal.make_durable() {}
        let owed = node.journal.take_owed();
        owed.into_iter().flat_map(|at| self.step(id, Input::Persisted(at))).collect()
    }

    /// Steps every replica with a tick.
    pub fn tick(&mut self) {
        for id in self.members.clone() {
            self.step(id, Input::Tick);
        }
    }

    /// Delivers the oldest frame waiting; false if none waits.
    pub fn deliver(&mut self) -> bool {
        let Some(frame) = self.frames.pop_front() else { return false };
        self.step(frame.to, Input::Received { from: frame.from, frame: frame.bytes });
        true
    }

    /// Delivers frames until none waits.
    pub fn settle(&mut self) {
        for _ in 0..SETTLE_LIMIT {
            if !self.deliver() {
                return;
            }
        }
        panic!("the group did not settle in {SETTLE_LIMIT} frames");
    }

    /// Drops the frames waiting for which `lost` holds.
    pub fn drop_frames(&mut self, lost: impl Fn(&Frame) -> bool) {
        self.frames.retain(|f| !lost(f));
    }

    /// Replica `id` crashes, the first `reached` of its writes waiting
    /// having reached its disk, and is made again from its journal. Its
    /// database, which keeps nothing, is empty again.
    pub fn crash(&mut self, id: ReplicaId, reached: usize) {
        let members = self.members.clone();
        let node = self.nodes.iter_mut().find(|n| n.id == id).unwrap();
        node.journal.crash(reached);
        node.consensus = (self.make)(&members, id, Some(node.journal.recovered()));
        node.applied.clear();
    }
}
