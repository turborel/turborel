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
//! The group is the [`World`]; each replica up has the crate's real driver
//! (`sim/ground.rs`), over its database, which is either modelled, as the
//! records it holds ([`simulate`]), or a real turborel database following
//! its replica's log through the core's `Standby` seam and serving SQL when
//! its replica is primary ([`simulate_databases`]).

mod checker;
mod databases;
mod ground;
mod network;
mod rng;
mod world;

use std::sync::{Arc, Mutex};

use turborel::db::Standby;
use turborel::error::{Code, Result, SqlError};
use turborel::log::{Log, Lsn};
use turborel_replication::Driver;

pub use checker::{Checker, Violation};
pub use databases::{DatabaseReport, Replicas, simulate_databases};
pub use ground::{Host, Runner};
pub use network::{Link, Network, Weather};
pub use rng::{PER_MILLE, Rng};
pub use world::{Moved, World};

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

/// A database modelled as the records it holds, in order: it follows by
/// holding the records it is given, and keeps nothing when opened again.
#[derive(Default)]
pub struct Model(Mutex<Vec<Vec<u8>>>);

impl Model {
    /// The records it holds.
    pub fn records(&self) -> Vec<Vec<u8>> {
        self.0.lock().unwrap().clone()
    }

    /// Holds `record`, at `at`, the position after its last.
    fn hold(&self, at: Lsn, record: Vec<u8>) -> Result<()> {
        let mut held = self.0.lock().unwrap();
        if at.0 != u64::try_from(held.len()).unwrap() + 1 {
            return Err(SqlError::new(Code::Api, format!("[TOOL] record {} given after {}", at.0, held.len())));
        }
        held.push(record);
        Ok(())
    }
}

impl Standby for Model {
    fn replay(&self, records: &mut dyn Iterator<Item = Result<(Lsn, Vec<u8>)>>) -> Result<Lsn> {
        let mut last = Lsn(0);
        for read in records {
            let (at, record) = read?;
            self.hold(at, record)?;
            last = at;
        }
        Ok(last)
    }

    fn promote(&self, _log: Arc<dyn Log>) -> Result<()> {
        Ok(())
    }

    fn depose(&self) -> Result<()> {
        Ok(())
    }

    fn reopen(&self) -> Result<Lsn> {
        self.0.lock().unwrap().clear();
        Ok(Lsn(0))
    }
}

/// The modelled databases of a run, each new as its replica starts: a
/// handle, shared with the run's ground.
#[derive(Default, Clone)]
pub struct Models(Arc<Mutex<Vec<Option<Arc<Model>>>>>);

impl Models {
    /// Replica `index`'s database, while it is up.
    fn model(&self, index: usize) -> Option<Arc<Model>> {
        self.0.lock().unwrap().get(index).cloned().flatten()
    }
}

impl Host for Models {
    fn open(&mut self, index: usize, _driver: &Arc<Driver>) -> std::result::Result<(Arc<dyn Standby>, Lsn), Violation> {
        let model = Arc::new(Model::default());
        let mut models = self.0.lock().unwrap();
        if models.len() <= index {
            models.resize(index + 1, None);
        }
        models[index] = Some(Arc::clone(&model));
        Ok((model, Lsn(0)))
    }

    fn crashed(&mut self, index: usize) {
        self.0.lock().unwrap()[index] = None;
    }
}

/// Runs `plan` from `seed`, each replica made by `make`, each replica's
/// database modelled as the records it holds.
///
/// # Errors
///
/// The first violation found, naming the seed and the moment.
pub fn simulate(seed: u64, plan: &Plan, make: &Make) -> std::result::Result<Report, Violation> {
    let mut runner = Runner::new(seed, *plan, make, Models::default())?;
    let mut rng = Rng::new(seed);
    let (mut calm_from, mut before) = (None, 0);
    while !runner.world().over() {
        if runner.world().calm() && calm_from.is_none() {
            calm_from = Some(runner.world().report().appended);
            before = runner.world().committed_log().len();
        }
        runner.moment()?;
        let now = runner.world().now();
        if plan.appending(now) && rng.chance(plan.append) {
            append(&mut runner, &mut rng)?;
        }
    }
    converged(&mut runner, (before, calm_from.unwrap_or(0)))
}

/// A record appended by the database of the replica serving as primary,
/// if one does.
fn append(runner: &mut Runner<Models>, rng: &mut Rng) -> std::result::Result<(), Violation> {
    let Some(index) = runner.primary() else { return Ok(()) };
    let (Some(driver), Some(model)) = (runner.driver(index), runner.host.model(index)) else { return Ok(()) };
    let record = format!("{}:{}:{}", runner.world().seed(), index, rng.bits()).into_bytes();
    runner.world().report().appended += 1;
    match driver.log().append(&record) {
        Ok(at) => model.hold(at, record).map_err(|e| runner.world().fail(&format!("replica {}: {e}", index + 1))),
        // Its disk failed as it persisted the record: the next moment
        // crashes it.
        Err(_) => Ok(()),
    }
}

/// The end: every replica's database holds the committed log, which grew
/// while calm if records were appended; a replica serves as primary; the
/// outcomes told agree with the log.
fn converged(runner: &mut Runner<Models>, (before, calm_from): (usize, u64)) -> std::result::Result<Report, Violation> {
    let committed = runner.world().committed_log().to_vec();
    let seed = runner.world().seed();
    let replicas = runner.world().replicas();
    for index in 0..replicas {
        let held = runner.host.model(index).map(|m| m.records()).unwrap_or_default();
        if held != committed {
            let (count, all) = (held.len(), committed.len());
            return Err(format!(
                "seed {seed}: replica {} did not converge: its database holds {count} records of {all} committed",
                index + 1
            ));
        }
    }
    if committed.len() == before && runner.world().report().appended > calm_from {
        return Err(format!("seed {seed}: nothing was committed while calm"));
    }
    if runner.primary().is_none() {
        return Err(runner.world().fail("no replica serves as primary after the calm"));
    }
    runner.world().finish()
}
