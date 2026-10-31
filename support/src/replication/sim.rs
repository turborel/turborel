//! A deterministic simulator of a replica group (RULES.md D57, R-TEST-5),
//! after TigerBeetle's VOPR: from a seed, a run of moments in which frames
//! are delayed, dropped, duplicated and reordered, the group is
//! partitioned, replicas crash and are made again from their journals,
//! disks are slow, lose writes not yet durable and fail; and in which the
//! primary's database appends records. The [`Checker`] checks every step;
//! then, faults stopped, the group must converge: every replica's database
//! holding the committed log. A failure names its seed, so its run can be
//! made again exactly.
//!
//! The group is the [`World`]; a replica's database is either modelled, as
//! the records it applied ([`simulate`]), or a real turborel database
//! following its replica's log through the core's `Standby` seam and
//! serving SQL when its replica is primary ([`simulate_databases`]).

mod checker;
mod databases;
mod log;
mod network;
mod rng;
mod world;

use std::sync::Arc;

pub use checker::{Checker, Violation};
pub use databases::{DatabaseReport, Replicas, simulate_databases};
pub use log::SimLog;
pub use network::{Link, Network, Weather};
pub use rng::{PER_MILLE, Rng};
pub use world::{Effect, World};

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

impl Plan {
    /// Whether, at moment `now`, the primary's database still appends: in
    /// the faulty moments and the first half of the calm.
    pub fn appending(&self, now: u64) -> bool {
        now < self.faulty + self.calm / 2
    }
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

/// Runs `plan` from `seed`, each replica made by `make`, each replica's
/// database modelled as the records it applied.
///
/// # Errors
///
/// The first violation found, naming the seed and the moment.
pub fn simulate(seed: u64, plan: &Plan, make: &Make) -> Result<Report, Violation> {
    let mut world = World::new(seed, *plan, Arc::clone(make));
    let mut databases: Vec<Vec<Vec<u8>>> = vec![Vec::new(); world.replicas()];
    let mut rng = Rng::new(seed);
    let (mut calm_from, mut before) = (None, 0);
    while !world.over() {
        if world.calm() && calm_from.is_none() {
            calm_from = Some(world.report().appended);
            before = world.committed_log().len();
        }
        world.moment()?;
        carry_out(&mut world, &mut databases)?;
        if plan.appending(world.now()) && rng.chance(plan.append) {
            append(&mut world, &mut databases, &mut rng)?;
        }
    }
    converged(&mut world, &databases, (before, calm_from.unwrap_or(0)))
}

/// The effects queued, carried out on the modelled databases.
fn carry_out(world: &mut World, databases: &mut [Vec<Vec<u8>>]) -> Result<(), Violation> {
    for (index, effect) in world.take_effects() {
        let db = &mut databases[index];
        match effect {
            Effect::Deliver(records) => {
                for (at, record) in records {
                    if at.0 != u64::try_from(db.len()).unwrap() + 1 {
                        return Err(format!("seed {}: replica {} was given record {} after {}", world.seed(), index + 1, at.0, db.len()));
                    }
                    db.push(record);
                }
            }
            Effect::Stop(records) => *db = records.into_iter().map(|(_, record)| record).collect(),
            Effect::Serve(_) => {}
            Effect::Crash => db.clear(),
        }
    }
    Ok(())
}

/// A record appended by the database of a replica serving as primary.
fn append(world: &mut World, databases: &mut [Vec<Vec<u8>>], rng: &mut Rng) -> Result<(), Violation> {
    let serving: Vec<usize> = (0..world.replicas()).filter(|&i| world.serving(i).is_some()).collect();
    if serving.is_empty() {
        return Ok(());
    }
    let index = serving[rng.index(serving.len())];
    let record = format!("{}:{}", world.seed(), world.report().appended + 1).into_bytes();
    if world.append(index, record.clone())?.is_some() {
        databases[index].push(record);
    }
    carry_out(world, databases)
}

/// The end: every replica's database holds the committed log, which grew
/// while calm if records were appended, and the outcomes told agree with
/// it.
fn converged(world: &mut World, databases: &[Vec<Vec<u8>>], (before, calm_from): (usize, u64)) -> Result<Report, Violation> {
    let committed = world.committed_log().to_vec();
    let seed = world.seed();
    for (index, db) in databases.iter().enumerate() {
        if *db != committed {
            let (held, all) = (db.len(), committed.len());
            return Err(format!(
                "seed {seed}: replica {} did not converge: its database holds {held} records of {all} committed",
                index + 1
            ));
        }
    }
    if committed.len() == before && world.report().appended > calm_from {
        return Err(format!("seed {seed}: nothing was committed while calm"));
    }
    world.finish()
}
