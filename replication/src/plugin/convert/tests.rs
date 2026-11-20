//! Unit tests: replication's inputs, outputs, events and what a journal
//! recovered, each taken to what protocol 1 carries and back unchanged
//! (RULES.md D272, D273, R-TEST-1).

use super::*;

/// A group of two members.
fn members() -> Configuration {
    vec![Member { id: ReplicaId(1), address: "a:1".into() }, Member { id: ReplicaId(2), address: "b:2".into() }]
}

/// Verifies: [EXTENSION] RULES.md D273: every input of a consensus is
/// taken to protocol 1's and back as it was.
#[test]
fn each_input_crosses_and_comes_back() {
    let inputs = [
        Input::Append(vec![1]),
        Input::Received { from: ReplicaId(2), frame: vec![3] },
        Input::Tick,
        Input::Persisted(Lsn(4)),
        Input::Applied(Lsn(5)),
        Input::Installed { holds: Lsn(6), records: vec![vec![7]] },
        Input::Retain(Lsn(8)),
        Input::Reconfigure { record: vec![9], configuration: members() },
        Input::Opened(Lsn(10)),
    ];
    for i in inputs {
        assert_eq!(input_from(input_to(i.clone())), i);
    }
}

/// Verifies: [EXTENSION] RULES.md D273: every output of a consensus, its
/// roles and events among them, is taken to protocol 1's and back as it
/// was.
#[test]
fn each_output_crosses_and_comes_back() {
    let mark = Mark { epoch: 3, configuration: members() };
    let events = [
        ReplicaEvent::ViewChanged { view: 1, primary: ReplicaId(2) },
        ReplicaEvent::FrameRefused { from: ReplicaId(1), why: "why".into() },
        ReplicaEvent::AppendRefused,
        ReplicaEvent::Recovering,
        ReplicaEvent::Recovered { view: 4, from: ReplicaId(2) },
        ReplicaEvent::Reconfigured { epoch: 5, configuration: members() },
        ReplicaEvent::Removed,
    ];
    let mut outputs = vec![
        Output::Assigned(Lsn(1)),
        Output::Persist { at: Lsn(2), record: vec![3] },
        Output::Truncate { after: Lsn(4) },
        Output::Retain { through: Lsn(5) },
        Output::PersistState(vec![6]),
        Output::Send { to: ReplicaId(1), frame: vec![7] },
        Output::Committed(Lsn(8)),
        Output::Deliver { through: Lsn(9) },
        Output::Outcome { at: Lsn(10), kept: Some(true) },
        Output::Outcome { at: Lsn(11), kept: None },
        Output::Role(Role::Primary { view: 12 }),
        Output::Role(Role::Backup),
        Output::FetchState { from: ReplicaId(2) },
        Output::Mark { at: Lsn(13), mark },
        Output::Reach(members()),
        Output::Epoch { epoch: 14, configuration: members() },
        Output::Retire,
    ];
    outputs.extend(events.into_iter().map(Output::Event));
    for o in outputs {
        assert_eq!(output_from(output_to(o.clone())), o);
    }
}

/// Verifies: [EXTENSION] RULES.md D272: what a journal recovered, its
/// state, records and marks, is taken to a held state and back as it was.
#[test]
fn what_a_journal_recovered_crosses_and_comes_back() {
    let marks = BTreeMap::from([(Lsn(5), Mark { epoch: 2, configuration: members() })]);
    let recovered = Recovered { state: Some(vec![1]), first: Lsn(3), records: vec![vec![4], vec![5]], marks };
    let held = recovered_to(&recovered, true);
    assert!(held.durable);
    assert_eq!(recovered_from(held), recovered);
    assert_eq!(configuration_from(configuration_to(&members())), members());
}
