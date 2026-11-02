//! The driver (RULES.md D55, D57) through its own seams: a replica alone
//! driven as a job of its database's thread manager, whose database it
//! promotes and which writes through its log; what the driver refuses; and
//! the built-in seams. Groups of replicas run it in the simulator
//! (`simulation.rs`, `databases.rs`).

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use turborel::db::{Database, LogStorage, OpenOptions, Standby, StorePlace};
use turborel::disk::io::FileSystem;
use turborel::disk::memory::MemoryFileSystem;
use turborel::error::{Code, Result, SqlError};
use turborel::log::{Log, Lsn, MemoryLog};
use turborel::threads::JobPolicy;
use turborel_replication::driver::{Blocking, Inbox, JOB_LABEL, Ticker, Ticks, Unwatched, Wait, Watch};
use turborel_replication::{
    Consensus, Driver, DriverJob, GroupLogs, Input, Journal, LogJournal, Output, Parts, ReplicaEvent, ReplicaId, Role, Transport, Vr,
    WholeFiles,
};
use turborel_test_support::database::rows;

use crate::common::options;

/// A transport of a group of one: it sends nothing.
struct Alone([ReplicaId; 1]);

impl Transport for Alone {
    fn send(&self, to: ReplicaId, _frame: &[u8]) -> Result<()> {
        Err(SqlError::new(Code::Api, format!("[TOOL] replica {} is no member", to.0)))
    }
    fn members(&self) -> &[ReplicaId] {
        &self.0
    }
}

/// A tick at every turn.
struct EveryTurn;

impl Ticker for EveryTurn {
    fn take(&self) -> u64 {
        1_u64
    }
}

/// A sync's wait that moves the driver's inputs on itself.
struct Stepping;

impl Wait for Stepping {
    fn wait(&self, driver: &Arc<Driver>) -> Result<()> {
        driver.step_inputs()
    }
}

/// What a watch was told, as words.
#[derive(Default)]
struct Told(Mutex<Vec<String>>);

impl Watch for Told {
    fn committed(&self, (_, through): (Lsn, Lsn), _log: &[Vec<u8>]) {
        self.0.lock().unwrap().push(format!("committed {}", through.0));
    }
    fn serving(&self, view: u64) {
        self.0.lock().unwrap().push(format!("serving {view}"));
    }
    fn stopped(&self) {
        self.0.lock().unwrap().push("stopped".to_owned());
    }
    fn outcome(&self, at: Lsn, _record: &[u8], kept: Option<bool>) {
        self.0.lock().unwrap().push(format!("outcome {} {kept:?}", at.0));
    }
    fn event(&self, event: &ReplicaEvent) {
        self.0.lock().unwrap().push(format!("{event:?}"));
    }
}

/// `told` as a watch.
fn watched(told: &Arc<Told>) -> Arc<dyn Watch> {
    told.clone()
}

/// The parts of a driver of `consensus`, alone, its journal in memory.
fn parts(consensus: Box<dyn Consensus>, wait: Arc<dyn Wait>, watch: Arc<dyn Watch>) -> Parts {
    let (journal, _) = LogJournal::open(Arc::new(MemoryLog::new())).unwrap();
    let transport = Arc::new(Alone([ReplicaId(1_u32)]));
    Parts {
        consensus,
        journal: Box::new(journal),
        first: Lsn(0),
        records: Vec::new(),
        retain_bytes: u64::MAX,
        transport,
        inbox: Arc::default(),
        ticker: Arc::new(EveryTurn),
        wait,
        watch,
        transfer: Arc::new(WholeFiles::new(Arc::new(MemoryFileSystem::new()), std::path::PathBuf::from("/transfer"))),
    }
}

/// A database opened from `fs` to follow `driver`'s log.
fn opened(fs: &Arc<dyn FileSystem>, driver: &Arc<Driver>) -> (Database, Lsn) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_log_storage(Box::new(GroupLogs::of(Arc::clone(driver)))).unwrap();
    let holds = db.open_following(fs, Path::new("/db"), &OpenOptions::default()).unwrap();
    (db, holds)
}

