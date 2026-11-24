//! \[EXTENSION\] The routine debugger's messages, with the capability
//! `debugger` (RULES.md D1030 to D1034): what a debugger's connection asks
//! of the session it debugs, another connection's, and what the server
//! answers and tells it unasked as that session pauses. The debugger
//! itself, the pausing and stepping of routines, is the database's
//! (`turborel::debugger`); these messages carry its calls and what they
//! give.
//!
//! ```text
//! client 'V' debug:      what u8, then
//!                        0 identify
//!                        1 attach: connection u64
//!                        2 detach
//!                        3 break: kind u8, schema, name, line u32,
//!                          conditioned u8, the condition if 1
//!                        4 clear: number u32
//!                        5 breakpoints
//!                        6 resume: action u8
//!                        7 interrupt
//!                        8 stack
//! server 'V' debugged:   what u8, then
//!                        0 connection: its ID u64
//!                        1 breakpoint: a breakpoint
//!                        2 breakpoints: their count u32, then each
//!                        3 stack: paused u8, the pause if 1
//!        'W' event:      what u8, then
//!                        0 paused: the pause
//!                        1 timed out, 2 cancelled, 3 ended
//! ```
//!
//! A kind is 0 a procedure, 1 a function, 2 a trigger; an action 0 into,
//! 1 over, 2 out, 3 continue. The pause, its frames and a breakpoint are
//! as `debug/paused.rs` writes them.

mod paused;

use turborel::bytes::{Put, Reader};
use turborel::catalog::QualifiedName;
use turborel::debugger::{Action, Kind, Paused};
use turborel::error::Result;

use crate::fields::{counted, flag, put_count, put_flag};
use paused::{breakpoint, kind, name, pause, put_breakpoint, put_kind, put_name, put_pause};
pub(crate) use paused::{event, put_event};

/// \[EXTENSION\] A breakpoint, as the server tells it a debugger set it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugBreakpoint {
    /// Its number, from 1, which the debugger clears it by.
    pub number: u32,
    /// The specific name of its routine, or the name of its trigger.
    pub routine: QualifiedName,
    /// Its line of the routine's text, from 1.
    pub line: u32,
    /// The `<search condition>` it pauses only where true, if given.
    pub condition: Option<String>,
}

/// \[EXTENSION\] What a debugger's connection asks (RULES.md D1030).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugRequest {
    /// The ID of this connection, which another attaches to it by.
    Identify,
    /// This connection attached, as a debugger, to the session of the
    /// connection of this ID (RULES.md D1031).
    Attach(u64),
    /// This connection detached: its breakpoints gone, its target, if
    /// paused, going on.
    Detach,
    /// A breakpoint at a line of the text of a routine or a trigger.
    Break {
        /// What `routine` names.
        kind: Kind,
        /// The routine's specific name, or its name if it is the one of
        /// that kind so named; the trigger's name. An empty schema is the
        /// debugger's session's default.
        routine: QualifiedName,
        /// The line, from 1.
        line: u32,
        /// A `<search condition>` over the frame's parameters and
        /// variables, if given.
        condition: Option<String>,
    },
    /// The breakpoint of this number cleared.
    Clear(u32),
    /// The breakpoints set, in the order they were set.
    Breakpoints,
    /// The paused target told how to go on.
    Resume(Action),
    /// The running target told to pause at its next statement.
    Interrupt,
    /// The target's pause, with its call stack, if it is paused.
    Stack,
}

/// \[EXTENSION\] What the server answers a debug request with, but a
/// completion.
#[derive(Debug, Clone)]
pub enum DebugReply {
    /// Identify's: the connection's ID.
    Connection(u64),
    /// Break's: the breakpoint set.
    Breakpoint(DebugBreakpoint),
    /// Breakpoints': each set, in order.
    Breakpoints(Vec<DebugBreakpoint>),
    /// Stack's: the target's pause, none if it runs.
    Stack(Option<Paused>),
}

const IDENTIFY: u8 = 0;
const ATTACH: u8 = 1;
const DETACH: u8 = 2;
const BREAK: u8 = 3;
const CLEAR: u8 = 4;
const BREAKPOINTS: u8 = 5;
const RESUME: u8 = 6;
const INTERRUPT: u8 = 7;
const STACK: u8 = 8;
const CONNECTION: u8 = 0;
const BREAKPOINT: u8 = 1;
const LISTED: u8 = 2;
const STACKED: u8 = 3;

