//! Real databases in the simulator (RULES.md D57): each replica keeps a
//! turborel `Database` in a directory of a file system in memory, kept
//! across its crashes, whose log is its driver's (`GroupLogs`). Opened to
//! follow (`Database::open_following`), it recovers from its last
//! checkpoint alone, and its driver delivers it the committed records
//! after; promoted when its replica serves; deposed and opened again in
//! place when its replica stops; lost when it crashes, and opened again
//! from its files when its replica is made again. The primary's database
//! takes SQL at random, its sessions' transactions spanning moments, so
//! the records a backup is given end inside transactions, a view change
//! leaves some open, and a commit waits on the group as the world moves
//! on; and the databases take checkpoints at random, the backups' as
//! following databases; and the primary's database is asked
//! reconfigurations (`Database::reconfigure`), a spare joining as a
//! member leaves. At the end every backup must hold what the primary
//! holds: the same rows and the same catalog; and every database whose
//! replica left the group is retired, serving nothing.

use std::path::Path;
use std::sync::{Arc, Mutex};

use turborel::db::{Database, OpenOptions, Session, Standby};
use turborel::disk::io::FileSystem;
use turborel::error::Code;
use turborel::log::Lsn;
use turborel::value::Value;
use turborel_replication::{Driver, GroupLogs, PageDifferences, StateTransfer};

use super::super::group::Make;
use super::{Host, Plan, Report, Rng, Runner, Violation, serving_primary};
use crate::database::{contents, number, rows};
use crate::volatile::VolatileFileSystem;

/// The users whose sessions a database's contents are read through.
const READERS: [&str; 1] = ["APP"];
/// The directory of each replica's database.
const DIR: &str = "/db";
/// The schema the primary's database defines first.
const SCHEMA: &str = "CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER)";
/// Per mille, at each moment, that a database up takes a checkpoint.
const CHECKPOINTS: u32 = 10;
/// Per mille, at each moment, that a reader begins on a backup's
/// database, holding its snapshot a while.
const READS: u32 = 20;
/// The most moments a reader on a backup holds its snapshot.
const READ_MOMENTS: u64 = 60;
/// The kinds of action the primary's database takes, by weight: insert,
/// update, delete, a statement undone, commit, rollback, a table defined
/// in a transaction, an index, a setting.
const ACTIONS: [(Action, u64); 9] = [
    (Action::Insert, 14),
    (Action::Update, 4),
    (Action::Delete, 2),
    (Action::Undone, 2),
    (Action::Commit, 10),
    (Action::Rollback, 2),
    (Action::Table, 1),
    (Action::Index, 1),
    (Action::Setting, 1),
];

/// An action of the primary's database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Insert,
    Update,
    Delete,
    Undone,
    Commit,
    Rollback,
    Table,
    Index,
    Setting,
}

/// What a run of real databases did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DatabaseReport {
    /// What replication did.
    pub replication: Report,
    /// Transactions the primaries' databases committed.
    pub committed: u64,
    /// Commits left in doubt as their primary stopped serving or crashed.
    pub in_doubt: u64,
    /// Transactions open on a primary's database as it stopped or crashed.
    pub interrupted: u64,
    /// Databases promoted in a view after the first.
    pub promoted: u64,
    /// Checkpoints taken by following databases.
    pub following_checkpoints: u64,
    /// Databases opened again from their files after a crash.
    pub reopened: u64,
    /// Reads served by backups, each holding its snapshot a while.
    pub backup_reads: u64,
    /// Reconfigurations asked of a primary's database that were done.
    pub reconfigured: u64,
    /// Backups compared with the primary at the end.
    pub compared: usize,
}

/// Each replica's file system, kept across its crashes, its database
/// while it is up, and its session with a transaction open, serving, with
/// the view it was opened in: a handle, shared with the run's ground.
#[derive(Default, Clone)]
pub struct Files(Arc<Mutex<Kept>>);

/// What `Files` keeps.
#[derive(Default)]
struct Kept {
    /// Each replica's file system, which loses what was not synced when
    /// the replica crashes.
    systems: Vec<Arc<VolatileFileSystem>>,
    databases: Vec<Option<Database>>,
    open: Vec<Option<(Session, u64)>>,
    /// A reader on each backup's database, holding its snapshot until a
    /// moment.
    readers: Vec<Option<(Session, u64)>>,
    /// Databases opened again after a crash.
    opened: u64,
    /// Transactions open as their database stopped serving or crashed.
    interrupted: u64,
}

