//! \[EXTENSION\] The routine debugger (RULES.md D1003 to D1012): one
//! session attached to another stops its procedures, functions and
//! triggered actions at breakpoints, steps through them, reads their call
//! stack and their frames' values, and lets the target go on as it
//! detaches, its connection drops, the target ends, or a pause times out.

use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use turborel::catalog::QualifiedName;
use turborel::db::{Database, DebugTarget, Debugger, Session};
use turborel::debugger::{Action, Event, Frame, Kind, Paused, Reason};
use turborel::error::{Code, SqlState};
use turborel::value::{Decimal, Value};

/// How long a test waits for an event that comes.
const COMES: Duration = Duration::from_secs(20);

/// Schema A, owned by A: T (K), procedures P and Q, function F, P's lines
/// numbered from its CREATE's.
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let a = db.session("A");
    a.run(
        "CREATE PROCEDURE Q (IN Z INTEGER) MODIFIES SQL DATA\n\
         BEGIN\n\
         \x20 INSERT INTO T VALUES (Z + 1);\n\
         \x20 INSERT INTO T VALUES (Z + 2);\n\
         END",
    )
    .unwrap();
    a.run(
        "CREATE FUNCTION F (V INTEGER) RETURNS INTEGER CONTAINS SQL\n\
         BEGIN ATOMIC\n\
         \x20 DECLARE W INTEGER;\n\
         \x20 SET W = V + 100;\n\
         \x20 RETURN W;\n\
         END",
    )
    .unwrap();
    a.run(
        "CREATE PROCEDURE P (IN X INTEGER) MODIFIES SQL DATA\n\
         BEGIN\n\
         \x20 DECLARE Y INTEGER DEFAULT 0;\n\
         \x20 DECLARE C CURSOR FOR SELECT K FROM T;\n\
         \x20 SET Y = X * 2;\n\
         \x20 CALL Q (Y);\n\
         \x20 OPEN C;\n\
         \x20 INSERT INTO T VALUES (F (Y));\n\
         \x20 CLOSE C;\n\
         END",
    )
    .unwrap();
    a.commit().unwrap();
    db
}

/// The name `ident` of schema A.
fn named(ident: &str) -> QualifiedName {
    QualifiedName { auth: "A".to_owned(), ident: ident.to_owned() }
}

/// A target session of `user` on a thread of its own, its handle, and the
/// word that has it run `sql` and commit: it returns the SQLCODE of the
/// first that failed, if one did.
fn target(db: &Database, user: &str, sql: &'static str) -> (DebugTarget, Sender<()>, JoinHandle<Option<i32>>) {
    let (db, user) = (db.clone(), user.to_owned());
    let (handle, held) = channel();
    let (go, word) = channel::<()>();
    let thread = std::thread::spawn(move || {
        let s = db.session(&user);
        handle.send(s.debug_target()).unwrap();
        word.recv().unwrap();
        s.run(sql).and_then(|_| s.commit()).err().map(|e| e.sqlcode())
    });
    (held.recv().unwrap(), go, thread)
}

/// The next pause `debugger` hears of.
fn paused(debugger: &Debugger, s: &Session) -> Paused {
    match debugger.wait(s, COMES).unwrap() {
        Some(Event::Paused(p)) => p,
        other => panic!("not a pause: {other:?}"),
    }
}

/// The frame paused in.
fn top(p: &Paused) -> &Frame {
    p.frames.last().unwrap()
}

/// The value of `name` among the frame's parameters and variables.
fn value(f: &Frame, name: &str) -> String {
    f.parameters.iter().chain(&f.variables).find(|n| n.name == name).map(|n| n.value.to_string().trim().to_owned()).unwrap()
}

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The keys of T, in order.
fn keys(s: &Session) -> Vec<String> {
    let mut c = s.open_cursor("SELECT K FROM T ORDER BY K").unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(ToString::to_string).collect::<String>());
    }
    out
}

