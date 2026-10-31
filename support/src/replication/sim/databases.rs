//! Real databases in the simulator (RULES.md D57): each replica of the
//! [`World`] keeps a turborel `Database`, in memory, which carries out its
//! replica's effects through the core's `Standby` seam alone: it replays
//! the records delivered, following; it is promoted, over a [`SimLog`] of
//! its replica, when its replica serves; deposed and opened again from
//! what it knew committed when its replica stops; lost when it crashes.
//! The primary's database takes SQL at random, its sessions' transactions
//! spanning moments, so the records a backup is given end inside
//! transactions, a view change leaves some open, and a commit waits on
//! the group as the world moves on. At the end every backup must hold what
//! the primary holds: the same rows and the same catalog.

use std::sync::{Arc, Mutex};

use turborel::db::{Database, Session, Standby};
use turborel::error::Code;
use turborel::log::{Log, Lsn, MemoryLog};
use turborel::value::Value;

use super::super::group::Make;
use super::{Effect, Plan, Report, Rng, SimLog, Violation, World};
use crate::database::{contents, number, rows};

/// The users whose sessions a database's contents are read through.
const READERS: [&str; 1] = ["APP"];
/// The schema the primary's database defines first.
const SCHEMA: &str = "CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER)";
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
    /// Backups compared with the primary at the end.
    pub compared: usize,
}

/// One replica's database.
struct Replica {
    db: Database,
    /// Whether it serves as primary.
    serving: bool,
    /// The last record replayed.
    replayed: Lsn,
    /// Its session with a transaction open, serving.
    open: Option<Session>,
}

impl Replica {
    /// A new database, in memory, empty: its jobs run only when asked, and
    /// it takes no checkpoint, so its work is the simulation's alone; it
    /// logs to a log of its own until promoted, at which a change it would
    /// log while following is refused.
    fn new() -> Replica {
        let db = Database::new();
        db.set_setting("JOBS.POLICY", &Value::Char("MANUAL".into())).unwrap();
        db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
        db.set_log(Arc::new(MemoryLog::new())).unwrap();
        Replica { db, serving: false, replayed: Lsn(0), open: None }
    }

    /// Replays `records`, the next after those it holds, following.
    fn replay(&mut self, records: Vec<(Lsn, Vec<u8>)>) -> Result<(), String> {
        if records.first().is_some_and(|&(at, _)| at.0 != self.replayed.0 + 1) {
            return Err(format!("records from {:?} given after {:?}", records.first().map(|r| r.0), self.replayed));
        }
        let given = records.last().map(|&(at, _)| at);
        let taken = Standby::replay(&self.db, &mut records.into_iter().map(Ok)).map_err(|e| format!("its replay failed: {e}"))?;
        if given.is_some_and(|last| last != taken) {
            return Err(format!("its replay took through {taken:?}, given through {given:?}"));
        }
        self.replayed = given.unwrap_or(self.replayed);
        Ok(())
    }
}

/// A group of replicas, each with a real database, in a world.
pub struct Replicas {
    world: Arc<Mutex<World>>,
    plan: Plan,
    members: Vec<Replica>,
    rng: Rng,
    /// The next key the primary's database inserts.
    next: u64,
    report: DatabaseReport,
}

impl Replicas {
    /// A group of `plan` from `seed`, each replica made by `make`, each
    /// database new.
    pub fn new(seed: u64, plan: &Plan, make: &Make) -> Replicas {
        let world = World::new(seed, *plan, Arc::clone(make));
        let members = (0..world.replicas()).map(|_| Replica::new()).collect();
        let rng = Rng::new(seed.rotate_left(32));
        Replicas { world: Arc::new(Mutex::new(world)), plan: *plan, members, rng, next: 0, report: DatabaseReport::default() }
    }

    /// The world, locked.
    fn world(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.lock().unwrap()
    }

    /// The first violation found, if one was.
    fn check(&self) -> Result<(), Violation> {
        self.world().failed().map_or(Ok(()), |why| Err(why.clone()))
    }

