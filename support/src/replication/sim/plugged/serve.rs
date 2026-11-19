//! The stand-in programs' answers: each call of a host kind of
//! replication read as protocol 1 carries it, carried out by the
//! simulator's own part, and answered as a program would answer it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use turborel::bytes::{Put, Reader};
use turborel::db::{ExportedFile, StateSource};
use turborel::error::{Code, Result, SqlError};
use turborel::files::FileOwner;
use turborel::plugin::payload::reader;
use turborel::plugin::payload::replication::consensus::{CONSENSUS_DESCRIBE, CONSENSUS_OPEN, CONSENSUS_STEP, input, open, put_outputs};
use turborel::plugin::payload::replication::{
    JOURNAL_FLUSH, JOURNAL_MARK, JOURNAL_OPEN, JOURNAL_PERSIST, JOURNAL_RETAIN, JOURNAL_STATE, JOURNAL_SYNC, JOURNAL_TRUNCATE, Manifest,
    Served, TRANSFER_BEGIN, TRANSFER_CHUNK, TRANSFER_EXPORT, TRANSFER_FETCHED, TRANSFER_SERVE, TRANSFER_TAKE, TRANSPORT_OPEN,
    TRANSPORT_RECEIVE, TRANSPORT_RECONFIGURE, TRANSPORT_SEND, chunk_digest, configuration, given, manifest, mark, put_arrived, put_held,
    put_manifest, put_members, put_optional, put_positions, put_served,
};
use turborel_replication::driver::Inbox;
use turborel_replication::plugin::convert::{configuration_from, input_from, output_to, recovered_from, recovered_to};
use turborel_replication::{Consensus, Fetching, Journal, Lsn, Mark, Recovered, ReplicaId, StateTransfer, Transport};

use super::super::super::group::Make;

/// A stand-in program's part: each call answered.
pub trait Serve: Send {
    /// The RESULT of operation `op` of `payload`.
    fn answer(&mut self, op: u8, payload: &[u8]) -> Result<Vec<u8>>;
    /// Whether it is at a moment its program dies more often at.
    fn hot(&self) -> bool {
        false
    }
}

/// The refusal of an operation a kind has not.
fn no_op(op: u8) -> SqlError {
    SqlError::new(Code::Api, format!("[TOOL] the stand-in has no operation {op}"))
}

/// A journal's program, over the simulator's journal.
pub struct JournalServe {
    journal: Box<dyn Journal>,
    recovered: Recovered,
}

impl JournalServe {
    /// Over `journal`, which holds `recovered`.
    pub fn new(journal: Box<dyn Journal>, recovered: Recovered) -> JournalServe {
        JournalServe { journal, recovered }
    }
}

impl Serve for JournalServe {
    fn answer(&mut self, op: u8, payload: &[u8]) -> Result<Vec<u8>> {
        let mut r = reader(payload);
        let mut out = Vec::new();
        match op {
            JOURNAL_OPEN => put_held(&mut out, &recovered_to(&self.recovered, self.journal.durable())),
            JOURNAL_PERSIST => self.journal.persist(Lsn(r.u64()?), r.bytes()?)?,
            JOURNAL_TRUNCATE => self.journal.truncate(Lsn(r.u64()?))?,
            JOURNAL_RETAIN => self.journal.retain(Lsn(r.u64()?))?,
            JOURNAL_MARK => {
                let at = Lsn(r.u64()?);
                let (epoch, members) = mark(&mut r)?;
                self.journal.mark(at, &Mark { epoch, configuration: configuration_from(members) })?;
            }
            JOURNAL_STATE => self.journal.persist_state(r.bytes()?)?,
            JOURNAL_SYNC => put_positions(&mut out, &self.journal.sync()?.into_iter().map(|l| l.0).collect::<Vec<_>>()),
            JOURNAL_FLUSH => put_positions(&mut out, &self.journal.flush()?.into_iter().map(|l| l.0).collect::<Vec<_>>()),
            other => return Err(no_op(other)),
        }
        Ok(out)
    }
}

/// A transport's program, over the simulator's link and the inbox the
/// network puts frames in.
pub struct TransportServe {
    link: Arc<dyn Transport>,
    inbox: Arc<Inbox>,
}

impl TransportServe {
    /// Over `link`, its frames arriving in `inbox`.
    pub fn new(link: Arc<dyn Transport>, inbox: Arc<Inbox>) -> TransportServe {
        TransportServe { link, inbox }
    }

