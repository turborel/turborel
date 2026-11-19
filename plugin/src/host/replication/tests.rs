//! Unit tests of the replication host kinds' traits, each call made
//! through `Harness` as the bridges make it (RULES.md R-TEST-1).

use std::collections::BTreeMap;

use turborel::bytes::Put;
use turborel::plugin::payload::replication::consensus::{self as c, Input, Output};
use turborel::plugin::payload::replication::{self as p, Held, Manifest, Member, Served};
use turborel::plugin::payload::{self as payload};

use super::super::super::{Answer, Harness, Plugin, PluginError, Target, TargetStore, Upload};
use super::{Consensus, Journal, ReplicaTransport, StateTransfer};

/// A transport looping frames back to their sender.
#[derive(Default)]
struct Loop(Vec<(u32, Vec<u8>)>, Vec<u32>);

impl ReplicaTransport for Loop {
    fn open(&mut self, _me: u32, configuration: &[Member]) -> Answer<Vec<u32>> {
        self.1 = configuration.iter().map(|m| m.0).collect();
        Ok(self.1.clone())
    }
    fn send(&mut self, to: u32, frame: &[u8]) -> Answer<bool> {
        let linked = self.1.contains(&to);
        if linked {
            self.0.push((to, frame.to_vec()));
        }
        Ok(linked)
    }
    fn receive(&mut self) -> Answer<Vec<(u32, Vec<u8>)>> {
        Ok(std::mem::take(&mut self.0))
    }
    fn reconfigure(&mut self, configuration: &[Member]) -> Answer<Vec<u32>> {
        self.open(0, configuration)
    }
}

/// A journal in memory, every write durable at the next sync.
#[derive(Default)]
struct Memory(BTreeMap<u64, Vec<u8>>, Vec<u64>);

impl Journal for Memory {
    fn open(&mut self) -> Answer<Held> {
        Ok(Held { durable: true, records: self.0.values().cloned().collect(), ..Held::default() })
    }
    fn persist(&mut self, at: u64, record: &[u8]) -> Answer<()> {
        self.0.insert(at, record.to_vec());
        self.1.push(at);
        Ok(())
    }
    fn truncate(&mut self, after: u64) -> Answer<()> {
        self.0.retain(|&at, _| at <= after);
        Ok(())
    }
    fn retain(&mut self, _through: u64) -> Answer<()> {
        Ok(())
    }
    fn mark(&mut self, _at: u64, _epoch: u64, _members: &[Member]) -> Answer<()> {
        Err(PluginError::external("no marks"))
    }
    fn persist_state(&mut self, _state: &[u8]) -> Answer<()> {
        Ok(())
    }
    fn sync(&mut self) -> Answer<Vec<u64>> {
        Ok(std::mem::take(&mut self.1))
    }
    fn flush(&mut self) -> Answer<Vec<u64>> {
        self.sync()
    }
}

/// A consensus assigning each append the next position.
#[derive(Default)]
struct Counting(u64);

impl Consensus for Counting {
    fn open(&mut self, _me: u32, _configuration: &[Member], recovered: Option<Held>, _nonce: u64) -> Answer<()> {
        self.0 = recovered.map_or(0, |h| h.first);
        Ok(())
    }
    fn step(&mut self, input: Input) -> Answer<Vec<Output>> {
        let Input::Append(record) = input else { return Ok(Vec::new()) };
        self.0 = self.0.saturating_add(1);
        Ok(vec![Output::Assigned(self.0), Output::Persist(self.0, record)])
    }
    fn describe(&mut self) -> String {
        format!("at {}", self.0)
    }
}

/// A transfer of one file of three bytes, served in one piece.
#[derive(Default)]
struct One;

impl StateTransfer for One {
    fn begin(&mut self, _from: u32) -> Answer<(u64, Option<Vec<u8>>)> {
        Ok((7, Some(vec![1])))
    }
    fn take(&mut self, _fetch: u64, _frame: &[u8]) -> Answer<(bool, Option<Vec<u8>>)> {
        Ok((true, None))
    }
    fn fetched(&mut self, _fetch: u64) -> Answer<Manifest> {
        Ok(Manifest { image: vec![9], files: vec![("db".into(), 3, String::new())] })
    }
    fn chunk(&mut self, _fetch: u64, _file: u32, at: u64, _max: u32) -> Answer<Vec<u8>> {
        let from = usize::try_from(at).map_err(|_| PluginError::external("too far"))?;
        Ok(b"abc".get(from..).unwrap_or_default().to_vec())
    }
    fn export(&mut self, _export: u64, _manifest: Manifest) -> Answer<()> {
        Ok(())
    }
    fn serve(&mut self, _to_export: (u32, u64), _request: &[u8], given: Option<(u32, u64, Vec<u8>)>) -> Answer<Served> {
        Ok(given.map_or(Served::Need(0, 0, 3), |(_, _, piece)| Served::Answer(piece)))
    }
}

