//! \[DESIGN\] The payloads of a consensus served by a host plug-in (kind
//! 35, RULES.md D273): the inputs a driver steps it with and the outputs
//! it answers, as the replication crate's `Input` and `Output` are, each a
//! tag and its fields, so a program in any language is a consensus.
//!
//! ```text
//! CONSENSUS 1 open      me u32 | configuration | recovered (u8 0, or 1 and held) | nonce u64  -> name str
//!           2 step      input                                -> count u32 | output...
//!           3 describe  (nothing)                            -> str
//! input:  1 append record | 2 received from u32, frame | 3 tick | 4 persisted u64 | 5 applied u64
//!         | 6 installed holds u64, records | 7 retain u64 | 8 reconfigure record, configuration | 9 opened u64
//! output: 1 assigned u64 | 2 persist at u64, record | 3 truncate u64 | 4 retain u64 | 5 state bytes
//!         | 6 send to u32, frame | 7 committed u64 | 8 deliver u64 | 9 outcome at u64, kept u8 (0 no, 1 yes, 2 unknown)
//!         | 10 role u8 (0 backup, 1 primary) and a primary's view u64 | 11 fetch from u32 | 12 mark at u64, mark
//!         | 13 reach configuration | 14 epoch u64, configuration | 15 retire | 16 event
//! event:  1 view changed view u64, primary u32 | 2 frame refused from u32, why str | 3 append refused
//!         | 4 recovering | 5 recovered view u64, from u32 | 6 reconfigured epoch u64, configuration | 7 removed
//! ```
//!
//! STEP answers every output of its input in one RESULT, so a program
//! that ends mid-step has its host carry out none of them.

use crate::bytes::{Put, Reader};
use crate::error::Result;

use super::{Held, Member, configuration, held, put_configuration, put_held, put_mark};
use crate::plugin::payload::count;

/// A consensus opened.
pub const CONSENSUS_OPEN: u8 = 1;
/// A consensus stepped with an input.
pub const CONSENSUS_STEP: u8 = 2;
/// Where a consensus stands, in words.
pub const CONSENSUS_DESCRIBE: u8 = 3;

/// An input, as the replication crate's `Input`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// The primary's database appended a record.
    Append(Vec<u8>),
    /// A frame arrived from a replica.
    Received(u32, Vec<u8>),
    /// A tick.
    Tick,
    /// The journal made the record at a position durable.
    Persisted(u64),
    /// The replay applied the records through a position.
    Applied(u64),
    /// A state transfer installed: where its database holds the log through,
    /// and the records after it.
    Installed(u64, Vec<Vec<u8>>),
    /// The records through a position may go.
    Retain(u64),
    /// A reconfiguration asked: the database's record of it, and the members.
    Reconfigure(Vec<u8>, Vec<Member>),
    /// The database, opened, holds the log through a position.
    Opened(u64),
}

/// An event for the operator, as the replication crate's `ReplicaEvent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A view entered, and its primary.
    ViewChanged(u64, u32),
    /// A frame refused, from whom and why.
    FrameRefused(u32, String),
    /// An append refused.
    AppendRefused,
    /// Recovering from the group.
    Recovering,
    /// Recovered: the view, and whose log.
    Recovered(u64, u32),
    /// An epoch entered, and its members.
    Reconfigured(u64, Vec<Member>),
    /// Removed from the group.
    Removed,
}

/// An output, as the replication crate's `Output`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// The position of the record appended.
    Assigned(u64),
    /// A record to write at a position.
    Persist(u64, Vec<u8>),
    /// The records after a position to remove.
    Truncate(u64),
    /// The records through a position to let go.
    Retain(u64),
    /// The protocol's state to write, durable at once.
    PersistState(Vec<u8>),
    /// A frame to send a replica.
    Send(u32, Vec<u8>),
    /// Every record through a position committed.
    Committed(u64),
    /// The records through a position to deliver.
    Deliver(u64),
    /// Whether the log kept a record appended in a service ended.
    Outcome(u64, Option<bool>),
    /// Serve as primary of a view, or, none, stop.
    Role(Option<u64>),
    /// Fetch a state transfer from a replica.
    FetchState(u32),
    /// A record marked a reconfiguration: its position, epoch and members.
    Mark(u64, u64, Vec<Member>),
    /// Replicas to reach.
    Reach(Vec<Member>),
    /// An epoch entered, and its members.
    Epoch(u64, Vec<Member>),
    /// The replica removed.
    Retire,
    /// An event for the operator.
    Event(Event),
}