/// Verifies: [EXTENSION] RULES.md D1003, D1007: a breakpoint by line
/// pauses the target there, its frame showing the routine, the place, the
/// parameters, the variables and the cursors with their values; continued,
/// it runs to its end as if never paused.
#[test]
fn a_breakpoint_pauses_and_shows_the_frame() {
    let db = defined();
    let a = db.session("A");
    let (t, go, thread) = target(&db, "A", "CALL P (5)");
    let debugger = a.attach(&t).unwrap();
    let point = debugger.break_at(&a, (Kind::Procedure, &named("P")), 8, None).unwrap();
    assert_eq!((point.number, point.line), (1, 8));
    go.send(()).unwrap();
    let p = paused(&debugger, &a);
    assert_eq!(p.reason, Reason::Breakpoint(1));
    assert_eq!(p.frames.len(), 1, "Q has returned");
    let f = top(&p);
    assert_eq!(f.origin.as_ref().map(|o| (o.kind, o.shown.ident.as_str())), Some((Kind::Procedure, "P")));
    assert_eq!(f.line, Some((8, 3)), "INSERT is the line's statement, at its third character");
    assert_eq!((value(f, "X"), value(f, "Y")), ("5".to_owned(), "10".to_owned()));
    assert_eq!(f.cursors.iter().map(|c| (c.name.as_str(), c.open)).collect::<Vec<_>>(), [("C", true)]);
    assert!(debugger.paused().unwrap().is_some());
    debugger.resume(Action::Continue).unwrap();
    assert_eq!(thread.join().unwrap(), None);
    assert_eq!(keys(&a), ["11", "12", "110"]);
}

/// Verifies: [EXTENSION] RULES.md D1007: stepping into a CALL pauses in
/// the procedure called, its frame atop its caller's; out of it, at the
/// caller's next statement; over a statement that invokes a function, at
/// the next of the same frame; into it, in the function's own frame.
#[test]
fn steps_go_into_over_and_out_of_calls() {
    let db = defined();
    let a = db.session("A");
    let (t, go, thread) = target(&db, "A", "CALL P (5)");
    let debugger = a.attach(&t).unwrap();
    debugger.break_at(&a, (Kind::Procedure, &named("P")), 6, None).unwrap();
    go.send(()).unwrap();
    assert_eq!(top(&paused(&debugger, &a)).line.map(|l| l.0), Some(6));
    debugger.resume(Action::Into).unwrap();
    let p = paused(&debugger, &a);
    let names: Vec<_> = p.frames.iter().map(|f| f.origin.as_ref().map(|o| o.shown.ident.clone()).unwrap_or_default()).collect();
    assert_eq!(names, ["P", "Q"]);
    assert_eq!((p.reason, top(&p).line.map(|l| l.0), value(top(&p), "Z")), (Reason::Step, Some(3), "10".to_owned()));
    debugger.resume(Action::Out).unwrap();
    let p = paused(&debugger, &a);
    assert_eq!((p.frames.len(), top(&p).line.map(|l| l.0)), (1, Some(7)), "OPEN C, next in P");
    debugger.resume(Action::Over).unwrap();
    assert_eq!(top(&paused(&debugger, &a)).line.map(|l| l.0), Some(8));
    debugger.resume(Action::Into).unwrap();
    let p = paused(&debugger, &a);
    let f = top(&p);
    assert_eq!((p.frames.len(), f.origin.as_ref().map(|o| o.kind)), (2, Some(Kind::Function)));
    assert_eq!((f.line.map(|l| l.0), value(f, "V")), (Some(4), "10".to_owned()));
    debugger.resume(Action::Continue).unwrap();
    assert_eq!(thread.join().unwrap(), None);
    assert_eq!(keys(&a), ["11", "12", "110"]);
}

/// Verifies: [EXTENSION] RULES.md D1007: stepping over a CALL pauses at
/// the caller's next statement, the procedure called run whole.
#[test]
fn a_step_over_a_call_runs_it_whole() {
    let db = defined();
    let a = db.session("A");
    let (t, go, thread) = target(&db, "A", "CALL P (5)");
    let debugger = a.attach(&t).unwrap();
    debugger.break_at(&a, (Kind::Procedure, &named("P")), 6, None).unwrap();
    go.send(()).unwrap();
    paused(&debugger, &a);
    debugger.resume(Action::Over).unwrap();
    let p = paused(&debugger, &a);
    assert_eq!((p.frames.len(), top(&p).line.map(|l| l.0)), (1, Some(7)), "OPEN C, Q run whole");
    debugger.resume(Action::Continue).unwrap();
    assert_eq!(thread.join().unwrap(), None);
}

