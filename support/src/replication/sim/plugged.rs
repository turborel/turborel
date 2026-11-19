//! Replicas whose transport, journal, state transfer and consensus are
//! served by plug-ins (RULES.md D270 to D274), as the simulator drives
//! them: each part the crate's real bridge (`turborel_replication::plugin`)
//! over a stand-in program, a pipe answering each call out of the
//! simulator's own part (`plugged/serve.rs`), its payloads written and
//! read as protocol 1 carries them. A stand-in dies at moments the run's
//! seed chooses, before or after carrying out the call it was asked, as a
//! process killed before or after it acted but before it answered; a
//! consensus's dies more often while it changes views. The bridge then
//! fails stop, the driver halts, and the replica crashes and is made
//! again, its parts new programs, the checker watching that no replica
//! diverges or acknowledges what its journal did not make durable.

mod serve;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use turborel::disk::memory::MemoryFileSystem;
use turborel::error::Result;
use turborel::plugin::errors::ended;
use turborel_replication::driver::Inbox;
use turborel_replication::plugin::{Pipe, PluginConsensus, PluginJournal, PluginTransfer, PluginTransport};
use turborel_replication::{Consensus, Journal, Member, Recovered, ReplicaId, StateTransfer, Transport};

use super::super::group::Make;
use super::Rng;
use serve::{ConsensusServe, JournalServe, Serve, TransferServe, TransportServe};

/// A part of a replica a plug-in may serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// Its transport.
    Transport,
    /// Its journal.
    Journal,
    /// Its state transfer.
    Transfer,
    /// Its consensus.
    Consensus,
}

/// Every part.
pub const ALL_PARTS: [Part; 4] = [Part::Transport, Part::Journal, Part::Transfer, Part::Consensus];

/// Which parts plug-ins serve, and how often their programs die.
#[derive(Debug, Clone, Copy)]
pub struct Plugging {
    /// Per mille of calls on which a stand-in dies.
    pub kill: u32,
    /// Per mille of a consensus's steps, while it changes views, on which
    /// its stand-in dies.
    pub view_change_kill: u32,
    /// The parts plug-ins serve.
    pub parts: &'static [Part],
}

impl Plugging {
    /// Every part plugged, a stand-in dying on `kill` per mille of calls
    /// and `view_change_kill` of a consensus's steps in a view change.
    pub fn all(kill: u32, view_change_kill: u32) -> Plugging {
        Plugging { kill, view_change_kill, parts: &ALL_PARTS }
    }

    /// Whether a plug-in serves `part`.
    pub fn serves(&self, part: Part) -> bool {
        self.parts.contains(&part)
    }
}

/// The plug-ins of a run: what they serve, the chances of their deaths,
/// and the replicas whose stand-in died since they last started.
pub struct Plug {
    plugging: Plugging,
    rng: Mutex<Rng>,
    died: Mutex<BTreeSet<usize>>,
    kills: Mutex<u64>,
    /// Whether the faults have stopped: no stand-in dies then, so the
    /// group can converge.
    calm: AtomicBool,
}

impl Plug {
    /// The plug-ins of `plugging`, their deaths chosen from `seed`.
    pub fn new(plugging: Plugging, seed: u64) -> Arc<Plug> {
        let rng = Mutex::new(Rng::new(seed ^ 0x9e37_79b9_7f4a_7c15));
        Arc::new(Plug { plugging, rng, died: Mutex::new(BTreeSet::new()), kills: Mutex::new(0), calm: AtomicBool::new(false) })
    }

    /// Whether replica `index`'s stand-in died since it last started.
    pub fn died(&self, index: usize) -> bool {
        self.died.lock().unwrap().contains(&index)
    }

    /// The stand-ins killed in the run.
    pub fn kills(&self) -> u64 {
        *self.kills.lock().unwrap()
    }

    /// The faults have stopped, or not.
    pub fn set_calm(&self, calm: bool) {
        self.calm.store(calm, Ordering::Relaxed);
    }

    /// Whether a call dies, and if so whether after it was carried out.
    fn decide(&self, hot: bool) -> Option<bool> {
        if self.calm.load(Ordering::Relaxed) {
            return None;
        }
        let mut rng = self.rng.lock().unwrap();
        let chance = if hot { self.plugging.view_change_kill } else { self.plugging.kill };
        rng.chance(chance).then(|| rng.chance(500))
    }
}

/// A stand-in program of replica `index`: a pipe answering each call out
/// of `serve`, dying as the plug's chances say.
struct StandIn {
    plug: Arc<Plug>,
    index: usize,
    serve: Box<dyn Serve>,
    /// The first call, OPEN or BEGIN, never dies: a replica that cannot
    /// start is no part of what the run checks.
    first: bool,
    dead: bool,
}