impl Files {
    /// What it keeps, locked.
    fn kept(&self) -> std::sync::MutexGuard<'_, Kept> {
        self.0.lock().unwrap()
    }

    /// Replica `index`'s database, while it is up.
    fn database(&self, index: usize) -> Option<Database> {
        self.kept().databases.get(index).cloned().flatten()
    }

    /// Replica `index`'s session open, taken.
    fn take_open(&self, index: usize) -> Option<(Session, u64)> {
        self.kept().open.get_mut(index).and_then(Option::take)
    }

    /// Keeps `session` as replica `index`'s open, of `view`, while its
    /// database is up.
    fn keep_open(&self, index: usize, session: Session, view: u64) {
        let mut kept = self.kept();
        if kept.databases[index].is_some() {
            kept.open[index] = Some((session, view));
        }
    }
}

impl Host for Files {
    fn open(&mut self, index: usize, driver: &Arc<Driver>) -> Result<(Arc<dyn Standby>, Lsn), Violation> {
        let fs: Arc<dyn FileSystem> = {
            let mut kept = self.kept();
            kept.opened += u64::from(kept.systems.len() > index);
            kept.grown(index);
            let system: Arc<VolatileFileSystem> = Arc::clone(&kept.systems[index]);
            system
        };
        let db = Database::new();
        let opened = (|| {
            db.set_setting("JOBS.POLICY", &Value::Char("MANUAL".into()))?;
            db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX))?;
            db.set_log_storage(Box::new(GroupLogs::of(Arc::clone(driver))))?;
            db.open_following(&fs, Path::new(DIR), &OpenOptions::default())
        })();
        let holds = opened.map_err(|e| format!("replica {}: its database did not open: {e}", index + 1))?;
        self.kept().databases[index] = Some(db.clone());
        Ok((Arc::new(db), holds))
    }

    /// Its database and sessions lost, its files as the syncs left them.
    fn crashed(&mut self, index: usize) {
        let mut kept = self.kept();
        kept.databases[index] = None;
        kept.readers[index] = None;
        if kept.open[index].take().is_some() {
            kept.interrupted += 1;
        }
        kept.systems[index].crash().unwrap();
    }

    fn transfer(&mut self, index: usize) -> Arc<dyn StateTransfer> {
        let mut kept = self.kept();
        kept.grown(index);
        let system: Arc<VolatileFileSystem> = Arc::clone(&kept.systems[index]);
        Arc::new(PageDifferences::new(system, Path::new(DIR)))
    }
}

impl Kept {
    /// Grown to hold replica `index`.
    fn grown(&mut self, index: usize) {
        while self.systems.len() <= index {
            self.systems.push(Arc::new(VolatileFileSystem::new()));
            self.databases.push(None);
            self.open.push(None);
            self.readers.push(None);
        }
    }
}

/// A group of replicas, each with a real database, in a world.
pub struct Replicas {
    runner: Runner<Files>,
    rng: Rng,
    /// The next key the primary's database inserts.
    next: u64,
    report: DatabaseReport,
}