/// Verifies: [DESIGN] RULES.md D55, D57: a replica alone, its driver a job
/// of its database (`turborel-replica`), shown among its jobs: run as the
/// policy says, its turns promote the database, which then takes SQL
/// through the driver's log, a commit waiting for the consensus to commit
/// it; the watch is told of the service and of each commit.
#[test]
fn a_driver_job_serves_its_database() {
    let told = Arc::new(Told::default());
    let vr = Box::new(Vr::new(&[ReplicaId(1_u32)], ReplicaId(1_u32), options()).unwrap());
    let driver = Driver::new(parts(vr, Arc::new(Stepping), watched(&told)));
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let (db, holds) = opened(&fs, &driver);
    assert_eq!(holds, Lsn(0_u64));
    driver.join(holds).unwrap();
    let job = db.add_job(Arc::new(DriverJob::new(Arc::clone(&driver), Duration::from_millis(10_u64)))).unwrap();
    assert!(db.jobs().iter().any(|j| j.label == JOB_LABEL));
    for _ in 0_u32..20_u32 {
        db.request_job(job, Duration::ZERO);
        db.run_due().unwrap();
    }
    assert_eq!(driver.serving(), Some(0_u64));
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("APP");
    s.run("INSERT INTO T VALUES (1)").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT K FROM T").unwrap(), ["1"]);
    let log = driver.log();
    assert!(log.end().0 > 2_u64 && log.read_at(Lsn(1_u64)).unwrap().is_some() && log.read_at(log.end()).unwrap().is_none());
    assert_eq!(log.read_from(Lsn(1_u64)).count(), usize::try_from(log.end().0 - 1_u64).unwrap());
    assert!(!log.capabilities().durable, "as its journal, in memory");
    log.truncate_before(log.end()).unwrap();
    assert_eq!(log.sync(log.end()).err().map(|e| e.code), Some(Code::Api), "no position yet");
    let words = told.0.lock().unwrap().clone();
    assert!(words.contains(&"serving 0".to_owned()) && words.iter().any(|w| w.starts_with("committed")), "{words:?}");
}

/// Verifies: [DESIGN] RULES.md D55, D57 [VRR §4.3]: a replica alone made
/// again from a journal holding another primary's log and no state, a
/// transaction of it left open: it recovers at once, alone, in view 0;
/// its driver job delivers the log to its database, which follows, then
/// promotes it; the promotion's abort record of the transaction left open
/// is synced on the job's own thread, the built-in wait moving the driver
/// on there; the database holds the row committed, not the one left
/// open, and serves.
#[test]
fn a_promotion_syncs_on_the_drivers_own_thread() {
    let source = Arc::new(MemoryLog::new());
    let primary = Database::new();
    let logged: Arc<dyn Log> = source.clone();
    primary.set_log(logged).unwrap();
    primary.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER)").unwrap();
    let s = primary.session("APP");
    s.run("INSERT INTO T VALUES (1)").unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (2)").unwrap();
    let records: Vec<Vec<u8>> = source.read_from(Lsn(0_u64)).map(|r| r.unwrap().1).collect();
    let (mut journal, _) = LogJournal::open(Arc::new(MemoryLog::new())).unwrap();
    for (i, record) in records.iter().enumerate() {
        journal.persist(Lsn(u64::try_from(i).unwrap() + 1_u64), record).unwrap();
    }
    journal.sync().unwrap();
    let recovered = turborel_replication::Recovered { state: None, first: Lsn(0), records: records.clone() };
    let vr = Box::new(Vr::restart(&[ReplicaId(1_u32)], ReplicaId(1_u32), options(), recovered, 1_u64).unwrap());
    let wait = Arc::new(Blocking { most: Duration::from_millis(1_u64) });
    let parts = Parts {
        consensus: vr,
        journal: Box::new(journal),
        first: Lsn(0),
        records,
        retain_bytes: u64::MAX,
        wait,
        ..parts(Box::new(Scripted(Vec::new())), Arc::new(Stepping), Arc::new(Unwatched))
    };
    let driver = Driver::new(parts);
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let (db, holds) = opened(&fs, &driver);
    driver.join(holds).unwrap();
    let job = db.add_job(Arc::new(DriverJob::new(Arc::clone(&driver), Duration::from_millis(10_u64)))).unwrap();
    for _ in 0_u32..100_u32 {
        db.request_job(job, Duration::ZERO);
        db.run_due().unwrap();
    }
    assert_eq!(driver.serving(), Some(0_u64));
    assert_eq!(rows(&db.session("APP"), "SELECT K FROM T").unwrap(), ["1"]);
    let log = driver.log();
    let last = log.read_at(Lsn(log.end().0 - 1_u64)).unwrap().unwrap();
    let decoded = turborel::log::record::Record::decode(&last, &turborel::value::types::TypeRegistry::default()).unwrap();
    assert!(matches!(decoded.change, turborel::log::record::Change::Abort), "{decoded:?}");
    drop(s);
}