/// Verifies: [EXTENSION] RULES.md D1006: a prepared CALL, which runs its
/// procedure compiled, runs it interpreted while a debugger is attached,
/// so a breakpoint pauses it; detached, it runs compiled again.
#[test]
fn a_compiled_call_pauses_interpreted() {
    let db = defined();
    let a = db.session("A");
    let (handle, held) = channel();
    let (go, word) = channel::<()>();
    let other = db.clone();
    let thread = std::thread::spawn(move || {
        let s = other.session("A");
        let call = s.prepare("CALL Q (?)").unwrap();
        s.execute_prepared(&call, &[int(1)]).unwrap();
        handle.send(s.debug_target()).unwrap();
        word.recv().unwrap();
        s.execute_prepared(&call, &[int(10)]).unwrap();
        word.recv().unwrap();
        s.execute_prepared(&call, &[int(20)]).unwrap();
        s.commit().unwrap();
    });
    let debugger = a.attach(&held.recv().unwrap()).unwrap();
    debugger.break_at(&a, (Kind::Procedure, &named("Q")), 4, None).unwrap();
    go.send(()).unwrap();
    let p = paused(&debugger, &a);
    assert_eq!((top(&p).line.map(|l| l.0), value(top(&p), "Z")), (Some(4), "10".to_owned()));
    debugger.detach();
    go.send(()).unwrap();
    thread.join().unwrap();
    assert_eq!(keys(&a), ["2", "3", "11", "12", "21", "22"]);
}

/// Verifies: [EXTENSION] RULES.md D1009: a conditional breakpoint pauses
/// only where its condition is true of the frame.
#[test]
fn a_conditional_breakpoint_pauses_where_it_holds() {
    let db = defined();
    let a = db.session("A");
    let (t, go, thread) = target(&db, "A", "BEGIN CALL Q (1); CALL Q (7); END");
    let debugger = a.attach(&t).unwrap();
    debugger.break_at(&a, (Kind::Procedure, &named("Q")), 3, Some("Z > 5")).unwrap();
    go.send(()).unwrap();
    let p = paused(&debugger, &a);
    assert_eq!(value(top(&p), "Z"), "7".to_owned(), "Z = 1 passed by");
    debugger.resume(Action::Continue).unwrap();
    assert_eq!(thread.join().unwrap(), None);
}

/// Verifies: [EXTENSION] RULES.md D1011: a pause past
/// DEBUGGER.PAUSE_TIMEOUT_MS goes on by itself, and the debugger is told.
#[test]
fn a_pause_times_out() {
    let db = defined();
    db.set_setting("DEBUGGER.PAUSE_TIMEOUT_MS", &int(50)).unwrap();
    let a = db.session("A");
    let (t, go, thread) = target(&db, "A", "CALL Q (1)");
    let debugger = a.attach(&t).unwrap();
    debugger.break_at(&a, (Kind::Procedure, &named("Q")), 3, None).unwrap();
    go.send(()).unwrap();
    paused(&debugger, &a);
    assert!(matches!(debugger.wait(&a, COMES).unwrap(), Some(Event::TimedOut)));
    assert_eq!(thread.join().unwrap(), None);
    assert_eq!(keys(&a), ["2", "3"]);
    assert_eq!(debugger.resume(Action::Continue).unwrap_err().sqlstate(), SqlState::DEBUGGER, "no longer paused");
}

/// Verifies: [EXTENSION] RULES.md D1010: a debugger detached, or dropped
/// as its connection is, lets its paused target go on, as if never
/// debugged; the target's next debugger may attach.
#[test]
fn detaching_or_dropping_lets_the_target_go_on() {
    for dropped in [false, true] {
        let db = defined();
        let a = db.session("A");
        let (t, go, thread) = target(&db, "A", "BEGIN CALL Q (1); CALL Q (7); END");
        let debugger = a.attach(&t).unwrap();
        debugger.break_at(&a, (Kind::Procedure, &named("Q")), 3, None).unwrap();
        go.send(()).unwrap();
        paused(&debugger, &a);
        if dropped {
            drop(debugger);
        } else {
            debugger.detach();
        }
        let next = db.session("A").attach(&t);
        assert!(next.is_ok(), "detached, dropped: {dropped}");
        assert_eq!(thread.join().unwrap(), None, "dropped: {dropped}");
        assert_eq!(keys(&a), ["2", "3", "8", "9"]);
    }
}

