//! \[EXTENSION\] The host kinds replication and the archive are served by
//! (RULES.md D270 to D274), one trait each: a replica transport, a
//! journal, a state transfer (`replication/transfer.rs`), a consensus,
//! and an archive (a backup target's `Target`, its calls a kind 8
//! target's operations, its stores its own).
//! Each is registered by a method of `Plugin` naming it, served as a host
//! object of its kind, its payloads read and written as
//! `turborel::plugin::payload::replication` gives them.
//!
//! A host plug-in is fail-stop: any ERROR one of these answers stops the
//! node, save the archive's, whose ERROR fails only the call.

mod transfer;

use turborel::bytes::Put;
use turborel::plugin::payload::replication::consensus::{self as c, Input, Output};
use turborel::plugin::payload::replication::{self as p, Held, Member};

pub use self::transfer::StateTransfer;

use super::super::dispatch::{read, target, u64_of};
use super::super::objects::Held as Numbered;
use super::super::registry::Plugin;
use super::super::serve::Cx;
use super::super::{Answer, PluginError, Target};
use super::HostObject;

/// \[EXTENSION\] A replica transport (kind 32, RULES.md D271): the frames
/// of a replica's group carried to and from its members.
pub trait ReplicaTransport: Send {
    /// Links replica `me` to the members of `configuration`, answering
    /// those it links.
    ///
    /// # Errors
    ///
    /// The transport's: the node stops.
    fn open(&mut self, me: u32, configuration: &[Member]) -> Answer<Vec<u32>>;
    /// Sends `frame` to replica `to` whole, or not at all: false if `to`
    /// is no member it links. A frame lost on the way is as the network
    /// loses one.
    ///
    /// # Errors
    ///
    /// The transport's: the node stops.
    fn send(&mut self, to: u32, frame: &[u8]) -> Answer<bool>;
    /// The frames arrived since last asked, each whole, from its replica.
    ///
    /// # Errors
    ///
    /// The transport's: the node stops.
    fn receive(&mut self) -> Answer<Vec<(u32, Vec<u8>)>>;
    /// Links the members of `configuration`, answering those it links.
    ///
    /// # Errors
    ///
    /// The transport's: the node stops.
    fn reconfigure(&mut self, configuration: &[Member]) -> Answer<Vec<u32>>;
}

/// \[EXTENSION\] A journal (kind 33, RULES.md D272): a replica's records,
/// marks and protocol state, kept durable.
pub trait Journal: Send {
    /// What it holds, as it opens.
    ///
    /// # Errors
    ///
    /// The journal's: the node stops.
    fn open(&mut self) -> Answer<Held>;
    /// Writes `record` at `at`, durable once `sync` or `flush` names it.
    ///
    /// # Errors
    ///
    /// The journal's: the node stops.
    fn persist(&mut self, at: u64, record: &[u8]) -> Answer<()>;
    /// Removes the records after `after`.
    ///
    /// # Errors
    ///
    /// The journal's: the node stops.
    fn truncate(&mut self, after: u64) -> Answer<()>;
    /// Lets the records through `through` go.
    ///
    /// # Errors
    ///
    /// The journal's: the node stops.
    fn retain(&mut self, through: u64) -> Answer<()>;
    /// Marks the record at `at` a reconfiguration to `epoch` of `members`.
    ///
    /// # Errors
    ///
    /// The journal's: the node stops.
    fn mark(&mut self, at: u64, epoch: u64, members: &[Member]) -> Answer<()>;
    /// Writes the protocol's state, durable with every write before it
    /// once this answers.
    ///
    /// # Errors
    ///
    /// The journal's: the node stops.
    fn persist_state(&mut self, state: &[u8]) -> Answer<()>;
    /// The positions of the records made durable since `sync` or `flush`
    /// last answered, each once, in the order persisted, removed since or
    /// not.
    ///
    /// # Errors
    ///
    /// The journal's: the node stops.
    fn sync(&mut self) -> Answer<Vec<u64>>;
    /// As `sync`, every write made durable first.
    ///
    /// # Errors
    ///
    /// The journal's: the node stops.
    fn flush(&mut self) -> Answer<Vec<u64>>;
}

/// \[EXTENSION\] A consensus (kind 35, RULES.md D273): the replication
/// protocol of a replica, a machine of inputs and outputs that must be
/// deterministic, made again from its journal after any failure.
pub trait Consensus: Send {
    /// Made for replica `me` of `configuration`, from what its journal
    /// held if it is made again, with its recovery's `nonce`.
    ///
    /// # Errors
    ///
    /// The consensus's: the node stops.
    fn open(&mut self, me: u32, configuration: &[Member], recovered: Option<Held>, nonce: u64) -> Answer<()>;
    /// The outputs of `input`, every one, in order.
    ///
    /// # Errors
    ///
    /// The consensus's: the node stops.
    fn step(&mut self, input: Input) -> Answer<Vec<Output>>;
    /// Its standing, for the operator.
    fn describe(&mut self) -> String;
}

impl Plugin {
    /// It, serving `transport` as the replica transport `name`.
    #[must_use]
    pub fn replica_transport(self, name: &str, transport: impl ReplicaTransport + 'static) -> Plugin {
        self.host(p::REPLICA_TRANSPORT, name, Transporting(transport))
    }