impl Replicas {
    /// A group of `plan` from `seed`, each replica made by `make`, each
    /// database new.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn new(seed: u64, plan: &Plan, make: &Make) -> Result<Replicas, Violation> {
        let runner = Runner::new(seed, *plan, make, Files::default())?;
        Ok(Replicas { runner, rng: Rng::new(seed.rotate_left(32)), next: 0, report: DatabaseReport::default() })
    }

    /// Runs the plan's moments: at each, the group moves on, the primary's
    /// database perhaps acts, a database perhaps takes a checkpoint, and a
    /// reader perhaps begins on a backup's, those whose time came ended.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn run(&mut self) -> Result<(), Violation> {
        while !self.runner.world().over() {
            self.moments(1)?;
            let now = self.runner.world().now();
            if self.runner.plan.appending(now) && self.rng.chance(self.runner.plan.append) {
                self.act()?;
            }
            if self.runner.plan.appending(now) && self.rng.chance(CHECKPOINTS) {
                self.checkpoint();
            }
            self.end_reads(now);
            if self.runner.plan.appending(now) && self.rng.chance(READS) {
                self.read()?;
            }
            if !self.runner.world().calm() && self.rng.chance(self.runner.plan.faults.reconfigure) {
                self.reconfigure()?;
            }
        }
        Ok(())
    }

    /// The primary's database is asked a reconfiguration, if one serves,
    /// asked none in its view, and a spare waits: a spare joins, a member
    /// leaves; the call waits on the group, moments passing within it.
    fn reconfigure(&mut self) -> Result<(), Violation> {
        let Some((index, view)) = serving_primary(&self.runner) else { return Ok(()) };
        let Some(db) = self.database(index) else { return Ok(()) };
        let next = {
            let mut world = self.runner.world();
            let (id, spare) = (world.id(index), world.spare_to_add());
            world.epochs.propose(&mut self.rng, (id, view), spare)
        };
        let Some(next) = next else { return Ok(()) };
        let members: Vec<String> = next.iter().map(|id| format!("{}=node{}:1", id.0, id.0)).collect();
        // Deposed meanwhile, the call fails: the record may be committed
        // all the same, which the epochs entered tell.
        if db.reconfigure(&members.join(",")).is_ok() {
            self.report.reconfigured += 1;
        }
        self.runner.world().check()
    }

    /// A reader begins on a backup's database, at random, if none is on
    /// it: its read must be served, and its snapshot is held a while,
    /// during which the database's following checkpoints wait. A database
    /// whose replica left the group serves no read.
    fn read(&mut self) -> Result<(), Violation> {
        let index = self.rng.index(self.replicas());
        let (Some(db), Some(driver)) = (self.database(index), self.runner.driver(index)) else { return Ok(()) };
        if driver.serving().is_some()
            || self.runner.host.kept().readers.get(index).is_some_and(Option::is_some)
            || self.runner.world().left(index)
        {
            return Ok(());
        }
        let session = db.session("APP");
        match rows(&session, "SELECT COUNT(*) FROM T") {
            Ok(_) => {
                self.report.backup_reads += 1;
                let until = self.runner.world().now() + self.rng.between(1, READ_MOMENTS);
                let mut kept = self.runner.host.kept();
                if kept.databases[index].is_some() {
                    kept.readers[index] = Some((session, until));
                }
                Ok(())
            }
            // A backup takes no change (SQLCODE -917), but serves reads.
            Err(Code::TransactionState) => Err(self.runner.world().fail(&format!("replica {}: a backup refused a read", index + 1))),
            // Before the primary's schema reached it, there is nothing to
            // read; opened again meanwhile, its sessions begun before are
            // refused.
            Err(_) => Ok(()),
        }
    }

    /// The readers whose time came end their transactions.
    fn end_reads(&mut self, now: u64) {
        let mut kept = self.runner.host.kept();
        for reader in &mut kept.readers {
            if reader.as_ref().is_some_and(|(_, until)| *until <= now)
                && let Some((session, _)) = reader.take()
            {
                let _ = session.commit();
            }
        }
    }

    /// Moves the group on `count` moments.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn moments(&mut self, count: u64) -> Result<(), Violation> {
        for _ in 0..count {
            let promoted = self.runner.primary();
            self.runner.moment()?;
            self.forget_stopped();
            let now = self.runner.primary();
            self.report.promoted += u64::from(now.is_some() && now != promoted && self.view(now) > 0);
        }
        Ok(())
    }

    /// The view replica `index` serves, 0 if none.
    fn view(&self, index: Option<usize>) -> u64 {
        index.and_then(|i| self.runner.driver(i)).and_then(|d| d.serving()).unwrap_or(0)
    }

    /// Replica `index` crashes now.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn crash(&mut self, index: usize) -> Result<(), Violation> {
        self.runner.crash(index)?;
        self.forget_stopped();
        Ok(())
    }

    /// The sessions of databases no longer serving the view they were
    /// opened in let go: their transactions were interrupted.
    fn forget_stopped(&mut self) {
        for index in 0..self.replicas() {
            let view = self.runner.driver(index).and_then(|d| d.serving());
            let mut kept = self.runner.host.kept();
            if kept.open.get(index).is_some_and(|open| open.as_ref().is_some_and(|(_, opened)| view != Some(*opened))) {
                kept.open[index] = None;
                kept.interrupted += 1;
            }
        }
    }

    /// How many replicas the group has.
    fn replicas(&self) -> usize {
        self.runner.world().replicas()
    }

    /// Where each replica's consensus stands, for a violation.
    fn standings(&self) -> String {
        (0..self.replicas())
            .map(|i| self.runner.driver(i).map_or_else(|| format!("{}: down", i + 1), |d| format!("{}: {}", i + 1, d.describe())))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The replica whose database serves as primary, if one does.
    pub fn primary(&self) -> Option<usize> {
        self.runner.primary()
    }

    /// The database of replica `index`, while it is up.
    pub fn database(&self, index: usize) -> Option<Database> {
        self.runner.host.database(index)
    }

    /// A database up, at random, takes a checkpoint, if it can: holding
    /// the checkpoints' turn, it serves no state transfer meanwhile, as
    /// an export would wait on the turn on a node.
    fn checkpoint(&mut self) {
        let index = self.rng.index(self.replicas());
        let Some(db) = self.database(index) else { return };
        let following = self.runner.driver(index).is_some_and(|d| d.serving().is_none());
        let taken = db.checkpoint();
        let id = self.runner.world().id(index);
        self.runner
            .world()
            .note(id, &format!("checkpoint {}", taken.as_ref().map_or_else(|e| format!("failed: {e}"), |()| "taken".to_owned())));
        if taken.is_ok() && following {
            self.report.following_checkpoints += 1;
        }
    }

    /// The primary's database acts once, at random, unless it was asked a
    /// reconfiguration in its view: it takes no change after.
    fn act(&mut self) -> Result<(), Violation> {
        let Some((index, _)) = serving_primary(&self.runner) else { return Ok(()) };
        let Some(db) = self.database(index) else { return Ok(()) };
        if rows(&db.session("APP"), "SELECT COUNT(*) FROM T").is_err() {
            let _ = db.schema(SCHEMA);
            return self.runner.world().check();
        }
        let action = pick(&mut self.rng);
        self.next += 1;
        let key = self.next;
        match action {
            Action::Commit | Action::Rollback => self.end(index, action == Action::Commit),
            Action::Index => {
                let _ = db.create_index(&format!("T_V{key}"), "APP.T", &["V"], "btree");
            }
            Action::Setting => {
                let _ = db.set_setting("OPTIMIZER.MAX_PASSES", &number(key % 5 + 1));
            }
            Action::Insert | Action::Update | Action::Delete | Action::Undone | Action::Table => {
                let sql = self.statement(action, key);
                let view = self.view(Some(index));
                let (session, opened) = self.runner.host.take_open(index).unwrap_or_else(|| (db.session("APP"), view));
                for statement in sql {
                    let _ = session.run(&statement);
                }
                self.runner.host.keep_open(index, session, opened);
            }
        }
        self.runner.world().check()
    }

    /// The statements of `action`, its key `key`.
    fn statement(&mut self, action: Action, key: u64) -> Vec<String> {
        let some = self.rng.below(key);
        match action {
            Action::Insert => vec![format!("INSERT INTO T VALUES ({key}, {key})")],
            Action::Update => vec![format!("UPDATE T SET V = V + 1 WHERE K = {some}")],
            Action::Delete => vec![format!("DELETE FROM T WHERE K = {some}")],
            Action::Undone => vec![format!("INSERT INTO T VALUES ({some}, 0)")],
            Action::Table => vec![format!("CREATE TABLE U{key} (K INTEGER)"), format!("INSERT INTO U{key} VALUES ({key})")],
            Action::Commit | Action::Rollback | Action::Index | Action::Setting => Vec::new(),
        }
    }

    /// Ends the open transaction of replica `index`'s database, if one is
    /// open: commits it, or rolls it back.
    fn end(&mut self, index: usize, commit: bool) {
        let Some((session, _)) = self.runner.host.take_open(index) else { return };
        if !commit {
            let _ = session.rollback();
            return;
        }
        match session.commit() {
            Ok(()) => self.report.committed += 1,
            Err(e) => self.report.in_doubt += u64::from(e.code == Code::InDoubt),
        }
    }

    /// The primary's database, holding `expected`, held to what a database
    /// made from the committed log alone holds: a fresh one, following,
    /// given every record the checker saw committed, in order.
    ///
    /// # Errors
    ///
    /// The database could not be made or given the records; the primary
    /// holds other than it.
    fn checked_against_committed_log(&self, primary: usize, expected: &[String]) -> Result<(), Violation> {
        let driver = self.runner.driver(primary).ok_or_else(|| "the primary's driver is gone".to_owned())?;
        let records: Vec<(Lsn, Vec<u8>)> =
            self.runner.world().committed_log().iter().enumerate().map(|(i, r)| (Lsn(u64::try_from(i).unwrap() + 1), r.clone())).collect();
        let fs: Arc<dyn FileSystem> = Arc::new(VolatileFileSystem::new());
        let db = Database::new();
        let made = (|| {
            db.set_setting("JOBS.POLICY", &Value::Char("MANUAL".into()))?;
            db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX))?;
            db.set_log_storage(Box::new(GroupLogs::of(Arc::clone(&driver))))?;
            db.open_following(&fs, Path::new(DIR), &OpenOptions::default())?;
            Standby::replay(&db, &mut records.into_iter().map(Ok))
        })();
        made.map_err(|e| format!("a database made from the committed log: {e}"))?;
        let canonical = contents(&db, &READERS);
        if expected != canonical {
            let of_primary: Vec<&String> = expected.iter().filter(|line| !canonical.contains(line)).collect();
            let of_log: Vec<&String> = canonical.iter().filter(|line| !expected.contains(line)).collect();
            let why = format!(
                "the primary {} differs from a database made from the committed log: it alone holds {of_primary:?}, the log alone {of_log:?}",
                primary + 1
            );
            return Err(self.runner.world().fail(&why));
        }
        Ok(())
    }

    /// The end: the primary's open transaction rolled back; every backup
    /// holds the committed log and what the primary holds, its rows and its
    /// catalog, both what a database made from the committed log alone
    /// holds; every database whose replica left the group is retired,
    /// refusing a read (SQLCODE -917); the outcomes told agree with the
    /// log.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn compare(&mut self) -> Result<DatabaseReport, Violation> {
        let Some(primary) = self.primary() else {
            let standings = self.standings();
            return Err(self.runner.world().fail(&format!("no database serves after the calm; the replicas: {standings}")));
        };
        if let Some((session, _)) = self.runner.host.take_open(primary) {
            let _ = session.rollback();
        }
        self.end_reads(u64::MAX);
        let expected = contents(&self.database(primary).unwrap(), &READERS);
        self.checked_against_committed_log(primary, &expected)?;
        let committed = u64::try_from(self.runner.world().committed_log().len()).unwrap();
        for index in (0..self.replicas()).filter(|&i| i != primary && self.runner.world().joined(i)) {
            if self.runner.world().left(index) {
                let refused = self.database(index).map(|db| rows(&db.session("APP"), "SELECT COUNT(*) FROM T").err());
                if !self.runner.world().removed_told(index) || refused != Some(Some(Code::TransactionState)) {
                    let why = format!("replica {} left the group but its database is not retired: {refused:?}", index + 1);
                    return Err(self.runner.world().fail(&why));
                }
                continue;
            }
            let holds = self.runner.driver(index).map_or(0, |d| d.holds().0);
            if holds != committed {
                let standing = self.runner.driver(index).map_or_else(|| "down".to_owned(), |d| d.describe());
                let why = format!("replica {} holds {holds} records of {committed}; its replica: {standing}", index + 1);
                return Err(self.runner.world().fail(&why));
            }
            let found = contents(&self.database(index).unwrap(), &READERS);
            if found != expected {
                let of_replica: Vec<&String> = found.iter().filter(|line| !expected.contains(line)).collect();
                let of_primary: Vec<&String> = expected.iter().filter(|line| !found.contains(line)).collect();
                let standing = self.runner.driver(index).map_or_else(|| "down".to_owned(), |d| d.describe());
                let why = format!(
                    "replica {} differs from its primary {}: it alone holds {of_replica:?}, the primary alone {of_primary:?}; its replica: {standing}",
                    index + 1,
                    primary + 1
                );
                return Err(self.runner.world().fail(&why));
            }
            self.report.compared += 1;
        }
        self.report.reopened = self.runner.host.kept().opened;
        self.report.interrupted = self.runner.host.kept().interrupted;
        self.report.replication = self.runner.world().finish()?;
        Ok(self.report)
    }
}

/// An action at random, by the weights of `ACTIONS`.
fn pick(rng: &mut Rng) -> Action {
    let total: u64 = ACTIONS.iter().map(|&(_, weight)| weight).sum();
    let mut left = rng.below(total);
    for (action, weight) in ACTIONS {
        if left < weight {
            return action;
        }
        left -= weight;
    }
    Action::Insert
}

/// Runs `plan` from `seed` with real databases, each replica made by
/// `make`, then compares every backup with the primary.
///
/// # Errors
///
/// The first violation found, naming the seed.
pub fn simulate_databases(seed: u64, plan: &Plan, make: &Make) -> Result<DatabaseReport, Violation> {
    let mut replicas = Replicas::new(seed, plan, make)?;
    replicas.run()?;
    replicas.compare()
}
