//! The sample frames of the routine debugger (RULES.md D1030, D1032):
//! each Debug request, each Debugged reply and each Event.

use turborel::catalog::QualifiedName;
use turborel::debugger::{Action, CursorShown, Event, Frame, Kind, Named, Origin, Paused, Reason};
use turborel::value::{Decimal, Value};

use super::{Sample, client, server};
use crate::capability::DEBUGGER;
use crate::{DebugBreakpoint, DebugReply, DebugRequest, Reply, Request};

/// The name ANN.RAISE_PAY.
fn raise_pay() -> QualifiedName {
    QualifiedName { auth: "ANN".into(), ident: "RAISE_PAY".into() }
}

/// The sample of the Debug request `request`.
fn asked(name: &'static str, request: DebugRequest, means: &str) -> Sample {
    client(name, &[DEBUGGER], &Request::Debug(request), means)
}

/// The sample of the reply `reply`.
fn told(name: &'static str, reply: Reply, means: &str) -> Sample {
    server(name, &[DEBUGGER], &reply, means)
}

/// The samples of every Debug request.
fn requests() -> Vec<Sample> {
    let condition = Some("PCT > 5".to_owned());
    vec![
        asked("debug-identify", DebugRequest::Identify, "Debug identify, sub-kind 0"),
        asked("debug-attach", DebugRequest::Attach(2), "Debug attach, sub-kind 1, connection 2"),
        asked("debug-detach", DebugRequest::Detach, "Debug detach, sub-kind 2"),
        asked(
            "debug-break",
            DebugRequest::Break { kind: Kind::Procedure, routine: raise_pay(), line: 4, condition },
            "Debug break, sub-kind 3, procedure 0, ANN.RAISE_PAY, line 4, condition PCT > 5",
        ),
        asked("debug-clear", DebugRequest::Clear(1), "Debug clear, sub-kind 4, breakpoint 1"),
        asked("debug-breakpoints", DebugRequest::Breakpoints, "Debug breakpoints, sub-kind 5"),
        asked("debug-resume", DebugRequest::Resume(Action::Over), "Debug resume, sub-kind 6, over 1"),
        asked("debug-interrupt", DebugRequest::Interrupt, "Debug interrupt, sub-kind 7"),
        asked("debug-stack", DebugRequest::Stack, "Debug stack, sub-kind 8"),
    ]
}

/// A pause at breakpoint 1 in line 4, column 3 of ANN.RAISE_PAY.
fn pause() -> Paused {
    let frame = Frame {
        origin: Some(Origin { kind: Kind::Procedure, name: raise_pay(), shown: raise_pay() }),
        visible: true,
        at: Some(1),
        line: Some((4, 3)),
        parameters: vec![Named { name: "PCT".into(), data_type: "INTEGER".into(), value: Value::Exact(Decimal::new(10, 0)) }],
        variables: vec![Named { name: "N".into(), data_type: "INTEGER".into(), value: Value::Null }],
        cursors: vec![CursorShown { name: "C".into(), open: false, query: "SELECT ID FROM STAFF".into() }],
    };
    Paused { reason: Reason::Breakpoint(1), frames: vec![frame] }
}

/// The samples of every Debugged reply and every Event.
fn replies() -> Vec<Sample> {
    let set = DebugBreakpoint { number: 1, routine: raise_pay(), line: 4, condition: None };
    vec![
        told("debugged-connection", Reply::Debug(DebugReply::Connection(2)), "Debugged connection, sub-kind 0, ID 2"),
        told(
            "debugged-breakpoint",
            Reply::Debug(DebugReply::Breakpoint(set.clone())),
            "Debugged breakpoint, sub-kind 1, number 1, ANN.RAISE_PAY, line 4, no condition",
        ),
        told("debugged-breakpoints", Reply::Debug(DebugReply::Breakpoints(vec![set])), "Debugged breakpoints, sub-kind 2, one of them"),
        told("debugged-running", Reply::Debug(DebugReply::Stack(None)), "Debugged stack, sub-kind 3, none: running"),
        told(
            "debugged-stack",
            Reply::Debug(DebugReply::Stack(Some(pause()))),
            "Debugged stack, sub-kind 3, a pause at breakpoint 1, one frame of ANN.RAISE_PAY at line 4, column 3, PCT 10, N null, cursor C closed",
        ),
        told("event-paused", Reply::Event(Event::Paused(pause())), "Event paused, sub-kind 0, the pause of debugged-stack"),
        told("event-timed-out", Reply::Event(Event::TimedOut), "Event timed out, sub-kind 1"),
        told("event-cancelled", Reply::Event(Event::Cancelled), "Event cancelled, sub-kind 2"),
        told("event-ended", Reply::Event(Event::Ended), "Event ended, sub-kind 3"),
    ]
}

/// Every sample of the debugger's.
pub(super) fn samples() -> Vec<Sample> {
    let mut all = requests();
    all.extend(replies());
    all
}