    /// The members linked, by number.
    fn members(&self) -> Vec<u32> {
        self.link.members().into_iter().map(|m| m.0).collect()
    }
}

impl Serve for TransportServe {
    fn answer(&mut self, op: u8, payload: &[u8]) -> Result<Vec<u8>> {
        let mut r = reader(payload);
        let mut out = Vec::new();
        match op {
            TRANSPORT_OPEN => {
                let _me = r.u32()?;
                self.link.reconfigure(&configuration_from(configuration(&mut r)?))?;
                put_members(&mut out, &self.members());
            }
            TRANSPORT_SEND => {
                let (to, frame) = (ReplicaId(r.u32()?), r.bytes()?);
                let sent = match self.link.send(to, frame) {
                    Ok(()) => 1,
                    Err(e) if e.sqlcode() == -910_i32 => 0,
                    Err(e) => return Err(e),
                };
                out.put_u8(sent);
            }
            TRANSPORT_RECEIVE => {
                let arrived: Vec<(u32, Vec<u8>)> = self.inbox.take().into_iter().map(|(from, f)| (from.0, f)).collect();
                put_arrived(&mut out, &arrived);
            }
            TRANSPORT_RECONFIGURE => {
                self.link.reconfigure(&configuration_from(configuration(&mut r)?))?;
                put_members(&mut out, &self.members());
            }
            other => return Err(no_op(other)),
        }
        Ok(out)
    }
}

/// A consensus's program, over the consensus `make` makes.
pub struct ConsensusServe {
    make: Make,
    consensus: Option<Box<dyn Consensus>>,
}

impl ConsensusServe {
    /// Of what `make` makes.
    pub fn new(make: Make) -> ConsensusServe {
        ConsensusServe { make, consensus: None }
    }
}

impl Serve for ConsensusServe {
    fn answer(&mut self, op: u8, payload: &[u8]) -> Result<Vec<u8>> {
        let mut r = reader(payload);
        let mut out = Vec::new();
        match (op, self.consensus.as_mut()) {
            (CONSENSUS_OPEN, _) => {
                let (me, members, held, _nonce) = open(&mut r)?;
                let made = (self.make)(&configuration_from(members), ReplicaId(me), held.map(recovered_from));
                out.put_str(made.name());
                self.consensus = Some(made);
            }
            (CONSENSUS_STEP, Some(c)) => {
                let mut outputs = Vec::new();
                c.step(input_from(input(&mut r)?), &mut outputs);
                put_outputs(&mut out, &outputs.into_iter().map(output_to).collect::<Vec<_>>());
            }
            (CONSENSUS_DESCRIBE, Some(c)) => out.put_str(&c.describe()),
            (other, _) => return Err(no_op(other)),
        }
        Ok(out)
    }

    /// While it changes views.
    fn hot(&self) -> bool {
        self.consensus.as_ref().is_some_and(|c| c.describe().contains("ViewChange"))
    }
}

/// A state transfer's program, over the simulator's transfer: the
/// fetches it began, the exports fetched, and the exports it serves, each
/// read through the pieces the host gives.
pub struct TransferServe {
    transfer: Arc<dyn StateTransfer>,
    fetches: BTreeMap<u64, Box<dyn Fetching>>,
    fetched: BTreeMap<u64, Box<dyn StateSource>>,
    exports: BTreeMap<u64, Given>,
    next: u64,
}

impl TransferServe {
    /// Over `transfer`.
    pub fn new(transfer: Arc<dyn StateTransfer>) -> TransferServe {
        TransferServe { transfer, fetches: BTreeMap::new(), fetched: BTreeMap::new(), exports: BTreeMap::new(), next: 0 }
    }
}

