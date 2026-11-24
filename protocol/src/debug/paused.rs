//! \[EXTENSION\] What a debugger is told of its target (RULES.md D1030,
//! D1032): an event, a pause with its call stack, each frame's routine,
//! place, parameters, variables and cursors, and a breakpoint.
//!
//! ```text
//! event:      what u8: 0 paused, then a pause; 1 timed out; 2 cancelled;
//!             3 ended
//! pause:      reason u8: 0 a breakpoint, its number u32; 1 a step; then
//!             its frames, their count u32, then each
//! frame:      has origin u8, the origin if 1; visible u8; has place u8,
//!             the character u64 if 1; has line u8, line u32 and column
//!             u32 if 1; parameters, variables (each a named), cursors
//! origin:     kind u8; its name (schema, name); the name shown (schema,
//!             name)
//! named:      name, data type as SQL writes it, value
//! cursor:     name, open u8, query
//! breakpoint: number u32; routine (schema, name); line u32; conditioned
//!             u8, the condition if 1
//! ```

use turborel::bytes::{Put, Reader};
use turborel::catalog::QualifiedName;
use turborel::debugger::{CursorShown, Event, Frame, Kind, Named, Origin, Paused, Reason};
use turborel::error::Result;

use super::{DebugBreakpoint, given, put_given};
use crate::fields::{counted, flag, put_count, put_flag, put_value, value};

const PAUSED: u8 = 0;
const TIMED_OUT: u8 = 1;
const CANCELLED: u8 = 2;
const ENDED: u8 = 3;
const AT_BREAKPOINT: u8 = 0;
const STEPPED: u8 = 1;

/// A routine's kind, as one octet.
pub(super) fn put_kind(b: &mut Vec<u8>, k: Kind) {
    b.put_u8(match k {
        Kind::Procedure => 0,
        Kind::Function => 1,
        Kind::Trigger => 2,
    });
}

/// A routine's kind, as `put_kind` wrote it.
pub(super) fn kind(r: &mut Reader<'_>) -> Result<Kind> {
    match r.u8()? {
        0 => Ok(Kind::Procedure),
        1 => Ok(Kind::Function),
        2 => Ok(Kind::Trigger),
        _ => Err(r.corrupt("not a kind of routine")),
    }
}

/// A qualified name: its schema, then its own.
pub(super) fn put_name(b: &mut Vec<u8>, n: &QualifiedName) {
    b.put_str(&n.auth);
    b.put_str(&n.ident);
}

/// A qualified name, as `put_name` wrote it.
pub(super) fn name(r: &mut Reader<'_>) -> Result<QualifiedName> {
    Ok(QualifiedName { auth: r.str()?, ident: r.str()? })
}

/// A breakpoint.
pub(super) fn put_breakpoint(b: &mut Vec<u8>, p: &DebugBreakpoint) {
    b.put_u32(p.number);
    put_name(b, &p.routine);
    b.put_u32(p.line);
    put_given(b, p.condition.as_deref());
}

/// A breakpoint, as `put_breakpoint` wrote it.
pub(super) fn breakpoint(r: &mut Reader<'_>) -> Result<DebugBreakpoint> {
    Ok(DebugBreakpoint { number: r.u32()?, routine: name(r)?, line: r.u32()?, condition: given(r)? })
}

/// An event.
pub(crate) fn put_event(b: &mut Vec<u8>, e: &Event) {
    match e {
        Event::Paused(p) => {
            b.put_u8(PAUSED);
            put_pause(b, p);
        }
        Event::TimedOut => b.put_u8(TIMED_OUT),
        Event::Cancelled => b.put_u8(CANCELLED),
        Event::Ended => b.put_u8(ENDED),
    }
}

/// An event, as `put_event` wrote it.
pub(crate) fn event(r: &mut Reader<'_>) -> Result<Event> {
    match r.u8()? {
        PAUSED => Ok(Event::Paused(pause(r)?)),
        TIMED_OUT => Ok(Event::TimedOut),
        CANCELLED => Ok(Event::Cancelled),
        ENDED => Ok(Event::Ended),
        _ => Err(r.corrupt("no debugger's event is of this kind")),
    }
}

/// A pause: why, then its frames.
pub(super) fn put_pause(b: &mut Vec<u8>, p: &Paused) {
    match p.reason {
        Reason::Breakpoint(number) => {
            b.put_u8(AT_BREAKPOINT);
            b.put_u32(number);
        }
        Reason::Step => b.put_u8(STEPPED),
    }
    put_count(b, p.frames.len());
    for f in &p.frames {
        put_frame(b, f);
    }
}

/// A pause, as `put_pause` wrote it.
pub(super) fn pause(r: &mut Reader<'_>) -> Result<Paused> {
    let reason = match r.u8()? {
        AT_BREAKPOINT => Reason::Breakpoint(r.u32()?),
        STEPPED => Reason::Step,
        _ => return Err(r.corrupt("no pause is for this reason")),
    };
    Ok(Paused { reason, frames: counted(r, frame)? })
}

/// A frame of the call stack.
fn put_frame(b: &mut Vec<u8>, f: &Frame) {
    put_flag(b, f.origin.is_some());
    if let Some(o) = &f.origin {
        put_kind(b, o.kind);
        put_name(b, &o.name);
        put_name(b, &o.shown);
    }
    put_flag(b, f.visible);
    put_flag(b, f.at.is_some());
    if let Some(at) = f.at {
        b.put_u64(u64::try_from(at).unwrap_or(u64::MAX));
    }
    put_flag(b, f.line.is_some());
    if let Some((line, column)) = f.line {
        b.put_u32(line);
        b.put_u32(column);
    }
    for list in [&f.parameters, &f.variables] {
        put_count(b, list.len());
        for n in list {
            b.put_str(&n.name);
            b.put_str(&n.data_type);
            put_value(b, &n.value);
        }
    }
    put_count(b, f.cursors.len());
    for c in &f.cursors {
        b.put_str(&c.name);
        put_flag(b, c.open);
        b.put_str(&c.query);
    }
}

/// A frame, as `put_frame` wrote it.
fn frame(r: &mut Reader<'_>) -> Result<Frame> {
    let origin = if flag(r)? { Some(Origin { kind: kind(r)?, name: name(r)?, shown: name(r)? }) } else { None };
    let visible = flag(r)?;
    let at = if flag(r)? { Some(usize::try_from(r.u64()?).map_err(|_| r.corrupt("a place past this machine's"))?) } else { None };
    let line = if flag(r)? { Some((r.u32()?, r.u32()?)) } else { None };
    let (parameters, variables) = (counted(r, named)?, counted(r, named)?);
    let cursors = counted(r, |r| Ok(CursorShown { name: r.str()?, open: flag(r)?, query: r.str()? }))?;
    Ok(Frame { origin, visible, at, line, parameters, variables, cursors })
}

/// A parameter or variable, as `put_frame` wrote it.
fn named(r: &mut Reader<'_>) -> Result<Named> {
    Ok(Named { name: r.str()?, data_type: r.str()?, value: value(r)? })
}