/// OPEN's payload: the replica, its configuration, what its journal held
/// if it is made again, and its recovery's nonce.
pub fn put_open(out: &mut Vec<u8>, (me, members): (u32, &[Member]), recovered: Option<&Held>, nonce: u64) {
    out.put_u32(me);
    put_configuration(out, members);
    match recovered {
        Some(h) => {
            out.put_u8(1);
            put_held(out, h);
        }
        None => out.put_u8(0),
    }
    out.put_u64(nonce);
}

/// What `put_open` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn open(r: &mut Reader<'_>) -> Result<(u32, Vec<Member>, Option<Held>, u64)> {
    let me = r.u32()?;
    let members = configuration(r)?;
    let recovered = if r.u8()? == 0 { None } else { Some(held(r)?) };
    Ok((me, members, recovered, r.u64()?))
}

/// An input.
pub fn put_input(out: &mut Vec<u8>, input: &Input) {
    match input {
        Input::Append(record) => tagged_bytes(out, 1, record),
        Input::Received(from, frame) => {
            out.put_u8(2);
            out.put_u32(*from);
            out.put_bytes(frame);
        }
        Input::Tick => out.put_u8(3),
        Input::Persisted(at) => tagged_u64(out, 4, *at),
        Input::Applied(at) => tagged_u64(out, 5, *at),
        Input::Installed(holds, records) => {
            tagged_u64(out, 6, *holds);
            put_records(out, records);
        }
        Input::Retain(at) => tagged_u64(out, 7, *at),
        Input::Reconfigure(record, members) => {
            tagged_bytes(out, 8, record);
            put_configuration(out, members);
        }
        Input::Opened(at) => tagged_u64(out, 9, *at),
    }
}

/// What `put_input` wrote.
///
/// # Errors
///
/// `Reader`'s; malformed of a tag no input's.
pub fn input(r: &mut Reader<'_>) -> Result<Input> {
    Ok(match r.u8()? {
        1 => Input::Append(r.bytes()?.to_vec()),
        2 => Input::Received(r.u32()?, r.bytes()?.to_vec()),
        3 => Input::Tick,
        4 => Input::Persisted(r.u64()?),
        5 => Input::Applied(r.u64()?),
        6 => Input::Installed(r.u64()?, records(r)?),
        7 => Input::Retain(r.u64()?),
        8 => Input::Reconfigure(r.bytes()?.to_vec(), configuration(r)?),
        9 => Input::Opened(r.u64()?),
        other => return Err(r.corrupt(&format!("no input has the tag {other}"))),
    })
}

/// Outputs, counted.
pub fn put_outputs(out: &mut Vec<u8>, outputs: &[Output]) {
    out.put_u32(count(outputs.len()));
    for o in outputs {
        put_output(out, o);
    }
}

/// What `put_outputs` wrote.
///
/// # Errors
///
/// As `output`.
pub fn outputs(r: &mut Reader<'_>) -> Result<Vec<Output>> {
    (0..r.u32()?).map(|_| output(r)).collect()
}

/// An output.
fn put_output(out: &mut Vec<u8>, output: &Output) {
    match output {
        Output::Assigned(at) => tagged_u64(out, 1, *at),
        Output::Persist(at, record) => {
            tagged_u64(out, 2, *at);
            out.put_bytes(record);
        }
        Output::Truncate(at) => tagged_u64(out, 3, *at),
        Output::Retain(at) => tagged_u64(out, 4, *at),
        Output::PersistState(state) => tagged_bytes(out, 5, state),
        Output::Send(to, frame) => {
            out.put_u8(6);
            out.put_u32(*to);
            out.put_bytes(frame);
        }
        Output::Committed(at) => tagged_u64(out, 7, *at),
        Output::Deliver(at) => tagged_u64(out, 8, *at),
        Output::Outcome(at, kept) => {
            tagged_u64(out, 9, *at);
            out.put_u8(kept.map_or(KEPT_UNKNOWN, u8::from));
        }
        Output::Role(view) => {
            out.put_u8(10);
            put_role(out, *view);
        }
        Output::FetchState(from) => {
            out.put_u8(11);
            out.put_u32(*from);
        }
        Output::Mark(at, epoch, members) => {
            tagged_u64(out, 12, *at);
            put_mark(out, *epoch, members);
        }
        Output::Reach(members) => {
            out.put_u8(13);
            put_configuration(out, members);
        }
        Output::Epoch(epoch, members) => put_mark_tagged(out, 14, *epoch, members),
        Output::Retire => out.put_u8(15),
        Output::Event(event) => {
            out.put_u8(16);
            put_event(out, event);
        }
    }
}

