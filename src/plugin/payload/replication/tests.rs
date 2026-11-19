//! Unit tests of the payloads of replication's host kinds: each written
//! and read back as it was, and a malformed one refused.

use super::consensus::{Event, Input, Output, input, open, outputs, put_input, put_open, put_outputs};
use super::*;
use crate::error::Code;
use crate::plugin::payload::reader;

/// Verifies: [EXTENSION] RULES.md D270, D271, D272: a journal's held
/// state, an export's manifest, frames arrived and SERVE's three outcomes
/// read back as they were written.
#[test]
fn replication_payloads_read_back_as_written() {
    let members = vec![(1, "a:1".to_owned()), (2, "b:2".to_owned())];
    let h = Held { durable: true, state: Some(vec![9]), first: 4, records: vec![vec![1], vec![2, 3]], marks: vec![(5, 2, members.clone())] };
    let mut out = Vec::new();
    put_held(&mut out, &h);
    put_manifest(&mut out, &Manifest { image: vec![7], files: vec![("f".to_owned(), 3, String::new())] });
    put_arrived(&mut out, &[(2, vec![4, 5])]);
    for s in [Served::Nothing, Served::Answer(vec![1]), Served::Need(1, 2, 3)] {
        put_served(&mut out, &s);
    }
    put_given(&mut out, Some((1, 8, &[6])));
    put_given(&mut out, None);
    let mut r = reader(&out);
    assert_eq!(held(&mut r).unwrap(), h);
    assert_eq!(manifest(&mut r).unwrap().files, vec![("f".to_owned(), 3, String::new())]);
    assert_eq!(arrived(&mut r).unwrap(), vec![(2, vec![4, 5])]);
    assert_eq!(served(&mut r).unwrap(), Served::Nothing);
    assert_eq!(served(&mut r).unwrap(), Served::Answer(vec![1]));
    assert_eq!(served(&mut r).unwrap(), Served::Need(1, 2, 3));
    assert_eq!(given(&mut r).unwrap(), Some((1, 8, vec![6])));
    assert_eq!(given(&mut r).unwrap(), None);
    r.finish().unwrap();
    assert_ne!(chunk_digest(b"a"), chunk_digest(b"b"));
}

/// Verifies: [EXTENSION] RULES.md D273: every input, output and event of
/// a consensus read back as written.
#[test]
fn consensus_inputs_and_outputs_read_back() {
    let members = vec![(1, "a".to_owned())];
    let inputs = [
        Input::Append(vec![1]),
        Input::Received(2, vec![3]),
        Input::Tick,
        Input::Persisted(4),
        Input::Applied(5),
        Input::Installed(6, vec![vec![7]]),
        Input::Retain(8),
        Input::Reconfigure(vec![9], members.clone()),
        Input::Opened(10),
    ];
    for i in &inputs {
        let mut out = Vec::new();
        put_input(&mut out, i);
        assert_eq!(&input(&mut reader(&out)).unwrap(), i);
    }
    let events = [
        Event::ViewChanged(1, 2),
        Event::FrameRefused(3, "why".to_owned()),
        Event::AppendRefused,
        Event::Recovering,
        Event::Recovered(4, 5),
        Event::Reconfigured(6, members.clone()),
        Event::Removed,
    ];
    let mut all = vec![
        Output::Assigned(1),
        Output::Persist(2, vec![3]),
        Output::Truncate(4),
        Output::Retain(5),
        Output::PersistState(vec![6]),
        Output::Send(7, vec![8]),
        Output::Committed(9),
        Output::Deliver(10),
        Output::Outcome(11, Some(true)),
        Output::Outcome(12, Some(false)),
        Output::Outcome(13, None),
        Output::Role(Some(14)),
        Output::Role(None),
        Output::FetchState(15),
        Output::Mark(16, 17, members.clone()),
        Output::Reach(members.clone()),
        Output::Epoch(18, members.clone()),
        Output::Retire,
    ];
    all.extend(events.into_iter().map(Output::Event));
    let mut out = Vec::new();
    put_outputs(&mut out, &all);
    assert_eq!(outputs(&mut reader(&out)).unwrap(), all);
}

/// Verifies: [EXTENSION] RULES.md D273: OPEN reads back as written, and
/// an unknown tag of an input, output or outcome is refused as malformed.
#[test]
fn consensus_open_reads_back_and_unknown_tags_are_refused() {
    let members = vec![(1, "a".to_owned())];
    let mut opened = Vec::new();
    put_open(&mut opened, (3, &members), Some(&Held::default()), 77);
    assert_eq!(open(&mut reader(&opened)).unwrap(), (3, members, Some(Held::default()), 77));
    assert_eq!(input(&mut reader(&[99])).err().map(|e| e.code), Some(Code::Connection));
    assert!(outputs(&mut reader(&[1, 0, 0, 0, 99])).is_err());
    assert!(served(&mut reader(&[7])).is_err());
}