/// An archive of objects in memory.
#[derive(Default)]
struct Shelf;

impl Target for Shelf {
    fn open(&mut self, _location: &str) -> Answer<Box<dyn TargetStore>> {
        Ok(Box::new(Shelved))
    }
}

struct Shelved;

impl TargetStore for Shelved {
    fn begin(&mut self, object: &str) -> Answer<Box<dyn Upload>> {
        Err(PluginError::external(&format!("{object} is read only")))
    }
    fn read(&mut self, _object: &str, _offset: u64, _max: u32) -> Answer<Vec<u8>> {
        Ok(b"kept".to_vec())
    }
    fn list(&mut self, _prefix: &str) -> Answer<Vec<String>> {
        Ok(vec!["a".into()])
    }
    fn remove(&mut self, _object: &str) -> Answer<()> {
        Ok(())
    }
}

/// The plug-in of the tests, one object of each kind.
fn harness() -> Harness {
    let plugin = Plugin::new("1.0.0").replica_transport("LOOP", Loop::default()).journal("MEMORY", Memory::default());
    Harness::new(plugin.consensus("COUNTING", Counting::default()).state_transfer("ONE", One).archive("SHELF", Shelf))
}

/// Verifies: [EXTENSION] RULES.md D271: a transport's calls carried as
/// the bridge makes them: a frame to a member looped back, one to no
/// member refused.
#[test]
fn a_transport_answers_as_the_bridge_reads() {
    let mut h = harness();
    let mut opened = Vec::new();
    opened.put_u32(1);
    p::put_configuration(&mut opened, &[(1, "a".into()), (2, "b".into())]);
    let members = h.call(p::REPLICA_TRANSPORT, p::TRANSPORT_OPEN, "LOOP", opened).unwrap();
    assert_eq!(p::members(&mut payload::reader(&members)).unwrap(), [1, 2]);
    for (to, sent) in [(2, 1), (3, 0)] {
        let mut frame = Vec::new();
        frame.put_u32(to);
        frame.put_bytes(b"hi");
        assert_eq!(h.call(p::REPLICA_TRANSPORT, p::TRANSPORT_SEND, "LOOP", frame).unwrap(), [sent]);
    }
    let arrived = h.call(p::REPLICA_TRANSPORT, p::TRANSPORT_RECEIVE, "LOOP", Vec::new()).unwrap();
    assert_eq!(p::arrived(&mut payload::reader(&arrived)).unwrap(), [(2, b"hi".to_vec())]);
}

/// Verifies: [EXTENSION] RULES.md D272: a journal names each record
/// persisted once, by SYNC, and its ERROR is sent as one.
#[test]
fn a_journal_names_what_it_made_durable() {
    let mut h = harness();
    for at in [1_u64, 2] {
        let mut write = Vec::new();
        write.put_u64(at);
        write.put_bytes(b"r");
        h.call(p::JOURNAL, p::JOURNAL_PERSIST, "MEMORY", write).unwrap();
    }
    let named = h.call(p::JOURNAL, p::JOURNAL_SYNC, "MEMORY", Vec::new()).unwrap();
    assert_eq!(p::positions(&mut payload::reader(&named)).unwrap(), [1, 2]);
    let again = h.call(p::JOURNAL, p::JOURNAL_FLUSH, "MEMORY", Vec::new()).unwrap();
    assert_eq!(p::positions(&mut payload::reader(&again)).unwrap(), Vec::<u64>::new());
    let held = h.call(p::JOURNAL, p::JOURNAL_OPEN, "MEMORY", Vec::new()).unwrap();
    assert_eq!(p::held(&mut payload::reader(&held)).unwrap().records.len(), 2);
    let mut mark = Vec::new();
    mark.put_u64(1);
    p::put_mark(&mut mark, 1, &[]);
    assert_eq!(h.call(p::JOURNAL, p::JOURNAL_MARK, "MEMORY", mark).unwrap_err().state, "38000");
}

/// Verifies: [EXTENSION] RULES.md D273: a consensus opened from what its
/// journal held, each step's outputs answered whole.
#[test]
fn a_consensus_steps_as_the_bridge_reads() {
    let mut h = harness();
    let mut open = Vec::new();
    c::put_open(&mut open, (1, &[(1, "a".into())]), Some(&Held { first: 4, ..Held::default() }), 0);
    h.call(p::CONSENSUS, c::CONSENSUS_OPEN, "COUNTING", open).unwrap();
    let mut step = Vec::new();
    c::put_input(&mut step, &Input::Append(b"x".to_vec()));
    let out = h.call(p::CONSENSUS, c::CONSENSUS_STEP, "COUNTING", step).unwrap();
    assert_eq!(c::outputs(&mut payload::reader(&out)).unwrap(), [Output::Assigned(5), Output::Persist(5, b"x".to_vec())]);
    let told = h.call(p::CONSENSUS, c::CONSENSUS_DESCRIBE, "COUNTING", Vec::new()).unwrap();
    assert_eq!(payload::reader(&told).str().unwrap(), "at 5");
}