/// An output read.
///
/// # Errors
///
/// `Reader`'s; malformed of a tag no output's.
fn output(r: &mut Reader<'_>) -> Result<Output> {
    Ok(match r.u8()? {
        1 => Output::Assigned(r.u64()?),
        2 => Output::Persist(r.u64()?, r.bytes()?.to_vec()),
        3 => Output::Truncate(r.u64()?),
        4 => Output::Retain(r.u64()?),
        5 => Output::PersistState(r.bytes()?.to_vec()),
        6 => Output::Send(r.u32()?, r.bytes()?.to_vec()),
        7 => Output::Committed(r.u64()?),
        8 => Output::Deliver(r.u64()?),
        9 => Output::Outcome(r.u64()?, kept(r)?),
        10 => Output::Role(role(r)?),
        11 => Output::FetchState(r.u32()?),
        12 => Output::Mark(r.u64()?, r.u64()?, configuration(r)?),
        13 => Output::Reach(configuration(r)?),
        14 => Output::Epoch(r.u64()?, configuration(r)?),
        15 => Output::Retire,
        16 => Output::Event(event(r)?),
        other => return Err(r.corrupt(&format!("no output has the tag {other}"))),
    })
}

/// An outcome's truth: unknown.
const KEPT_UNKNOWN: u8 = 2;

/// An outcome's truth read.
fn kept(r: &mut Reader<'_>) -> Result<Option<bool>> {
    match r.u8()? {
        0 => Ok(Some(false)),
        1 => Ok(Some(true)),
        KEPT_UNKNOWN => Ok(None),
        other => Err(r.corrupt(&format!("no outcome is {other}"))),
    }
}

/// A role: backup, or primary of a view.
fn put_role(out: &mut Vec<u8>, view: Option<u64>) {
    match view {
        Some(v) => tagged_u64(out, 1, v),
        None => out.put_u8(0),
    }
}

/// A role read.
fn role(r: &mut Reader<'_>) -> Result<Option<u64>> {
    if r.u8()? == 0 { Ok(None) } else { r.u64().map(Some) }
}

/// An event.
fn put_event(out: &mut Vec<u8>, event: &Event) {
    match event {
        Event::ViewChanged(view, primary) => {
            tagged_u64(out, 1, *view);
            out.put_u32(*primary);
        }
        Event::FrameRefused(from, why) => {
            out.put_u8(2);
            out.put_u32(*from);
            out.put_str(why);
        }
        Event::AppendRefused => out.put_u8(3),
        Event::Recovering => out.put_u8(4),
        Event::Recovered(view, from) => {
            tagged_u64(out, 5, *view);
            out.put_u32(*from);
        }
        Event::Reconfigured(epoch, members) => put_mark_tagged(out, 6, *epoch, members),
        Event::Removed => out.put_u8(7),
    }
}

/// An event read.
fn event(r: &mut Reader<'_>) -> Result<Event> {
    Ok(match r.u8()? {
        1 => Event::ViewChanged(r.u64()?, r.u32()?),
        2 => Event::FrameRefused(r.u32()?, r.str()?),
        3 => Event::AppendRefused,
        4 => Event::Recovering,
        5 => Event::Recovered(r.u64()?, r.u32()?),
        6 => Event::Reconfigured(r.u64()?, configuration(r)?),
        7 => Event::Removed,
        other => return Err(r.corrupt(&format!("no event has the tag {other}"))),
    })
}

/// A tag and a u64.
fn tagged_u64(out: &mut Vec<u8>, tag: u8, v: u64) {
    out.put_u8(tag);
    out.put_u64(v);
}

/// A tag and bytes.
fn tagged_bytes(out: &mut Vec<u8>, tag: u8, v: &[u8]) {
    out.put_u8(tag);
    out.put_bytes(v);
}

/// A tag, an epoch and its members.
fn put_mark_tagged(out: &mut Vec<u8>, tag: u8, epoch: u64, members: &[Member]) {
    out.put_u8(tag);
    put_mark(out, epoch, members);
}

/// Records, counted.
fn put_records(out: &mut Vec<u8>, records: &[Vec<u8>]) {
    out.put_u32(count(records.len()));
    for record in records {
        out.put_bytes(record);
    }
}

/// What `put_records` wrote.
fn records(r: &mut Reader<'_>) -> Result<Vec<Vec<u8>>> {
    (0..r.u32()?).map(|_| r.bytes().map(<[u8]>::to_vec)).collect()
}
