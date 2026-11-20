//! \[EXTENSION\] The crate's inputs, outputs, configurations and journals
//! as the payloads of a host plug-in carry them
//! (`turborel::plugin::payload::replication`), both ways (RULES.md
//! D271, D273): the bridges send the crate's and read the program's, a
//! stand-in program reads the crate's and answers its own.

use std::collections::BTreeMap;

use turborel::plugin::payload::replication::consensus::{Event, Input as WireInput, Output as WireOutput};
use turborel::plugin::payload::replication::{Held, Member as WireMember};

use crate::{Configuration, Input, Lsn, Mark, Member, Output, Recovered, ReplicaEvent, ReplicaId, Role};

/// A configuration, as payloads carry it.
pub fn configuration_to(configuration: &[Member]) -> Vec<WireMember> {
    configuration.iter().map(|m| (m.id.0, m.address.clone())).collect()
}

/// A configuration a payload carried.
pub fn configuration_from(members: Vec<WireMember>) -> Configuration {
    members.into_iter().map(|(id, address)| Member { id: ReplicaId(id), address }).collect()
}

/// What a journal held, as OPEN answers it; `durable` of the journal.
pub fn recovered_to(recovered: &Recovered, durable: bool) -> Held {
    let marks = recovered.marks.iter().map(|(at, m)| (at.0, m.epoch, configuration_to(&m.configuration))).collect();
    Held { durable, state: recovered.state.clone(), first: recovered.first.0, records: recovered.records.clone(), marks }
}

/// What a journal's OPEN answered, as the crate recovers from it.
pub fn recovered_from(held: Held) -> Recovered {
    let marks: BTreeMap<Lsn, Mark> =
        held.marks.into_iter().map(|(at, epoch, members)| (Lsn(at), Mark { epoch, configuration: configuration_from(members) })).collect();
    Recovered { state: held.state, first: Lsn(held.first), records: held.records, marks }
}

/// An input, as STEP carries it.
pub fn input_to(input: Input) -> WireInput {
    match input {
        Input::Append(record) => WireInput::Append(record),
        Input::Received { from, frame } => WireInput::Received(from.0, frame),
        Input::Tick => WireInput::Tick,
        Input::Persisted(at) => WireInput::Persisted(at.0),
        Input::Applied(at) => WireInput::Applied(at.0),
        Input::Installed { holds, records } => WireInput::Installed(holds.0, records),
        Input::Retain(at) => WireInput::Retain(at.0),
        Input::Reconfigure { record, configuration } => WireInput::Reconfigure(record, configuration_to(&configuration)),
        Input::Opened(at) => WireInput::Opened(at.0),
    }
}

/// An input STEP carried.
pub fn input_from(input: WireInput) -> Input {
    match input {
        WireInput::Append(record) => Input::Append(record),
        WireInput::Received(from, frame) => Input::Received { from: ReplicaId(from), frame },
        WireInput::Tick => Input::Tick,
        WireInput::Persisted(at) => Input::Persisted(Lsn(at)),
        WireInput::Applied(at) => Input::Applied(Lsn(at)),
        WireInput::Installed(holds, records) => Input::Installed { holds: Lsn(holds), records },
        WireInput::Retain(at) => Input::Retain(Lsn(at)),
        WireInput::Reconfigure(record, members) => Input::Reconfigure { record, configuration: configuration_from(members) },
        WireInput::Opened(at) => Input::Opened(Lsn(at)),
    }
}