/// A consensus that answers each tick with the next of its outputs.
struct Scripted(Vec<Output>);

impl Consensus for Scripted {
    fn name(&self) -> &'static str {
        "scripted"
    }
    fn step(&mut self, input: Input, out: &mut Vec<Output>) {
        if input == Input::Tick && !self.0.is_empty() {
            out.push(self.0.remove(0));
        }
    }
}

/// A standby that holds nothing and counts its calls.
#[derive(Default)]
struct Counted(AtomicU64);

impl Standby for Counted {
    fn replay(&self, records: &mut dyn Iterator<Item = Result<(Lsn, Vec<u8>)>>) -> Result<Lsn> {
        self.0.fetch_add(1_u64, Ordering::SeqCst);
        Ok(records.last().transpose()?.map_or(Lsn(0_u64), |(at, _)| at))
    }
    fn promote(&self, _log: Arc<dyn Log>) -> Result<()> {
        self.0.fetch_add(1_u64, Ordering::SeqCst);
        Ok(())
    }
    fn depose(&self) -> Result<()> {
        Ok(())
    }
    fn export(&self) -> Result<Box<dyn turborel::db::StateSource>> {
        Err(SqlError::new(Code::Api, "[TOOL] nothing to export"))
    }
    fn import(&self, _: Box<dyn turborel::db::StateSource>) -> Result<Lsn> {
        Err(SqlError::new(Code::Api, "[TOOL] nothing to import"))
    }
    fn reopen(&self) -> Result<Lsn> {
        self.0.fetch_add(1_u64, Ordering::SeqCst);
        Ok(Lsn(0_u64))
    }
}

/// A driver of `outputs`, one a tick, told to `told`.
fn scripted(outputs: Vec<Output>, told: &Arc<Told>) -> Arc<Driver> {
    Driver::new(parts(Box::new(Scripted(outputs)), Arc::new(Stepping), watched(told)))
}

