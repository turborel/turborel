//! Unit tests of the bridges of replication's host kinds, over pipes a
//! test answers, each call as a program's process would answer it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use turborel::bytes::Put;
use turborel::error::{Code, Result};
use turborel::plugin::errors::ended;
use turborel::plugin::payload::replication::{Held, put_arrived, put_held, put_members, put_positions};

use super::*;
use crate::{Consensus, Input, Journal, Lsn, Member, ReplicaId, Transport};

/// A pipe answering each call with the next answer given, recording the
/// operations asked.
struct Scripted {
    answers: VecDeque<Result<Vec<u8>>>,
    asked: Arc<Mutex<Vec<u8>>>,
}

impl Pipe for Scripted {
    fn call(&mut self, op: u8, _payload: Vec<u8>) -> Result<Vec<u8>> {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push(op);
        }
        self.answers.pop_front().unwrap_or_else(|| Err(ended("no answer left")))
    }
}

/// A scripted pipe of `answers`, and the operations it is asked.
fn scripted(answers: Vec<Option<Vec<u8>>>) -> (Box<dyn Pipe>, Arc<Mutex<Vec<u8>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let answers = answers.into_iter().map(|a| a.ok_or_else(|| ended("killed mid-message"))).collect();
    (Box::new(Scripted { answers, asked: Arc::clone(&asked) }), asked)
}

/// What `write` writes, as a pipe's answer.
fn written(write: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut out = Vec::new();
    write(&mut out);
    out
}

/// Verifies: [EXTENSION] RULES.md D271: a plug-in's transport keeps the
/// members OPEN linked, sends a frame as one call, refuses a frame to no
/// member as -910 (dropped by the driver as lost), brings frames arrived
/// in whole; once its process ends, every later call fails with that
/// failure and no call is made.
#[test]
fn a_plugin_transport_fails_stop() {
    let (pipe, asked) = scripted(vec![
        Some(written(|o| put_members(o, &[1, 2]))),
        Some(written(|o| o.put_u8(1))),
        Some(written(|o| o.put_u8(0))),
        Some(written(|o| put_arrived(o, &[(2, vec![7, 8])]))),
        None,
    ]);
    let members = vec![Member { id: ReplicaId(1), address: "a".into() }, Member { id: ReplicaId(2), address: "b".into() }];
    let t = PluginTransport::open(pipe, ReplicaId(1), &members).unwrap();
    assert_eq!(t.members(), vec![ReplicaId(1), ReplicaId(2)]);
    t.send(ReplicaId(2), b"f").unwrap();
    assert_eq!(t.send(ReplicaId(9), b"f").unwrap_err().code, Code::Api);
    assert_eq!(t.receive().unwrap(), vec![(ReplicaId(2), vec![7, 8])]);
    let died = t.send(ReplicaId(2), b"f").unwrap_err();
    assert_ne!(died.code, Code::Api, "a death is no lost frame");
    assert_eq!(t.receive().unwrap_err().msg, died.msg);
    assert_eq!(asked.lock().unwrap().len(), 5, "no call after the failure");
    assert!(t.failed().is_some());
}

/// Verifies: [EXTENSION] RULES.md D272: a plug-in's journal acknowledges
/// the records SYNC names, in the order persisted; a record truncated
/// away is never owed; one named that was not persisted, or out of order,
/// breaks the protocol (38T03) and nothing more is acknowledged; a death
/// mid-persist acknowledges nothing.
#[test]
fn a_plugin_journal_acknowledges_only_what_it_was_given() {
    let held = Held { durable: true, state: None, first: 0, records: Vec::new(), marks: Vec::new() };
    let (pipe, _) = scripted(vec![
        Some(written(|o| put_held(o, &held))),
        Some(Vec::new()),
        Some(Vec::new()),
        Some(Vec::new()),
        Some(written(|o| put_positions(o, &[1]))),
        Some(written(|o| put_positions(o, &[3]))),
    ]);
    let (mut j, recovered) = PluginJournal::open(pipe).unwrap();
    assert!(j.durable() && recovered.records.is_empty());
    j.persist(Lsn(1), b"a").unwrap();
    j.persist(Lsn(2), b"b").unwrap();
    j.truncate(Lsn(1)).unwrap();
    assert_eq!(j.sync().unwrap(), vec![Lsn(1)]);
    let refused = j.sync().unwrap_err();
    assert_eq!(refused.state.map(|s| s.to_string()), Some("38T03".to_owned()));
    assert!(j.flush().is_err());
    let (dying, _) = scripted(vec![Some(written(|o| put_held(o, &held))), None]);
    let (mut killed, _) = PluginJournal::open(dying).unwrap();
    assert!(killed.persist(Lsn(1), b"a").is_err());
    assert!(killed.sync().is_err(), "nothing acknowledged after a death mid-append");
}

/// Verifies: [EXTENSION] RULES.md D273: a plug-in's consensus that fails
/// a step answers no output of it and reports its failure, which the
/// driver halts on.
#[test]
fn a_plugin_consensus_that_dies_answers_nothing() {
    let (pipe, _) = scripted(vec![Some(written(|o| o.put_str("stand-in"))), None]);
    let mut c = PluginConsensus::open(pipe, (&[], ReplicaId(1)), None, 1).unwrap();
    assert_eq!(c.name(), "stand-in");
    assert!(c.failed().is_none());
    let mut out = Vec::new();
    c.step(Input::Tick, &mut out);
    assert_eq!(out, Vec::new());
    assert!(c.failed().is_some());
    assert!(c.describe().contains("D270"));
}