/// Verifies: [EXTENSION] RULES.md D272: a transfer's chunk carries the
/// digest the host checks, and SERVE asks for a piece before answering.
#[test]
fn a_transfer_gives_chunks_with_their_digest() {
    let mut h = harness();
    let mut chunk = Vec::new();
    chunk.put_u64(7);
    chunk.put_u32(0);
    chunk.put_u64(1);
    chunk.put_u32(64);
    let got = h.call(p::STATE_TRANSFER, p::TRANSFER_CHUNK, "ONE", chunk).unwrap();
    let mut r = payload::reader(&got);
    let bytes = r.bytes().unwrap().to_vec();
    assert_eq!((bytes.as_slice(), r.u64().unwrap()), (&b"bc"[..], p::chunk_digest(b"bc")));
    let serve = |given: Option<(u32, u64, &[u8])>| {
        let mut asked = Vec::new();
        asked.put_u32(2);
        asked.put_u64(1);
        asked.put_bytes(&[1]);
        p::put_given(&mut asked, given);
        asked
    };
    let need = h.call(p::STATE_TRANSFER, p::TRANSFER_SERVE, "ONE", serve(None)).unwrap();
    assert_eq!(p::served(&mut payload::reader(&need)).unwrap(), Served::Need(0, 0, 3));
    let answer = h.call(p::STATE_TRANSFER, p::TRANSFER_SERVE, "ONE", serve(Some((0, 0, b"abc")))).unwrap();
    assert_eq!(p::served(&mut payload::reader(&answer)).unwrap(), Served::Answer(b"abc".to_vec()));
}

/// Verifies: [EXTENSION] RULES.md D274: an archive's calls are a
/// target's, its stores numbered, its ERROR the call's alone.
#[test]
fn an_archive_answers_as_a_target() {
    let mut h = harness();
    assert!(h.kinds("EXAMPLE").contains(&p::ARCHIVE));
    let mut open = Vec::new();
    open.put_str("shelf://x");
    let store = h.call(p::ARCHIVE, payload::TARGET_OPEN, "SHELF", open).unwrap();
    let mut read = store.clone();
    read.put_str("a");
    read.put_u64(0);
    read.put_u32(16);
    let got = h.call(p::ARCHIVE, payload::TARGET_READ, "SHELF", read).unwrap();
    assert_eq!(payload::reader(&got).bytes().unwrap(), b"kept");
    let mut begin = store;
    begin.put_str("b");
    assert_eq!(h.call(p::ARCHIVE, payload::TARGET_BEGIN, "SHELF", begin).unwrap_err().state, "38000");
}

/// The code of a backup target's kind.
const KIND_TARGET: u8 = 8;

/// A kind 8 target whose stores read what it was named.
struct Labelled(&'static [u8]);

impl Target for Labelled {
    fn open(&mut self, _location: &str) -> Answer<Box<dyn TargetStore>> {
        Ok(Box::new(Label(self.0)))
    }
}

/// A store of `Labelled`.
struct Label(&'static [u8]);

impl TargetStore for Label {
    fn begin(&mut self, object: &str) -> Answer<Box<dyn Upload>> {
        Err(PluginError::external(&format!("{object} is read only")))
    }
    fn read(&mut self, _object: &str, _offset: u64, _max: u32) -> Answer<Vec<u8>> {
        Ok(self.0.to_vec())
    }
    fn list(&mut self, _prefix: &str) -> Answer<Vec<String>> {
        Ok(Vec::new())
    }
    fn remove(&mut self, _object: &str) -> Answer<()> {
        Ok(())
    }
}

/// Verifies: [EXTENSION] RULES.md D274: an archive and a kind 8 target of
/// one name are two objects, each opening its own stores, numbered apart.
#[test]
fn an_archive_and_a_target_of_one_name_stay_apart() {
    let plugin = Plugin::new("1.0.0").target("SHELF", Labelled(b"target")).archive("SHELF", Shelf);
    let mut h = Harness::new(plugin);
    let mut open = Vec::new();
    open.put_str("shelf://x");
    let archive_store = h.call(p::ARCHIVE, payload::TARGET_OPEN, "SHELF", open.clone()).unwrap();
    let target_store = h.call(KIND_TARGET, payload::TARGET_OPEN, "SHELF", open).unwrap();
    let read_of = |store: &[u8]| {
        let mut read = store.to_vec();
        read.put_str("a");
        read.put_u64(0);
        read.put_u32(16);
        read
    };
    let got = h.call(KIND_TARGET, payload::TARGET_READ, "SHELF", read_of(&target_store)).unwrap();
    assert_eq!(payload::reader(&got).bytes().unwrap(), b"target");
    let got = h.call(p::ARCHIVE, payload::TARGET_READ, "SHELF", read_of(&archive_store)).unwrap();
    assert_eq!(payload::reader(&got).bytes().unwrap(), b"kept");
}