/// Verifies: [DESIGN] RULES.md D57: the driver carries out its consensus's
/// outputs as the crate's contract says: a role begun promotes its
/// database once it holds the log, and an end of it deposes and opens it
/// again; events are told; an outcome of a record it did not append, a
/// state transfer asked for, a primary whose database lacks the log, or a
/// frame its transport refuses, fails it (SQLCODE -910), after which it
/// takes nothing; an append while it serves no primary is refused.
#[test]
fn a_driver_carries_out_its_consensus() {
    let told = Arc::new(Told::default());
    let standby = Counted::default();
    let event = Output::Event(ReplicaEvent::ViewChanged { view: 1_u64, primary: ReplicaId(1_u32) });
    let driver = scripted(vec![Output::Role(Role::Primary { view: 1_u64 }), Output::Role(Role::Backup), event], &told);
    assert_eq!(driver.log().append(b"r").err().map(|e| e.code), Some(Code::Api), "not serving");
    for _ in 0_u32..3_u32 {
        driver.turn(&standby).unwrap();
    }
    assert_eq!(standby.0.load(Ordering::SeqCst), 2_u64, "promoted, then opened again");
    assert_eq!(told.0.lock().unwrap().len(), 3_usize, "serving, stopped, and the event");

    let failing = [
        Output::Outcome { at: Lsn(1_u64), kept: Some(true) },
        Output::FetchState { from: ReplicaId(1_u32) },
        Output::Send { to: ReplicaId(2_u32), frame: Vec::new() },
    ];
    for output in failing {
        let failed = scripted(vec![output.clone()], &told);
        assert_eq!(failed.turn(&standby).err().map(|e| e.code), Some(Code::Api), "{output:?}");
        assert!(failed.failed().is_some());
        assert_eq!(failed.turn(&standby).err().map(|e| e.code), Some(Code::Api), "it takes nothing after");
    }
    let behind =
        scripted(vec![Output::Persist { at: Lsn(1_u64), record: b"r".to_vec() }, Output::Role(Role::Primary { view: 0_u64 })], &told);
    behind.turn(&standby).unwrap();
    assert_eq!(behind.turn(&standby).err().map(|e| e.code), Some(Code::Api), "its database lacks the log");
    let halted = scripted(Vec::new(), &told);
    halted.halt(SqlError::new(Code::Io, "[TOOL] stopped"));
    assert_eq!((halted.serving(), halted.log().sync(Lsn(1_u64)).err().map(|e| e.code)), (None, Some(Code::Io)));
}

/// Verifies: [DESIGN] RULES.md D57: the built-in seams: an inbox gives its
/// frames in order, once, and lets them go; ticks of an hour are none yet;
/// the blocking wait, off the thread carrying out the driver's effects,
/// waits for the driver to move on and returns; a watch that tells
/// nothing; the group's log storage keeps no log in the directory.
#[test]
fn the_built_in_seams() {
    let inbox = Inbox::default();
    inbox.push(ReplicaId(2_u32), b"a".to_vec());
    inbox.push(ReplicaId(3_u32), b"b".to_vec());
    assert_eq!(inbox.take(), [(ReplicaId(2_u32), b"a".to_vec()), (ReplicaId(3_u32), b"b".to_vec())]);
    inbox.push(ReplicaId(2_u32), b"c".to_vec());
    inbox.wait(Duration::from_millis(1_u64));
    inbox.clear();
    assert_eq!(inbox.take(), []);
    inbox.wait(Duration::from_millis(1_u64));

    let ticks = Ticks::every(Duration::from_secs(3600_u64));
    assert_eq!((ticks.take(), ticks.interval()), (0_u64, Duration::from_secs(3600_u64)));

    let unwatched = Unwatched;
    unwatched.committed((Lsn(0_u64), Lsn(1_u64)), &[]);
    unwatched.serving(0_u64);
    unwatched.stopped();
    unwatched.outcome(Lsn(1_u64), b"r", Some(true));
    unwatched.event(&ReplicaEvent::AppendRefused);
    let driver =
        Driver::new(parts(Box::new(Scripted(Vec::new())), Arc::new(Blocking { most: Duration::from_millis(1_u64) }), Arc::new(Unwatched)));
    Blocking { most: Duration::from_millis(1_u64) }.wait(&driver).unwrap();
    assert!(format!("{driver:?}").contains("Driver"));

    let storage = GroupLogs::of(Arc::clone(&driver));
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let formats = turborel::db::FileFormats::of(turborel_test_support::database::crc32c());
    let place = StorePlace::new(&fs, Path::new("/db"), &formats);
    assert!(!storage.exists(&place).unwrap());
    assert_eq!(storage.open(&place).unwrap().map(|o| o.log.end()), Some(Lsn(1_u64)));
    assert_eq!(storage.create(&place).unwrap().end(), Lsn(1_u64));
    assert_eq!(storage.name(), "group");
}