/// Verifies: [EXTENSION] RULES.md D1010: the target session ending tells
/// its debugger, which waits no more.
#[test]
fn the_target_ending_ends_the_debugger() {
    let db = defined();
    let a = db.session("A");
    let (t, go, thread) = target(&db, "A", "CALL Q (1)");
    let debugger = a.attach(&t).unwrap();
    go.send(()).unwrap();
    assert_eq!(thread.join().unwrap(), None);
    assert!(matches!(debugger.wait(&a, COMES).unwrap(), Some(Event::Ended)));
    assert!(debugger.ended().unwrap());
    assert!(debugger.wait(&a, COMES).unwrap().is_none(), "nothing more comes");
}

/// Verifies: [EXTENSION] RULES.md D1010: a paused statement cancelled
/// fails as cancelled (SQLCODE -920), and the debugger is told.
#[test]
fn a_paused_statement_is_cancelled() {
    let db = defined();
    let a = db.session("A");
    let (t, go, thread) = target(&db, "A", "CALL Q (1)");
    let debugger = a.attach(&t).unwrap();
    debugger.break_at(&a, (Kind::Procedure, &named("Q")), 3, None).unwrap();
    go.send(()).unwrap();
    paused(&debugger, &a);
    t.cancel();
    assert!(matches!(debugger.wait(&a, COMES).unwrap(), Some(Event::Cancelled)));
    assert_eq!(thread.join().unwrap(), Some(-920));
}

/// Verifies: [EXTENSION] RULES.md D1008: a debugger whose user holds no
/// DEBUG on a routine sets no breakpoint in it, and steps over its frames,
/// which show its name alone; DEBUG granted, it may.
#[test]
fn a_debugger_sees_into_what_it_may_debug() {
    let db = defined();
    let a = db.session("A");
    a.run("GRANT EXECUTE ON PROCEDURE P TO B").unwrap();
    a.run("GRANT DEBUG ON PROCEDURE P TO B").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    let (t, go, thread) = target(&db, "B", "CALL A.P (5)");
    let debugger = b.attach(&t).unwrap();
    let refusal = debugger.break_at(&b, (Kind::Procedure, &named("Q")), 3, None).unwrap_err();
    assert_eq!(refusal.code, Code::Privilege);
    debugger.break_at(&b, (Kind::Procedure, &named("P")), 6, None).unwrap();
    go.send(()).unwrap();
    paused(&debugger, &b);
    debugger.resume(Action::Into).unwrap();
    let p = paused(&debugger, &b);
    assert_eq!((p.frames.len(), top(&p).line.map(|l| l.0)), (1, Some(7)), "Q is stepped over");
    debugger.resume(Action::Continue).unwrap();
    assert_eq!(thread.join().unwrap(), None);
}

/// Verifies: [EXTENSION] RULES.md D1004, D1008: a triggered action pauses
/// at a breakpoint of its trigger set by the owner of its schema, its frame
/// showing its transition variables' columns and its variables.
#[test]
fn a_triggered_action_pauses() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TABLE U (K INTEGER)").unwrap();
    a.run(
        "CREATE TRIGGER TR AFTER INSERT ON T REFERENCING NEW ROW AS N FOR EACH ROW\n\
         BEGIN ATOMIC\n\
         \x20 DECLARE V INTEGER;\n\
         \x20 SET V = N.K * 3;\n\
         \x20 INSERT INTO U VALUES (V);\n\
         END",
    )
    .unwrap();
    a.commit().unwrap();
    let (t, go, thread) = target(&db, "A", "INSERT INTO T VALUES (4)");
    let debugger = a.attach(&t).unwrap();
    debugger.break_at(&a, (Kind::Trigger, &named("TR")), 5, None).unwrap();
    go.send(()).unwrap();
    let p = paused(&debugger, &a);
    let f = top(&p);
    assert_eq!(f.origin.as_ref().map(|o| (o.kind, o.name.ident.as_str())), Some((Kind::Trigger, "TR")));
    assert_eq!((f.line.map(|l| l.0), value(f, "N.K"), value(f, "V")), (Some(5), "4".to_owned(), "12".to_owned()));
    debugger.resume(Action::Continue).unwrap();
    assert_eq!(thread.join().unwrap(), None);
}

