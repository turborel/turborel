//! The simulator's network: each replica's [`Link`], the simulator's
//! [`Transport`], whose frames the network delays, drops, duplicates,
//! reorders and cuts off by partitions, by its seed's chances.

use std::sync::Mutex;

use turborel::error::{Code, Result, SqlError};
use turborel_replication::{ReplicaId, Transport};

use super::rng::Rng;

/// A replica's end of the network: what it sends waits in its outbox
/// until the simulator takes it into the network.
pub struct Link {
    members: Vec<ReplicaId>,
    outbox: Mutex<Vec<(ReplicaId, Vec<u8>)>>,
}

impl Link {
    /// The link of a replica of a group of `members`.
    pub fn new(members: &[ReplicaId]) -> Link {
        Link { members: members.to_vec(), outbox: Mutex::new(Vec::new()) }
    }

    /// The frames sent since last taken, in order.
    pub fn take(&self) -> Vec<(ReplicaId, Vec<u8>)> {
        std::mem::take(&mut *self.outbox.lock().unwrap())
    }
}

impl Transport for Link {
    fn send(&self, to: ReplicaId, frame: &[u8]) -> Result<()> {
        if !self.members.contains(&to) {
            return Err(SqlError::new(Code::Api, format!("[EXTENSION] RULES.md D57: replica {} is no member of the group", to.0)));
        }
        self.outbox.lock().unwrap().push((to, frame.to_vec()));
        Ok(())
    }

    fn members(&self) -> &[ReplicaId] {
        &self.members
    }
}

/// A frame on its way.
#[derive(Debug, Clone)]
struct Flight {
    due: u64,
    from: ReplicaId,
    to: ReplicaId,
    frame: Vec<u8>,
}

/// The chances of the network, per frame sent.
#[derive(Debug, Clone, Copy)]
pub struct Weather {
    /// Per mille of frames lost.
    pub drop: u32,
    /// Per mille of frames delivered twice.
    pub duplicate: u32,
    /// The most moments a frame takes; the least is 1.
    pub delay: u64,
    /// Per mille of frames held up, each up to `late_delay` moments.
    pub late: u32,
    /// The most moments a frame held up takes.
    pub late_delay: u64,
}

/// Frames on their way, and the partition, if any.
#[derive(Debug, Default)]
pub struct Network {
    flights: Vec<Flight>,
    /// The replicas on one side of a partition, until a moment: frames
    /// between the sides are lost.
    partition: Option<(Vec<ReplicaId>, u64)>,
}

impl Network {
    /// Sends `frame` from `from` to `to` at `now`, by `weather`.
    pub fn send(&mut self, rng: &mut Rng, weather: Weather, now: u64, (from, to, frame): (ReplicaId, ReplicaId, Vec<u8>)) {
        if rng.chance(weather.drop) {
            return;
        }
        let copies: u32 = if rng.chance(weather.duplicate) { 2 } else { 1 };
        for _ in 0..copies {
            let most = if rng.chance(weather.late) { weather.late_delay } else { weather.delay };
            let due = now + rng.between(1, most);
            self.flights.push(Flight { due, from, to, frame: frame.clone() });
        }
    }

    /// The frames due by `now`, taken, in the order sent; those across the
    /// partition lost.
    pub fn arrivals(&mut self, now: u64) -> Vec<(ReplicaId, ReplicaId, Vec<u8>)> {
        if self.partition.as_ref().is_some_and(|(_, until)| *until <= now) {
            self.partition = None;
        }
        let (due, waiting): (Vec<Flight>, Vec<Flight>) = self.flights.drain(..).partition(|f| f.due <= now);
        self.flights = waiting;
        due.into_iter().filter(|f| !self.across(f.from, f.to)).map(|f| (f.from, f.to, f.frame)).collect()
    }

    /// Whether a partition parts `a` from `b`.
    fn across(&self, a: ReplicaId, b: ReplicaId) -> bool {
        self.partition.as_ref().is_some_and(|(side, _)| side.contains(&a) != side.contains(&b))
    }

    /// Parts `side` from the rest until `until`.
    pub fn partition(&mut self, side: Vec<ReplicaId>, until: u64) {
        self.partition = Some((side, until));
    }

    /// Ends any partition, and loses no more frames to it.
    pub fn heal(&mut self) {
        self.partition = None;
    }

    /// Whether a partition stands.
    pub fn parted(&self) -> bool {
        self.partition.is_some()
    }
}
