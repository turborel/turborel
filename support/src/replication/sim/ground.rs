//! The drivers' side of the simulator: each replica of the [`World`] up
//! has the crate's real driver (`turborel_replication::Driver`) over its
//! consensus, its journal on the world's disk, its link and inbox, and its
//! database, whatever the [`Host`] makes it: a model (`sim.rs`) or a real
//! turborel database (`sim/databases.rs`). Each whole moment the world
//! moves on, replicas crash and are made again, then each driver takes a
//! turn, as its job would.
//!
//! A database's own call that waits on its log, a sync waiting for its
//! commit, waits as the driver's `Wait` says: here it moves the world on a
//! moment within the call, replicas due made again, every driver taking
//! its inputs alone; what they ask of their databases waits for the next
//! whole moment, so no database is called from within another's call, and
//! no fault is begun meanwhile.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use turborel::db::Standby;
use turborel::error::{Code, Result, SqlError};
use turborel_replication::driver::Wait;
use turborel_replication::{Driver, Lsn, Parts};

use super::super::group::Make;
use super::world::{Started, journal, locked, watch};
use super::{Plan, Violation, World};

/// Moments past the end of the run's plan within which a sync waiting must
/// have been answered: the faults have long stopped.
const SYNC_GRACE: u64 = 10_000;

/// \[TOOL\] What makes the replicas' databases.
pub trait Host: Send {
    /// Replica `index`'s database, made or opened from what it kept, over
    /// `driver`, following: it, and through where it holds the log.
    ///
    /// # Errors
    ///
    /// Why it could not be.
    fn open(&mut self, index: usize, driver: &Arc<Driver>) -> std::result::Result<(Arc<dyn Standby>, Lsn), Violation>;
    /// Replica `index` crashed: its database is lost, but what it kept.
    fn crashed(&mut self, index: usize);
}

/// The world, each replica's driver and database while it is up, and what
/// makes the databases.
pub struct Ground {
    me: Weak<Ground>,
    world: Arc<Mutex<World>>,
    make: Make,
    drivers: Mutex<Vec<Option<Arc<Driver>>>>,
    databases: Mutex<Vec<Option<Arc<dyn Standby>>>>,
    host: Mutex<Box<dyn Host>>,
    /// Replicas crashed within a database's call, not made again until it
    /// returns.
    held: Mutex<BTreeSet<usize>>,
}

impl Ground {
    /// The world, locked.
    fn world(&self) -> MutexGuard<'_, World> {
        locked(&self.world)
    }

    /// One moment: faults begun if `top`, replicas due made again, every
    /// driver taking a turn if `top`, else its inputs alone.
    ///
    /// # Errors
    ///
    /// The first violation found.
    fn moment(&self, top: bool) -> std::result::Result<(), Violation> {
        let moved = self.world().moment(top)?;
        if top {
            self.held.lock().unwrap().clear();
        }
        for index in moved.crashed {
            self.lost(index);
        }
        for index in moved.restart {
            if !self.held.lock().unwrap().contains(&index) {
                self.start(index)?;
            }
        }
        let count = self.world().replicas();
        for index in 0..count {
            let Some(driver) = self.driver(index) else { continue };
            let database = self.databases.lock().unwrap()[index].clone();
            let turned = match (top, database) {
                (true, Some(database)) => driver.turn(&*database),
                (true, None) | (false, _) => driver.step_inputs(),
            };
            let failed = turned.and_then(|()| driver.failed().map_or(Ok(()), Err));
            if let Err(e) = failed {
                if !self.world().disk_failed(index) {
                    return Err(self.world().fail(&format!("replica {}: {e}", index + 1)));
                }
                self.crash(index);
                // Crashed within a database's call, perhaps its own, it is
                // made again only once the call has returned.
                if !top {
                    self.held.lock().unwrap().insert(index);
                }
            }
        }
        let mut world = self.world();
        world.flush_sends();
        if world.over() && world.now() > world.end() + SYNC_GRACE {
            return Err(world.fail("a sync was never answered"));
        }
        world.check()
    }

    /// Replica `index`'s driver, while it is up.
    fn driver(&self, index: usize) -> Option<Arc<Driver>> {
        self.drivers.lock().unwrap()[index].clone()
    }

    /// Replica `index` made again, from its journal: its driver, and its
    /// database opened over it.
    fn start(&self, index: usize) -> std::result::Result<(), Violation> {
        let (members, id, first) = {
            let world = self.world();
            (world.members().to_vec(), world.members()[index], world.lives(index) == 0 && world.now() == 0)
        };
        let Started { recovered, disk, link, inbox, ticks } = self.world().start(index);
        let records = recovered.records.clone();
        let consensus = (self.make)(&members, id, if first { None } else { Some(recovered) });
        let wait: Arc<dyn Wait> = Arc::new(SimWait(Weak::clone(&self.me)));
        let watch = Arc::new(watch(&self.world, id));
        let parts = Parts { consensus, journal: Box::new(journal(disk)), records, transport: link, inbox, ticker: ticks, wait, watch };
        let driver = Driver::new(parts);
        self.drivers.lock().unwrap()[index] = Some(Arc::clone(&driver));
        let opened = self.host.lock().unwrap().open(index, &driver);
        let (database, holds) = opened.map_err(|why| self.world().fail(&why))?;
        driver.join(holds).map_err(|e| self.world().fail(&format!("replica {}: {e}", index + 1)))?;
        self.databases.lock().unwrap()[index] = Some(database);
        Ok(())
    }

    /// Replica `index` lost its driver and database: it crashed.
    fn lost(&self, index: usize) {
        if let Some(driver) = self.drivers.lock().unwrap()[index].take() {
            driver.halt(SqlError::new(Code::Io, "[TOOL] the simulated replica crashed"));
        }
        self.databases.lock().unwrap()[index] = None;
        self.host.lock().unwrap().crashed(index);
    }

    /// Replica `index` crashes now.
    fn crash(&self, index: usize) {
        self.world().crash(index);
        self.lost(index);
    }
}

