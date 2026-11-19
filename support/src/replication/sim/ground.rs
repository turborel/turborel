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
use turborel_replication::{Driver, Lsn, Parts, StateTransfer};

use super::super::group::{Make, configuration};
use super::parts::{journal, watch};
use super::plugged::{Plug, Unplugged};
use super::world::{Started, locked};
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
    /// How a state transfer crosses to replica `index`, staged where its
    /// database is kept.
    fn transfer(&mut self, index: usize) -> Arc<dyn StateTransfer>;
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
    /// Replicas crashed within their own database's call, not made again
    /// until it returns.
    held: Mutex<BTreeSet<usize>>,
    /// Replicas within a sync's wait: their drivers take no turn until it
    /// returns, as a turn would call their databases from within their
    /// own call, but answer state transfers (`Driver::serve_transfers`);
    /// the others' do, as on a node's other threads, so a replica waiting
    /// on the group is served by it meanwhile.
    waiting: Mutex<BTreeSet<usize>>,
    /// The plug-ins serving the replicas' parts, if any do
    /// (`sim/plugged.rs`).
    plug: Option<Arc<Plug>>,
}

impl Ground {
    /// The world, locked.
    fn world(&self) -> MutexGuard<'_, World> {
        locked(&self.world)
    }

    /// One moment: faults begun if `top`, replicas due made again, every
    /// driver taking a turn but those within a wait, which take their
    /// inputs alone.
    ///
    /// # Errors
    ///
    /// The first violation found.
    fn moment(&self, top: bool) -> std::result::Result<(), Violation> {
        let moved = self.world().moment(top)?;
        if let Some(plug) = &self.plug {
            plug.set_calm(self.world().calm());
        }
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
        let waiting = self.waiting.lock().unwrap().clone();
        for index in 0..count {
            let Some(driver) = self.driver(index) else { continue };
            let database = self.databases.lock().unwrap()[index].clone();
            let turned = match (waiting.contains(&index), database) {
                (false, Some(database)) => driver.turn(&*database),
                (true, Some(database)) => driver.step_inputs().and_then(|()| driver.serve_transfers(&*database)),
                (false | true, None) => driver.step_inputs(),
            };
            let failed = turned.and_then(|()| driver.failed().map_or(Ok(()), Err));
            if let Err(e) = failed {
                let plugin_died = self.plug.as_ref().is_some_and(|p| p.died(index));
                if !self.world().disk_failed(index) && !plugin_died {
                    return Err(self.world().fail(&format!("replica {}: {e}", index + 1)));
                }
                self.crash(index);
                // Crashed within its own database's call, it is made again
                // only once the call has returned.
                if !top && self.waiting.lock().unwrap().contains(&index) {
                    self.held.lock().unwrap().insert(index);
                }
            }
        }
        let standings = self.standings();
        let mut world = self.world();
        world.flush_sends();
        if world.over() && world.now() > world.end() + SYNC_GRACE {
            return Err(world.fail(&format!("a sync was never answered; the replicas: {standings}")));
        }
        world.check()
    }

    /// Replica `index`'s driver, while it is up.
    fn driver(&self, index: usize) -> Option<Arc<Driver>> {
        self.drivers.lock().unwrap()[index].clone()
    }

    /// Every replica's standing, for a violation: what its driver serves,
    /// holds and committed, and its consensus's own account.
    fn standings(&self) -> String {
        let count = self.drivers.lock().unwrap().len();
        let each: Vec<String> = (0..count)
            .map(|i| match self.driver(i) {
                Some(d) => {
                    format!("{}: serving {:?} holds {} committed {}: {}", i + 1, d.serving(), d.holds().0, d.committed().0, d.describe())
                }
                None => format!("{}: down", i + 1),
            })
            .collect();
        each.join("; ")
    }

    /// Replica `index` made again, from its journal, its node's setting of
    /// the members its starting point: its driver, and its database
    /// opened over it.
    fn start(&self, index: usize) -> std::result::Result<(), Violation> {
        let (members, id) = {
            let world = self.world();
            (world.setting(index), world.id(index))
        };
        let Started { new_group, recovered, disk, link, inbox, ticks } = self.world().start(index);
        let (retain_bytes, transfer) = (self.world().plan().retain_bytes, self.host.lock().unwrap().transfer(index));
        let unplugged = Unplugged {
            index,
            id,
            members: configuration(&members),
            journal: Box::new(journal(disk, &self.world, id)),
            recovered,
            link,
            inbox,
            transfer,
        };
        let replica = self.parts(unplugged, new_group).map_err(|e| self.world().fail(&format!("replica {}: {e}", index + 1)))?;
        let wait: Arc<dyn Wait> = Arc::new(SimWait(Weak::clone(&self.me)));
        let watch = Arc::new(watch(&self.world, id));
        let parts = Parts {
            consensus: replica.consensus,
            journal: replica.parts.journal,
            first: replica.first,
            records: replica.records,
            retain_bytes,
            transport: replica.parts.transport,
            inbox: Arc::clone(&replica.parts.inbox),
            ticker: ticks,
            wait,
            watch,
            transfer: replica.parts.transfer,
        };
        let driver = Driver::new(parts);
        if let Some(t) = &replica.parts.pump {
            driver.pump_with(t.pump(replica.parts.inbox));
        }
        self.drivers.lock().unwrap()[index] = Some(Arc::clone(&driver));
        let opened = self.host.lock().unwrap().open(index, &driver);
        let (database, holds) = opened.map_err(|why| self.world().fail(&why))?;
        self.world().note(id, &format!("its database opened, holding the log through {}", holds.0));
        self.databases.lock().unwrap()[index] = Some(database);
        if let Err(e) = driver.join(holds) {
            // A plug-in that died as the replica joined fails it stop.
            if !self.plug.as_ref().is_some_and(|p| p.died(index)) {
                return Err(self.world().fail(&format!("replica {}: {e}", index + 1)));
            }
            self.crash(index);
        }
        Ok(())
    }

    /// The parts of `u`, plugged as the run's plug says; its consensus
    /// made as a node's is: a new group's replica as the run begins, else
    /// from its journal, recovering if it holds nothing.
    fn parts(&self, u: Unplugged, new_group: bool) -> Result<Replica> {
        let (index, id, members) = (u.index, u.id, u.members.clone());
        let parts = match &self.plug {
            Some(plug) => plug.parts(u)?,
            None => super::plugged::Parts {
                journal: u.journal,
                recovered: u.recovered,
                transport: u.link,
                inbox: u.inbox,
                pump: None,
                transfer: u.transfer,
            },
        };
        let (first, records) = (parts.recovered.first, parts.recovered.records.clone());
        let held = (!new_group).then(|| parts.recovered.clone());
        let consensus = match &self.plug {
            Some(plug) => plug.consensus(index, (&members, id), held, &self.make)?,
            None => (self.make)(&members, id, held),
        };
        Ok(Replica { consensus, first, records, parts })
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

/// A replica's parts, made.
struct Replica {
    consensus: Box<dyn turborel_replication::Consensus>,
    first: Lsn,
    records: Vec<Vec<u8>>,
    parts: super::plugged::Parts,
}

/// A sync's wait: a moment of the ground's, within the call, the waiting
/// replica taking no turn in it.
struct SimWait(Weak<Ground>);

impl Wait for SimWait {
    fn wait(&self, driver: &Arc<Driver>) -> Result<()> {
        let ground = self.0.upgrade().ok_or_else(|| SqlError::new(Code::Io, "[TOOL] the simulation is over"))?;
        let index = ground.drivers.lock().unwrap().iter().position(|d| d.as_ref().is_some_and(|d| Arc::ptr_eq(d, driver)));
        let entered = index.filter(|i| ground.waiting.lock().unwrap().insert(*i));
        let moved = ground.moment(false);
        if let Some(left) = entered {
            ground.waiting.lock().unwrap().remove(&left);
        }
        moved.map_err(|why| SqlError::new(Code::Io, why))
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
    /// `make`, its database by `host`, every member started, the spares
    /// waiting.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn new(seed: u64, plan: Plan, make: &Make, host: H) -> std::result::Result<Runner<H>, Violation> {
        Runner::plugged(seed, plan, (make, host), None)
    }

    /// A run as `new` makes one, its replicas' parts served by plug-ins
    /// as `plug` says, if given.
    ///
    /// # Errors
    ///
    /// The first violation found.
    pub fn plugged(seed: u64, plan: Plan, (make, host): (&Make, H), plug: Option<Arc<Plug>>) -> std::result::Result<Runner<H>, Violation> {
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
            waiting: Mutex::new(BTreeSet::new()),
            plug,
        });
        for index in (0..count).filter(|&i| ground.world().joined(i)) {
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

    /// The plug-ins' stand-ins killed so far, if plug-ins serve.
    pub fn plugin_kills(&self) -> u64 {
        self.ground.plug.as_ref().map_or(0, |p| p.kills())
    }

    /// Every replica's standing, for a violation.
    pub fn standings(&self) -> String {
        self.ground.standings()
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