    /// Runs the plan's moments: at each, the world moves on, its effects
    /// are carried out, and the primary's database perhaps acts.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn run(&mut self) -> Result<(), Violation> {
        while !self.world().over() {
            self.moments(1)?;
            let now = self.world().now();
            if self.plan.appending(now) && self.rng.chance(self.plan.append) {
                self.act()?;
            }
        }
        self.carry_out()
    }

    /// Moves the world on `count` moments, carrying out the effects.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn moments(&mut self, count: u64) -> Result<(), Violation> {
        for _ in 0..count {
            self.world().moment()?;
            self.carry_out()?;
        }
        Ok(())
    }

    /// Replica `index` crashes now.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn crash(&mut self, index: usize) -> Result<(), Violation> {
        self.world().crash(index);
        self.carry_out()
    }

    /// The replica whose database serves as primary, if one does.
    pub fn primary(&self) -> Option<usize> {
        self.members.iter().position(|r| r.serving)
    }

    /// The database of replica `index`.
    pub fn database(&self, index: usize) -> &Database {
        &self.members[index].db
    }

    /// The effects queued carried out, in order, until none is queued: a
    /// promotion's sync may queue more.
    fn carry_out(&mut self) -> Result<(), Violation> {
        loop {
            let effects = self.world().take_effects();
            if effects.is_empty() {
                return self.check();
            }
            for (index, effect) in effects {
                self.apply(index, effect).map_err(|why| self.world().fail(&format!("replica {}: {why}", index + 1)))?;
            }
        }
    }

    /// Carries out `effect` on the database of replica `index`.
    fn apply(&mut self, index: usize, effect: Effect) -> Result<(), String> {
        match effect {
            Effect::Deliver(records) => self.members[index].replay(records),
            Effect::Serve(view) => self.promote(index, view),
            Effect::Stop(records) => {
                self.members[index].db.depose().map_err(|e| e.to_string())?;
                self.replace(index);
                self.members[index].replay(records)
            }
            Effect::Crash => {
                self.replace(index);
                Ok(())
            }
        }
    }

    /// The database of replica `index` replaced by a new one: deposed and
    /// opened again, or lost in a crash.
    fn replace(&mut self, index: usize) {
        let old = std::mem::replace(&mut self.members[index], Replica::new());
        if old.open.is_some() {
            self.report.interrupted += 1;
        }
    }

    /// Promotes the database of replica `index`, serving `view`, over its
    /// replica's log. Its promotion fails only if its replica stops serving
    /// meanwhile, or crashes.
    fn promote(&mut self, index: usize, view: u64) -> Result<(), String> {
        let log = SimLog::new(&self.world, index);
        let life = self.world().lives(index);
        match self.members[index].db.promote({
            let log: Arc<dyn Log> = Arc::new(log);
            log
        }) {
            Ok(()) => {
                self.members[index].serving = true;
                self.report.promoted += u64::from(view > 0);
                Ok(())
            }
            Err(e) => {
                let world = self.world();
                let still = world.serving(index) == Some(view) && world.lives(index) == life;
                if still { Err(format!("its promotion failed while it serves: {e}")) } else { Ok(()) }
            }
        }
    }

    /// The primary's database acts once, at random.
    fn act(&mut self) -> Result<(), Violation> {
        let Some(index) = self.primary() else { return Ok(()) };
        let db = self.members[index].db.clone();
        if rows(&db.session("APP"), "SELECT COUNT(*) FROM T").is_err() {
            let _ = db.schema(SCHEMA);
            return self.carry_out();
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
                let session = self.members[index].open.get_or_insert_with(|| db.session("APP"));
                for statement in sql {
                    let _ = session.run(&statement);
                }
            }
        }
        self.carry_out()
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
        let Some(session) = self.members[index].open.take() else { return };
        if !commit {
            let _ = session.rollback();
            return;
        }
        match session.commit() {
            Ok(()) => self.report.committed += 1,
            Err(e) => self.report.in_doubt += u64::from(e.code == Code::InDoubt),
        }
    }

    /// The end: the primary's open transaction rolled back, every backup
    /// holds the committed log and what the primary holds, its rows and its
    /// catalog; the outcomes told agree with the log.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn compare(&mut self) -> Result<DatabaseReport, Violation> {
        let seed = self.world().seed();
        let Some(primary) = self.primary() else { return Err(format!("seed {seed}: no database serves after the calm")) };
        if let Some(session) = self.members[primary].open.take() {
            let _ = session.rollback();
        }
        let expected = contents(&self.members[primary].db, &READERS);
        let committed = u64::try_from(self.world().committed_log().len()).unwrap();
        for (index, replica) in self.members.iter().enumerate().filter(|&(i, _)| i != primary) {
            if replica.replayed.0 != committed {
                return Err(format!("seed {seed}: replica {} replayed {} records of {committed}", index + 1, replica.replayed.0));
            }
            let found = contents(&replica.db, &READERS);
            if let Some((a, b)) = found.iter().zip(&expected).find(|(a, b)| a != b) {
                return Err(format!("seed {seed}: replica {} holds {a}, its primary {b}", index + 1));
            }
            if found.len() != expected.len() {
                return Err(format!("seed {seed}: replica {} holds {} lines, its primary {}", index + 1, found.len(), expected.len()));
            }
            self.report.compared += 1;
        }
        let replication = self.world().finish()?;
        self.report.replication = replication;
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
    let mut replicas = Replicas::new(seed, plan, make);
    replicas.run()?;
    replicas.compare()
}