    /// It, serving `journal` as the journal `name`.
    #[must_use]
    pub fn journal(self, name: &str, journal: impl Journal + 'static) -> Plugin {
        self.host(p::JOURNAL, name, Journaling(journal))
    }

    /// It, serving `consensus` as the consensus `name`.
    #[must_use]
    pub fn consensus(self, name: &str, consensus: impl Consensus + 'static) -> Plugin {
        self.host(p::CONSENSUS, name, Consenting(consensus))
    }

    /// It, serving `transfer` as the state transfer `name`.
    #[must_use]
    pub fn state_transfer(self, name: &str, transfer: impl StateTransfer + 'static) -> Plugin {
        self.host(p::STATE_TRANSFER, name, transfer::Transferring(transfer))
    }

    /// It, serving `target` as the archive of the scheme `name`.
    #[must_use]
    pub fn archive(self, name: &str, target: impl Target + 'static) -> Plugin {
        self.host(p::ARCHIVE, name, Archived { scheme: Box::new(target), held: Numbered::default() })
    }
}

/// An archive as a host object: a backup target's calls (kind 8's
/// operations), answered by its own scheme with stores and uploads of its
/// own, apart from any kind 8 target of its name.
struct Archived {
    /// The scheme.
    scheme: Box<dyn Target>,
    /// Its stores and uploads, by number.
    held: Numbered,
}

impl HostObject for Archived {
    fn answer(&mut self, _cx: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        target(self.scheme.as_mut(), &mut self.held, op, &mut turborel::plugin::payload::reader(payload))
    }
}

/// A transport as a host object.
struct Transporting<T>(T);

impl<T: ReplicaTransport> HostObject for Transporting<T> {
    fn answer(&mut self, _cx: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        let r = &mut turborel::plugin::payload::reader(payload);
        let mut out = Vec::new();
        match op {
            p::TRANSPORT_OPEN => {
                let (me, configuration) = read(r, |r| Ok((r.u32()?, p::configuration(r)?)))?;
                p::put_members(&mut out, &self.0.open(me, &configuration)?);
            }
            p::TRANSPORT_SEND => {
                let (to, frame) = read(r, |r| Ok((r.u32()?, r.bytes()?.to_vec())))?;
                out.put_u8(u8::from(self.0.send(to, &frame)?));
            }
            p::TRANSPORT_RECEIVE => p::put_arrived(&mut out, &self.0.receive()?),
            p::TRANSPORT_RECONFIGURE => p::put_members(&mut out, &self.0.reconfigure(&read(r, p::configuration)?)?),
            _ => return Err(unknown("replica transport", op)),
        }
        Ok(out)
    }
}

/// A journal as a host object.
struct Journaling<J>(J);

impl<J: Journal> HostObject for Journaling<J> {
    fn answer(&mut self, _cx: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        let r = &mut turborel::plugin::payload::reader(payload);
        let j = &mut self.0;
        match op {
            p::JOURNAL_OPEN => {
                let mut out = Vec::new();
                p::put_held(&mut out, &j.open()?);
                return Ok(out);
            }
            p::JOURNAL_PERSIST => {
                let (at, record) = read(r, |r| Ok((r.u64()?, r.bytes()?.to_vec())))?;
                j.persist(at, &record)?;
            }
            p::JOURNAL_TRUNCATE => j.truncate(read(r, u64_of)?)?,
            p::JOURNAL_RETAIN => j.retain(read(r, u64_of)?)?,
            p::JOURNAL_MARK => {
                let (at, (epoch, members)) = read(r, |r| Ok((r.u64()?, p::mark(r)?)))?;
                j.mark(at, epoch, &members)?;
            }
            p::JOURNAL_STATE => j.persist_state(&read(r, |r| Ok(r.bytes()?.to_vec()))?)?,
            p::JOURNAL_SYNC => return Ok(positions(&j.sync()?)),
            p::JOURNAL_FLUSH => return Ok(positions(&j.flush()?)),
            _ => return Err(unknown("journal", op)),
        }
        Ok(Vec::new())
    }
}

/// `named`, as SYNC and FLUSH answer them.
fn positions(named: &[u64]) -> Vec<u8> {
    let mut out = Vec::new();
    p::put_positions(&mut out, named);
    out
}

/// A consensus as a host object.
struct Consenting<C>(C);

impl<C: Consensus> HostObject for Consenting<C> {
    fn answer(&mut self, _cx: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        let r = &mut turborel::plugin::payload::reader(payload);
        let mut out = Vec::new();
        match op {
            c::CONSENSUS_OPEN => {
                let (me, configuration, recovered, nonce) = read(r, c::open)?;
                self.0.open(me, &configuration, recovered, nonce)?;
            }
            c::CONSENSUS_STEP => c::put_outputs(&mut out, &self.0.step(read(r, c::input)?)?),
            c::CONSENSUS_DESCRIBE => out.put_str(&self.0.describe()),
            _ => return Err(unknown("consensus", op)),
        }
        Ok(out)
    }
}

/// An operation `op` no object of `kind` has.
fn unknown(kind: &str, op: u8) -> PluginError {
    PluginError::external(&format!("a {kind} has no operation {op}"))
}

#[cfg(test)]
mod tests;