/// A sync's wait: a moment of the ground's, within the call.
struct SimWait(Weak<Ground>);

impl Wait for SimWait {
    fn wait(&self, _driver: &Arc<Driver>) -> Result<()> {
        let ground = self.0.upgrade().ok_or_else(|| SqlError::new(Code::Io, "[TOOL] the simulation is over"))?;
        ground.moment(false).map_err(|why| SqlError::new(Code::Io, why))
    }
}

/// \[TOOL\] A run: its ground, and the host its databases are made by.
pub struct Runner<H: Host + Clone + 'static> {
    ground: Arc<Ground>,
    /// What makes the databases: a handle on what the ground holds.
    pub host: H,
    /// The run's plan.
    pub plan: Plan,
}

impl<H: Host + Clone + 'static> Runner<H> {
    /// A run of `plan` from `seed`, each replica's consensus made by
    /// `make`, its database by `host`, every replica started.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn new(seed: u64, plan: Plan, make: &Make, host: H) -> std::result::Result<Runner<H>, Violation> {
        let world = World::new(seed, plan);
        let count = world.replicas();
        let ground = Arc::new_cyclic(|me| Ground {
            me: Weak::clone(me),
            world: Arc::new(Mutex::new(world)),
            make: Arc::clone(make),
            drivers: Mutex::new(vec![None; count]),
            databases: Mutex::new(vec![None; count]),
            host: Mutex::new(Box::new(host.clone())),
            held: Mutex::new(BTreeSet::new()),
        });
        for index in 0..count {
            ground.start(index)?;
        }
        Ok(Runner { ground, host, plan })
    }

    /// The world, locked.
    pub fn world(&self) -> MutexGuard<'_, World> {
        self.ground.world()
    }

    /// Replica `index`'s driver, while it is up.
    pub fn driver(&self, index: usize) -> Option<Arc<Driver>> {
        self.ground.driver(index)
    }

    /// The replica whose database serves as primary, if one does.
    pub fn primary(&self) -> Option<usize> {
        let count = self.world().replicas();
        (0..count).find(|&i| self.driver(i).is_some_and(|d| d.serving().is_some()))
    }

    /// Replica `index` crashes now.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn crash(&mut self, index: usize) -> std::result::Result<(), Violation> {
        self.ground.crash(index);
        self.world().check()
    }

    /// One whole moment: the world moved on, replicas crashed and made
    /// again, every driver taking a turn.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn moment(&mut self) -> std::result::Result<(), Violation> {
        self.ground.moment(true)
    }
}