impl Pipe for StandIn {
    fn call(&mut self, op: u8, payload: Vec<u8>) -> Result<Vec<u8>> {
        if self.dead {
            return Err(ended("[TOOL] the stand-in plug-in is dead"));
        }
        let first = std::mem::replace(&mut self.first, false);
        let decided = if first { None } else { self.plug.decide(self.serve.hot()) };
        let Some(after) = decided else { return self.serve.answer(op, &payload) };
        self.dead = true;
        self.plug.died.lock().unwrap().insert(self.index);
        *self.plug.kills.lock().unwrap() += 1;
        if after {
            // Killed after it acted, before its answer was written.
            let _acted = self.serve.answer(op, &payload);
        }
        Err(ended("[TOOL] the stand-in plug-in was killed"))
    }
}

/// The parts of a replica made again, plugged where the plug says.
pub struct Parts {
    /// Its journal.
    pub journal: Box<dyn Journal>,
    /// What its journal holds, as the journal's OPEN gave it.
    pub recovered: Recovered,
    /// Its transport.
    pub transport: Arc<dyn Transport>,
    /// Where its driver takes frames from.
    pub inbox: Arc<Inbox>,
    /// What brings frames in, of a plugged transport.
    pub pump: Option<Arc<PluginTransport>>,
    /// Its state transfer.
    pub transfer: Arc<dyn StateTransfer>,
}

/// What a replica is made of before it is plugged: its index and number,
/// its journal and what it holds, its link and inbox, its transfer.
pub struct Unplugged {
    /// Its index.
    pub index: usize,
    /// Its number.
    pub id: ReplicaId,
    /// Its configuration as its node sets it.
    pub members: Vec<Member>,
    /// Its journal on the world's disk.
    pub journal: Box<dyn Journal>,
    /// What that holds.
    pub recovered: Recovered,
    /// Its end of the network.
    pub link: Arc<dyn Transport>,
    /// Where the network puts its frames.
    pub inbox: Arc<Inbox>,
    /// Its transfer.
    pub transfer: Arc<dyn StateTransfer>,
}

impl Plug {
    /// A stand-in of replica `index` over `serve`.
    fn stand_in(self: &Arc<Self>, index: usize, serve: Box<dyn Serve>) -> Box<dyn Pipe> {
        Box::new(StandIn { plug: Arc::clone(self), index, serve, first: true, dead: false })
    }

    /// The parts of `u`, each the plug serves through a stand-in.
    ///
    /// # Errors
    ///
    /// A bridge's, opening: none dies then.
    pub fn parts(self: &Arc<Self>, u: Unplugged) -> Result<Parts> {
        self.died.lock().unwrap().remove(&u.index);
        let p = self.plugging;
        let (journal, recovered): (Box<dyn Journal>, Recovered) = if p.serves(Part::Journal) {
            let (j, r) = PluginJournal::open(self.stand_in(u.index, Box::new(JournalServe::new(u.journal, u.recovered))))?;
            (Box::new(j), r)
        } else {
            (u.journal, u.recovered)
        };
        let (transport, inbox, pump): (Arc<dyn Transport>, Arc<Inbox>, Option<Arc<PluginTransport>>) = if p.serves(Part::Transport) {
            let serve = Box::new(TransportServe::new(Arc::clone(&u.link), u.inbox));
            let t = Arc::new(PluginTransport::open(self.stand_in(u.index, serve), u.id, &u.members)?);
            (Arc::<PluginTransport>::clone(&t), Arc::new(Inbox::default()), Some(t))
        } else {
            (u.link, u.inbox, None)
        };
        let transfer: Arc<dyn StateTransfer> = if p.serves(Part::Transfer) {
            let pipe = self.stand_in(u.index, Box::new(TransferServe::new(u.transfer)));
            Arc::new(PluginTransfer::new(pipe, Arc::new(MemoryFileSystem::new()), PathBuf::from("/plugged/transfer")))
        } else {
            u.transfer
        };
        Ok(Parts { journal, recovered, transport, inbox, pump, transfer })
    }

    /// The consensus of replica `index`, `me` of `members`, made by `make`
    /// in a stand-in if the plug serves it.
    ///
    /// # Errors
    ///
    /// The bridge's, opening.
    pub fn consensus(
        self: &Arc<Self>,
        index: usize,
        (members, me): (&[Member], ReplicaId),
        recovered: Option<Recovered>,
        make: &Make,
    ) -> Result<Box<dyn Consensus>> {
        if !self.plugging.serves(Part::Consensus) {
            return Ok(make(members, me, recovered));
        }
        let pipe = self.stand_in(index, Box::new(ConsensusServe::new(Arc::clone(make))));
        Ok(Box::new(PluginConsensus::open(pipe, (members, me), recovered.as_ref(), 0)?))
    }
}