/// An output, as STEP answers it.
pub fn output_to(output: Output) -> WireOutput {
    match output {
        Output::Assigned(at) => WireOutput::Assigned(at.0),
        Output::Persist { at, record } => WireOutput::Persist(at.0, record),
        Output::Truncate { after } => WireOutput::Truncate(after.0),
        Output::Retain { through } => WireOutput::Retain(through.0),
        Output::PersistState(state) => WireOutput::PersistState(state),
        Output::Send { to, frame } => WireOutput::Send(to.0, frame),
        Output::Committed(at) => WireOutput::Committed(at.0),
        Output::Deliver { through } => WireOutput::Deliver(through.0),
        Output::Outcome { at, kept } => WireOutput::Outcome(at.0, kept),
        Output::Role(Role::Primary { view }) => WireOutput::Role(Some(view)),
        Output::Role(Role::Backup) => WireOutput::Role(None),
        Output::FetchState { from } => WireOutput::FetchState(from.0),
        Output::Mark { at, mark } => WireOutput::Mark(at.0, mark.epoch, configuration_to(&mark.configuration)),
        Output::Reach(configuration) => WireOutput::Reach(configuration_to(&configuration)),
        Output::Epoch { epoch, configuration } => WireOutput::Epoch(epoch, configuration_to(&configuration)),
        Output::Retire => WireOutput::Retire,
        Output::Event(event) => WireOutput::Event(event_to(event)),
    }
}

/// An output STEP answered.
pub fn output_from(output: WireOutput) -> Output {
    match output {
        WireOutput::Assigned(at) => Output::Assigned(Lsn(at)),
        WireOutput::Persist(at, record) => Output::Persist { at: Lsn(at), record },
        WireOutput::Truncate(after) => Output::Truncate { after: Lsn(after) },
        WireOutput::Retain(through) => Output::Retain { through: Lsn(through) },
        WireOutput::PersistState(state) => Output::PersistState(state),
        WireOutput::Send(to, frame) => Output::Send { to: ReplicaId(to), frame },
        WireOutput::Committed(at) => Output::Committed(Lsn(at)),
        WireOutput::Deliver(through) => Output::Deliver { through: Lsn(through) },
        WireOutput::Outcome(at, kept) => Output::Outcome { at: Lsn(at), kept },
        WireOutput::Role(Some(view)) => Output::Role(Role::Primary { view }),
        WireOutput::Role(None) => Output::Role(Role::Backup),
        WireOutput::FetchState(from) => Output::FetchState { from: ReplicaId(from) },
        WireOutput::Mark(at, epoch, members) => {
            Output::Mark { at: Lsn(at), mark: Mark { epoch, configuration: configuration_from(members) } }
        }
        WireOutput::Reach(members) => Output::Reach(configuration_from(members)),
        WireOutput::Epoch(epoch, members) => Output::Epoch { epoch, configuration: configuration_from(members) },
        WireOutput::Retire => Output::Retire,
        WireOutput::Event(event) => Output::Event(event_from(event)),
    }
}

/// An event, as an output carries it.
fn event_to(event: ReplicaEvent) -> Event {
    match event {
        ReplicaEvent::ViewChanged { view, primary } => Event::ViewChanged(view, primary.0),
        ReplicaEvent::FrameRefused { from, why } => Event::FrameRefused(from.0, why),
        ReplicaEvent::AppendRefused => Event::AppendRefused,
        ReplicaEvent::Recovering => Event::Recovering,
        ReplicaEvent::Recovered { view, from } => Event::Recovered(view, from.0),
        ReplicaEvent::Reconfigured { epoch, configuration } => Event::Reconfigured(epoch, configuration_to(&configuration)),
        ReplicaEvent::Removed => Event::Removed,
    }
}

/// An event an output carried.
fn event_from(event: Event) -> ReplicaEvent {
    match event {
        Event::ViewChanged(view, primary) => ReplicaEvent::ViewChanged { view, primary: ReplicaId(primary) },
        Event::FrameRefused(from, why) => ReplicaEvent::FrameRefused { from: ReplicaId(from), why },
        Event::AppendRefused => ReplicaEvent::AppendRefused,
        Event::Recovering => ReplicaEvent::Recovering,
        Event::Recovered(view, from) => ReplicaEvent::Recovered { view, from: ReplicaId(from) },
        Event::Reconfigured(epoch, members) => ReplicaEvent::Reconfigured { epoch, configuration: configuration_from(members) },
        Event::Removed => ReplicaEvent::Removed,
    }
}

#[cfg(test)]
mod tests;