/// Verifies: [EXTENSION] RULES.md D1014: each statement of a plain
/// triggered action, of no declarations, pauses: stepped into or over from
/// the first, at the second, each invocation's transition values shown.
#[test]
fn a_plain_triggered_actions_statements_pause() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TABLE S (K INTEGER)").unwrap();
    a.run("CREATE TABLE U (K INTEGER)").unwrap();
    a.run(
        "CREATE TRIGGER TP AFTER INSERT ON S REFERENCING NEW ROW AS N FOR EACH ROW\n\
         BEGIN ATOMIC\n\
         \x20 INSERT INTO U VALUES (N.K);\n\
         \x20 INSERT INTO U VALUES (N.K * 10);\n\
         END",
    )
    .unwrap();
    a.commit().unwrap();
    let (t, go, thread) = target(&db, "A", "INSERT INTO S VALUES (1), (2)");
    let debugger = a.attach(&t).unwrap();
    debugger.break_at(&a, (Kind::Trigger, &named("TP")), 3, None).unwrap();
    go.send(()).unwrap();
    for (k, action) in [("1", Action::Into), ("2", Action::Over)] {
        let p = paused(&debugger, &a);
        assert_eq!((p.reason, top(&p).line.map(|l| l.0), value(top(&p), "N.K")), (Reason::Breakpoint(1), Some(3), k.to_owned()));
        debugger.resume(action).unwrap();
        let p = paused(&debugger, &a);
        let o = top(&p).origin.as_ref().map(|o| (o.kind, o.name.ident.clone()));
        assert_eq!((p.reason, p.frames.len(), o), (Reason::Step, 1, Some((Kind::Trigger, "TP".to_owned()))), "{action:?}");
        assert_eq!(top(&p).line.map(|l| l.0), Some(4), "{action:?}");
        debugger.resume(Action::Continue).unwrap();
    }
    assert_eq!(thread.join().unwrap(), None);
    let mut c = a.open_cursor("SELECT K FROM U ORDER BY K").unwrap();
    let mut keys = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        keys.push(row.iter().map(ToString::to_string).collect::<String>());
    }
    assert_eq!(keys, ["1", "2", "10", "20"]);
}

/// Verifies: [EXTENSION] RULES.md D1013: each request its target's state
/// does not allow is refused, SQLSTATE TBD01, and a breakpoint in no
/// routine or at no line, SQLCODE -103.
#[test]
fn a_debuggers_refusals() {
    let db = defined();
    let a = db.session("A");
    assert_eq!(a.attach(&a.debug_target()).unwrap_err().sqlstate(), SqlState::DEBUGGER, "itself");
    let other = db.session("A");
    let debugger = a.attach(&other.debug_target()).unwrap();
    let again = db.session("A").attach(&other.debug_target()).unwrap_err();
    assert_eq!(again.sqlstate(), SqlState::DEBUGGER, "attached already");
    assert_eq!(debugger.resume(Action::Into).unwrap_err().sqlstate(), SqlState::DEBUGGER, "not paused");
    assert_eq!(debugger.clear(1).unwrap_err().sqlstate(), SqlState::DEBUGGER, "no breakpoint 1");
    let none = debugger.break_at(&a, (Kind::Procedure, &named("NONE")), 1, None).unwrap_err();
    assert_eq!(none.code, Code::SyntaxRule, "no routine");
    let function = debugger.break_at(&a, (Kind::Procedure, &named("F")), 1, None).unwrap_err();
    assert_eq!(function.code, Code::SyntaxRule, "F is no procedure");
    let line = debugger.break_at(&a, (Kind::Procedure, &named("Q")), 99, None).unwrap_err();
    assert_eq!(line.code, Code::SyntaxRule, "no line 99");
    let b = debugger.break_at(&a, (Kind::Function, &named("F")), 4, None).unwrap();
    debugger.clear(b.number).unwrap();
    assert_eq!(debugger.breakpoints().unwrap(), []);
    debugger.detach();
    db.session("A").attach(&other.debug_target()).unwrap();
}
