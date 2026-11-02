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

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use turborel::bytes::{Put, Reader};
use turborel::db::{ExportedFile, Standby, StateSource};
use turborel::disk::memory::MemoryFileSystem;
use turborel::error::{Code, Result, SqlError};
use turborel::log::{Log, Lsn};
use turborel_replication::{Driver, StateTransfer, WholeFiles};

pub use checker::{Checker, Violation};
pub use databases::{DatabaseReport, Replicas, simulate_databases};
pub use ground::{Host, Runner};
pub use network::{Link, Network, Weather};
pub use rng::{PER_MILLE, Rng};
pub use world::{Moved, World};

use super::group::Make;

/// Per mille, at each moment, that a modelled database up takes a
/// checkpoint.
const CHECKPOINTS: u32 = 10;

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
    /// Per mille of crashes in which the replica's journal is lost, one
    /// replica at a time: made again, it recovers from the group.
    pub lose_journal: u32,
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
    /// The bytes of the log a replica keeps before its database's last
    /// checkpoint at least (log retention): records before are let go, and
    /// a replica behind them fetches a state transfer.
    pub retain_bytes: u64,
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
    /// State transfers installed.
    pub transfers: u64,
    /// Journals lost in crashes.
    pub journals_lost: u64,
}

/// A database modelled as the records it holds, in order: it follows by
/// holding the records it is given, and, opened again, keeps those it was
/// given to replay, committed, as a database keeps what its checkpoints
/// hold, but not those it appended as primary, which may not have been;
/// a checkpoint of its own keeps every record it holds, and lets its
/// replica's log go through it; its export is its replayed records, which
/// another model installs in place of its own.
#[derive(Default)]
pub struct Model {
    held: Mutex<Vec<Vec<u8>>>,
    /// How many of them it was given to replay: kept when opened again.
    replayed: Mutex<usize>,
}

impl Model {
    /// The records it holds.
    pub fn records(&self) -> Vec<Vec<u8>> {
        self.held.lock().unwrap().clone()
    }

    /// A checkpoint: every record held through `committed` kept, as
    /// replayed, but those it appended as primary not yet committed, whose
    /// commit a database's checkpoint waits on; never less than it kept,
    /// as what it replayed is committed; the position it then holds the
    /// log through.
    fn checkpoint(&self, committed: Lsn) -> Lsn {
        let held = self.held.lock().unwrap().len().min(usize::try_from(committed.0).unwrap_or(usize::MAX));
        let mut replayed = self.replayed.lock().unwrap();
        *replayed = held.max(*replayed);
        Lsn(u64::try_from(*replayed).unwrap())
    }

    /// Holds `record`, at `at`, the position after its last.
    fn hold(&self, at: Lsn, record: Vec<u8>) -> Result<()> {
        let mut held = self.held.lock().unwrap();
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
        *self.replayed.lock().unwrap() = self.held.lock().unwrap().len();
        Ok(last)
    }

    fn promote(&self, _log: Arc<dyn Log>) -> Result<()> {
        Ok(())
    }

    fn depose(&self) -> Result<()> {
        Ok(())
    }

    fn reopen(&self) -> Result<Lsn> {
        let replayed = *self.replayed.lock().unwrap();
        self.held.lock().unwrap().truncate(replayed);
        Ok(Lsn(u64::try_from(replayed).unwrap()))
    }

    fn export(&self) -> Result<Box<dyn StateSource>> {
        let (held, replayed) = (self.held.lock().unwrap(), *self.replayed.lock().unwrap());
        let mut image = Vec::new();
        image.put_u32(u32::try_from(replayed).unwrap());
        for record in held.iter().take(replayed) {
            image.put_bytes(record);
        }
        Ok(Box::new(Exported(image)))
    }

    fn import(&self, source: Box<dyn StateSource>) -> Result<Lsn> {
        let mut r = Reader::new(source.image(), "a model's export");
        let count = usize::try_from(r.u32()?).unwrap();
        let mut records = Vec::with_capacity(count);
        for _ in 0..count {
            records.push(r.bytes()?.to_vec());
        }
        r.finish()?;
        *self.held.lock().unwrap() = records;
        *self.replayed.lock().unwrap() = count;
        Ok(Lsn(u64::try_from(count).unwrap()))
    }
}

/// A model's export: its replayed records, how many then each its length
/// and bytes, as the image; no file.
struct Exported(Vec<u8>);

impl StateSource for Exported {
    fn image(&self) -> &[u8] {
        &self.0
    }

    fn files(&self) -> &[ExportedFile] {
        &[]
    }

    fn read(&mut self, _: usize, _: u64, _: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
}

/// A replica's modelled database, kept across its crashes as files are,
/// and whether its replica is up.
#[derive(Default, Clone)]
struct KeptModel {
    model: Arc<Model>,
    up: bool,
}

/// The modelled databases of a run, each kept from its replica's first
/// start: a handle, shared with the run's ground.
#[derive(Default, Clone)]
pub struct Models(Arc<Mutex<Vec<KeptModel>>>);

impl Models {
    /// Replica `index`'s database, while it is up.
    fn model(&self, index: usize) -> Option<Arc<Model>> {
        self.0.lock().unwrap().get(index).filter(|kept| kept.up).map(|kept| Arc::clone(&kept.model))
    }
}

impl Host for Models {
    fn open(&mut self, index: usize, _driver: &Arc<Driver>) -> std::result::Result<(Arc<dyn Standby>, Lsn), Violation> {
        let mut models = self.0.lock().unwrap();
        if models.len() <= index {
            models.resize_with(index + 1, KeptModel::default);
        }
        let kept = &mut models[index];
        kept.up = true;
        let holds = kept.model.reopen().map_err(|e| format!("replica {}: {e}", index + 1))?;
        let model: Arc<Model> = Arc::clone(&kept.model);
        Ok((model, holds))
    }

    /// A crash keeps what its checkpoints hold, as `reopen` does.
    fn crashed(&mut self, index: usize) {
        self.0.lock().unwrap()[index].up = false;
    }

    fn transfer(&mut self, _index: usize) -> Arc<dyn StateTransfer> {
        Arc::new(WholeFiles::new(Arc::new(MemoryFileSystem::new()), PathBuf::from("/transfer")))
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
        if rng.chance(CHECKPOINTS) {
            checkpoint(&mut runner, &mut rng);
        }
    }
    converged(&mut runner, (before, calm_from.unwrap_or(0)))
}

/// A checkpoint of the database of a replica up, taken at random: its
/// replica's log let go through what it then holds. The retention failing
/// to be written, the disk failed: the next moment crashes the replica.
fn checkpoint(runner: &mut Runner<Models>, rng: &mut Rng) {
    let index = rng.index(runner.world().replicas());
    let (Some(driver), Some(model)) = (runner.driver(index), runner.host.model(index)) else { return };
    let holds = model.checkpoint(driver.committed());
    let _failed = driver.log().truncate_before(Lsn(holds.0.saturating_add(1)));
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