impl Serve for TransferServe {
    fn answer(&mut self, op: u8, payload: &[u8]) -> Result<Vec<u8>> {
        let mut r = reader(payload);
        let mut out = Vec::new();
        match op {
            TRANSFER_BEGIN => {
                let mut fetching = self.transfer.begin(ReplicaId(r.u32()?))?;
                self.next += 1;
                out.put_u64(self.next);
                put_optional(&mut out, fetching.request().as_deref());
                self.fetches.insert(self.next, fetching);
            }
            TRANSFER_TAKE => {
                let (fetch, frame) = (r.u64()?, r.bytes()?);
                let f = self.fetches.get_mut(&fetch).ok_or_else(|| no_op(op))?;
                let done = f.take(frame)?;
                out.put_u8(u8::from(done));
                put_optional(&mut out, if done { None } else { f.request() }.as_deref());
            }
            TRANSFER_FETCHED => fetched(self, &mut r, &mut out)?,
            TRANSFER_CHUNK => {
                let (fetch, file, at, max) = (r.u64()?, r.u32()?, r.u64()?, r.u32()?);
                let source = self.fetched.get_mut(&fetch).ok_or_else(|| no_op(op))?;
                let mut chunk = vec![0; usize::try_from(max).unwrap()];
                let read = source.read(usize::try_from(file).unwrap(), at, &mut chunk)?;
                chunk.truncate(read);
                out.put_bytes(&chunk);
                out.put_u64(chunk_digest(&chunk));
            }
            TRANSFER_EXPORT => {
                let export = r.u64()?;
                self.exports.insert(export, Given::new(manifest(&mut r)?));
            }
            TRANSFER_SERVE => put_served(&mut out, &serve(self, &mut r)?),
            other => return Err(no_op(other)),
        }
        Ok(out)
    }
}

/// FETCHED: the fetch's export taken, its manifest answered.
fn fetched(t: &mut TransferServe, r: &mut Reader<'_>, out: &mut Vec<u8>) -> Result<()> {
    let fetch = r.u64()?;
    let f = t.fetches.remove(&fetch).ok_or_else(|| no_op(TRANSFER_FETCHED))?;
    let source = f.fetched()?;
    let files = source
        .files()
        .iter()
        .map(|file| {
            let owner = match &file.owner {
                FileOwner::Plugin(name) => name.clone(),
                FileOwner::Database | FileOwner::Log | FileOwner::Spill | FileOwner::Server | FileOwner::Unknown => String::new(),
            };
            (file.within.to_string_lossy().replace('\\', "/"), file.length, owner)
        })
        .collect();
    put_manifest(out, &Manifest { image: source.image().to_vec(), files });
    t.fetched.insert(fetch, source);
    Ok(())
}

/// SERVE: the request answered out of the export, or a piece of it asked
/// for first.
fn serve(t: &mut TransferServe, r: &mut Reader<'_>) -> Result<Served> {
    let (to, export, frame) = (ReplicaId(r.u32()?), r.u64()?, r.bytes()?.to_vec());
    let piece = given(r)?;
    let source = t.exports.get_mut(&export).ok_or_else(|| no_op(TRANSFER_SERVE))?;
    if let Some((file, at, bytes)) = piece {
        source.pieces.insert((file, at), bytes);
    }
    source.need = None;
    match t.transfer.serve(source, (to, export), &frame) {
        Ok(answer) => {
            source.pieces.clear();
            Ok(answer.map_or(Served::Nothing, Served::Answer))
        }
        Err(e) => source.need.take().map(|(file, at, length)| Served::Need(file, at, length)).ok_or(e),
    }
}

/// An export as the host told it, its files read through the pieces the
/// host gives: a piece not given yet is asked for, the read failing.
struct Given {
    image: Vec<u8>,
    files: Vec<ExportedFile>,
    pieces: BTreeMap<(u32, u64), Vec<u8>>,
    need: Option<(u32, u64, u32)>,
}

impl Given {
    /// Of `manifest`.
    fn new(manifest: Manifest) -> Given {
        let files = manifest
            .files
            .into_iter()
            .map(|(within, length, owner)| ExportedFile {
                path: PathBuf::new(),
                within: PathBuf::from(within),
                length,
                owner: if owner.is_empty() { FileOwner::Database } else { FileOwner::Plugin(owner) },
            })
            .collect();
        Given { image: manifest.image, files, pieces: BTreeMap::new(), need: None }
    }
}

impl StateSource for Given {
    fn image(&self) -> &[u8] {
        &self.image
    }

    fn files(&self) -> &[ExportedFile] {
        &self.files
    }

    fn read(&mut self, file: usize, at: u64, into: &mut [u8]) -> Result<usize> {
        let key = (u32::try_from(file).unwrap(), at);
        if let Some(piece) = self.pieces.get(&key) {
            let n = piece.len().min(into.len());
            into[..n].copy_from_slice(&piece[..n]);
            return Ok(n);
        }
        self.need = Some((key.0, at, u32::try_from(into.len()).unwrap_or(u32::MAX)));
        Err(SqlError::new(Code::Io, "[TOOL] a piece the host has not given yet"))
    }
}