/// An action, as one octet.
fn put_action(b: &mut Vec<u8>, action: Action) {
    b.put_u8(match action {
        Action::Into => 0,
        Action::Over => 1,
        Action::Out => 2,
        Action::Continue => 3,
    });
}

/// An action, as `put_action` wrote it.
fn action(r: &mut Reader<'_>) -> Result<Action> {
    match r.u8()? {
        0 => Ok(Action::Into),
        1 => Ok(Action::Over),
        2 => Ok(Action::Out),
        3 => Ok(Action::Continue),
        _ => Err(r.corrupt("not a debugger's action")),
    }
}

/// A string if `given`, after its flag.
fn put_given(b: &mut Vec<u8>, given: Option<&str>) {
    put_flag(b, given.is_some());
    if let Some(s) = given {
        b.put_str(s);
    }
}

/// A string `put_given` wrote, if it was given.
fn given(r: &mut Reader<'_>) -> Result<Option<String>> {
    if flag(r)? { Ok(Some(r.str()?)) } else { Ok(None) }
}

impl DebugRequest {
    /// Appends its body to `b`.
    pub(crate) fn put(&self, b: &mut Vec<u8>) {
        match self {
            DebugRequest::Identify => b.put_u8(IDENTIFY),
            DebugRequest::Attach(connection) => {
                b.put_u8(ATTACH);
                b.put_u64(*connection);
            }
            DebugRequest::Detach => b.put_u8(DETACH),
            DebugRequest::Break { kind, routine, line, condition } => {
                b.put_u8(BREAK);
                put_kind(b, *kind);
                put_name(b, routine);
                b.put_u32(*line);
                put_given(b, condition.as_deref());
            }
            DebugRequest::Clear(number) => {
                b.put_u8(CLEAR);
                b.put_u32(*number);
            }
            DebugRequest::Breakpoints => b.put_u8(BREAKPOINTS),
            DebugRequest::Resume(how) => {
                b.put_u8(RESUME);
                put_action(b, *how);
            }
            DebugRequest::Interrupt => b.put_u8(INTERRUPT),
            DebugRequest::Stack => b.put_u8(STACK),
        }
    }

    /// The request `r` holds, as `put` wrote it.
    pub(crate) fn read(r: &mut Reader<'_>) -> Result<DebugRequest> {
        Ok(match r.u8()? {
            IDENTIFY => DebugRequest::Identify,
            ATTACH => DebugRequest::Attach(r.u64()?),
            DETACH => DebugRequest::Detach,
            BREAK => DebugRequest::Break { kind: kind(r)?, routine: name(r)?, line: r.u32()?, condition: given(r)? },
            CLEAR => DebugRequest::Clear(r.u32()?),
            BREAKPOINTS => DebugRequest::Breakpoints,
            RESUME => DebugRequest::Resume(action(r)?),
            INTERRUPT => DebugRequest::Interrupt,
            STACK => DebugRequest::Stack,
            _ => return Err(r.corrupt("no Debug request is of this kind")),
        })
    }
}

impl DebugReply {
    /// Appends its body to `b`.
    pub(crate) fn put(&self, b: &mut Vec<u8>) {
        match self {
            DebugReply::Connection(id) => {
                b.put_u8(CONNECTION);
                b.put_u64(*id);
            }
            DebugReply::Breakpoint(set) => {
                b.put_u8(BREAKPOINT);
                put_breakpoint(b, set);
            }
            DebugReply::Breakpoints(all) => {
                b.put_u8(LISTED);
                put_count(b, all.len());
                for each in all {
                    put_breakpoint(b, each);
                }
            }
            DebugReply::Stack(paused) => {
                b.put_u8(STACKED);
                put_flag(b, paused.is_some());
                if let Some(p) = paused {
                    put_pause(b, p);
                }
            }
        }
    }

    /// The reply `r` holds, as `put` wrote it.
    pub(crate) fn read(r: &mut Reader<'_>) -> Result<DebugReply> {
        Ok(match r.u8()? {
            CONNECTION => DebugReply::Connection(r.u64()?),
            BREAKPOINT => DebugReply::Breakpoint(breakpoint(r)?),
            LISTED => DebugReply::Breakpoints(counted(r, breakpoint)?),
            STACKED => DebugReply::Stack(if flag(r)? { Some(pause(r)?) } else { None }),
            _ => return Err(r.corrupt("no Debug reply is of this kind")),
        })
    }
}
