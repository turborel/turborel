//! The routine debugger over the native protocol (RULES.md D1030 to
//! D1035): its requests refused unless `debugger` was granted, malformed
//! ones failing the connection, a connection's ID told, and a debugger
//! attached only to a session it may debug, refused otherwise as of no
//! such connection.

use std::net::TcpStream;
use std::thread::JoinHandle;
use std::time::Duration;

use turborel::catalog::QualifiedName;
use turborel::db::Database;
use turborel_client::connection::{Connection, Trust};
use turborel_client::debug::{Action, Event, Kind, Paused, Reason};
use turborel_client::told::Offer;
use turborel_protocol::{DebugRequest, Request};

use crate::common::{Running, rows, set, start, state};

/// A database whose owner is ANN, beside the user BOB.
fn database() -> Database {
    let db = Database::new();
    db.create_user("ANN", b"x").unwrap();
    db.create_user("BOB", b"x").unwrap();
    db
}

/// A connection to `server` as `user`, asking what `offer` asks.
fn offering(server: &Running, user: &str, offer: Offer) -> Connection<TcpStream> {
    let stream = TcpStream::connect(server.address).unwrap();
    Connection::open_offering(stream, (user, &mut Trust), offer).unwrap()
}

/// The ID CONNECTIONS shows of the one connection of `user` that ANN,
/// the owner, sees, over `ann`.
fn listed(ann: &mut Connection<TcpStream>, user: &str) -> u64 {
    let sql = format!("SELECT ID FROM INFORMATION_SCHEMA.CONNECTIONS WHERE USER_NAME = '{user}'");
    let ids = rows(&ann.query(&sql).unwrap());
    ann.query("COMMIT").unwrap();
    assert_eq!(ids.len(), 1, "{ids:?}");
    ids.first().unwrap().trim().parse().unwrap()
}

/// Verifies: [EXTENSION] RULES.md D1030, D185: a client that does not ask
/// for `debugger` is not granted it, and its own client refuses to send a
/// Debug request (SQLSTATE 08006); a Debug request sent all the same, or
/// malformed, fails the connection; the first form serves as ever.
#[test]
fn a_debug_request_needs_the_capability_granted() {
    let db = database();
    let server = start(&db, &[]);
    let mut none = offering(&server, "BOB", Offer::none());
    assert!(!none.granted().has(turborel_protocol::capability::DEBUGGER));
    assert_eq!(state(&none.debug_id().unwrap_err()).0, "08006", "not granted, not sent");
    assert_eq!(rows(&none.query("VALUES 1").unwrap()), ["1"]);
    let mut raw = server.raw();
    raw.hello("BOB");
    raw.send(&Request::Debug(DebugRequest::Identify));
    assert_eq!(raw.refusal().sqlstate, "08006");
    let mut malformed = server.raw();
    malformed.send(&Request::Hello {
        version: turborel_protocol::VERSION,
        user: "BOB".into(),
        mechanism: "trust".into(),
        initial: vec![],
        capabilities: vec![turborel_protocol::capability::DEBUGGER.into()],
        attributes: vec![],
    });
    assert!(matches!(malformed.receive(), Some(turborel_protocol::Reply::Ready { .. })));
    malformed.send_bytes(&[b'V', 1, 0, 0, 0, 9]);
    assert_eq!(malformed.refusal().sqlstate, "08006");
}

/// Verifies: [EXTENSION] RULES.md D1034: Identify tells a connection its
/// own ID, the one CONNECTIONS shows of it.
#[test]
fn a_connection_is_told_its_id() {
    let db = database();
    let server = start(&db, &[]);
    let mut ann = server.connect("ANN");
    let mut bob = server.connect("BOB");
    assert_eq!(bob.debug_id().unwrap(), listed(&mut ann, "BOB"));
    assert_ne!(ann.debug_id().unwrap(), bob.debug_id().unwrap());
}

/// Verifies: [EXTENSION] RULES.md D1031: a debugger attaches to no
/// connection that is not served, nor to another user's, each refused
/// alike (SQLSTATE 08003), the connection going on; the owner attaches to
/// any user's, and a user to its own other connection; a second Attach,
/// an Attach to its own session, and a call of a connection attached to
/// none are refused (TBD01).
#[test]
fn a_debugger_attaches_only_where_it_may() {
    let db = database();
    let server = start(&db, &[]);
    let mut ann = server.connect("ANN");
    let mut bob = server.connect("BOB");
    let mut bob_too = server.connect("BOB");
    let (ann_id, bob_id) = (ann.debug_id().unwrap(), bob.debug_id().unwrap());
    let refused = |e: &turborel_client::Failure| state(e).0.to_owned();
    assert_eq!(refused(&bob.debug_attach(u64::MAX).unwrap_err()), "08003");
    assert_eq!(refused(&bob_too.debug_attach(ann_id).unwrap_err()), "08003", "another user's, as of none");
    assert!(!bob_too.is_lost(), "the connection goes on");
    assert_eq!(refused(&bob.debug_stack().unwrap_err()), "TBD01", "attached to none");
    assert_eq!(refused(&bob.debug_detach().unwrap_err()), "TBD01");
    assert_eq!(refused(&bob.debug_attach(bob_id).unwrap_err()), "TBD01", "its own session");
    bob_too.debug_attach(bob_id).unwrap();
    assert_eq!(refused(&bob_too.debug_attach(bob_id).unwrap_err()), "TBD01", "attached already");
    assert!(bob_too.debug_stack().unwrap().is_none(), "the target runs");
    bob_too.debug_detach().unwrap();
    ann.debug_attach(bob_id).unwrap();
    assert_eq!(ann.debug_breakpoints().unwrap().len(), 0);
    ann.debug_detach().unwrap();
}

/// How long a test waits for an event that comes.
const COMES: Duration = Duration::from_secs(20);
/// How long a test waits for an event that does not come.
const NONE_COMES: Duration = Duration::from_millis(300);

/// `database`, with ANN's schema holding T (K) and the procedure P, its
/// lines numbered from its CREATE's: line 4 sets Y, line 5 inserts it.
fn defined() -> Database {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE T (K INTEGER)").unwrap();
    let ann = db.session("ANN");
    ann.run(
        "CREATE PROCEDURE P (IN X INTEGER) MODIFIES SQL DATA\n\
         BEGIN\n\
         \x20 DECLARE Y INTEGER DEFAULT 0;\n\
         \x20 SET Y = X * 2;\n\
         \x20 INSERT INTO T VALUES (Y);\n\
         END",
    )
    .unwrap();
    ann.commit().unwrap();
    db
}

/// `connection` running `sql` on a thread of its own, a statement under
/// autocommit: its outcome's SQLCODE, once it ends.
fn running(mut connection: Connection<TcpStream>, sql: &'static str) -> JoinHandle<(Connection<TcpStream>, i32)> {
    std::thread::spawn(move || {
        connection.set_autocommit(true);
        let code = connection.query(sql).map_or_else(|f| f.sqlcode, |_| 0);
        (connection, code)
    })
}

/// The next pause `debugger` hears of.
fn paused(debugger: &mut Connection<TcpStream>) -> Paused {
    match debugger.debug_wait(Some(COMES)).unwrap() {
        Some(Event::Paused(p)) => p,
        other => panic!("not a pause: {other:?}"),
    }
}

/// The value of `name` in the frame paused in, as text.
fn value(p: &Paused, name: &str) -> String {
    let f = p.frames.last().unwrap();
    f.parameters.iter().chain(&f.variables).find(|n| n.name == name).map(|n| n.value.to_string()).unwrap()
}

/// Verifies: [EXTENSION] RULES.md D1030, D1032, D1033: over two
/// connections, a debugger sets a breakpoint by line, hears its target
/// pause there unasked, with the frame's place and values, reads the
/// stack again, steps over to the next line, and continues, the target's
/// CALL then completing; a breakpoint whose condition is false pauses
/// nothing.
#[test]
fn a_debugger_drives_another_connections_procedure() {
    let db = defined();
    let server = start(&db, &[]);
    let (mut debugger, mut target) = (server.connect("ANN"), server.connect("ANN"));
    debugger.debug_attach(target.debug_id().unwrap()).unwrap();
    let p = QualifiedName { auth: String::new(), ident: "P".into() };
    let set = debugger.debug_break((Kind::Procedure, &p), 4, None).unwrap();
    assert_eq!((set.number, set.routine.auth.as_str(), set.line), (1, "ANN", 4));
    let call = running(target, "CALL P (3)");
    let at = paused(&mut debugger);
    assert_eq!(at.reason, Reason::Breakpoint(1));
    assert_eq!(at.frames.last().unwrap().line.map(|(line, _)| line), Some(4));
    assert_eq!((value(&at, "X"), value(&at, "Y")), ("3".to_owned(), "0".to_owned()));
    let again = debugger.debug_stack().unwrap().unwrap();
    assert_eq!(again.frames.len(), at.frames.len());
    debugger.debug_resume(Action::Over).unwrap();
    let stepped = paused(&mut debugger);
    assert_eq!((stepped.reason, stepped.frames.last().unwrap().line.map(|(l, _)| l)), (Reason::Step, Some(5)));
    assert_eq!(value(&stepped, "Y"), "6");
    debugger.debug_resume(Action::Continue).unwrap();
    let code;
    (target, code) = call.join().unwrap();
    assert_eq!(code, 0);
    debugger.debug_clear(1).unwrap();
    debugger.debug_break((Kind::Procedure, &p), 4, Some("X > 5")).unwrap();
    let call = running(target, "CALL P (4)");
    assert!(debugger.debug_wait(Some(NONE_COMES)).unwrap().is_none(), "its condition is false");
    assert_eq!(call.join().unwrap().1, 0);
    assert_eq!(debugger.debug_breakpoints().unwrap().len(), 1);
    debugger.debug_detach().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D1011, D1030: a target paused past
/// DEBUGGER.PAUSE_TIMEOUT_MS goes on by itself and its debugger hears it
/// timed out; a target whose connection ends is heard as Ended, and its
/// debugger is then attached to none (TBD01).
#[test]
fn a_debugger_hears_a_timeout_and_its_targets_end() {
    let db = defined();
    set(&db, "DEBUGGER.PAUSE_TIMEOUT_MS", 50);
    let server = start(&db, &[]);
    let (mut debugger, mut target) = (server.connect("ANN"), server.connect("ANN"));
    debugger.debug_attach(target.debug_id().unwrap()).unwrap();
    let p = QualifiedName { auth: "ANN".into(), ident: "P".into() };
    debugger.debug_break((Kind::Procedure, &p), 5, None).unwrap();
    let call = running(target, "CALL P (1)");
    assert_eq!(paused(&mut debugger).reason, Reason::Breakpoint(1));
    assert!(matches!(debugger.debug_wait(Some(COMES)).unwrap(), Some(Event::TimedOut)));
    let (ended, code) = call.join().unwrap();
    assert_eq!(code, 0, "it went on by itself");
    target = ended;
    target.terminate().unwrap();
    assert!(matches!(debugger.debug_wait(Some(COMES)).unwrap(), Some(Event::Ended)));
    assert_eq!(state(&debugger.debug_stack().unwrap_err()).0, "TBD01", "detached");
}

/// Verifies: [EXTENSION] RULES.md D1008, D1031: a user attached to its own
/// other connection may set no breakpoint in a routine it neither owns nor
/// holds DEBUG on (SQLCODE -104); granted DEBUG, it may.
#[test]
fn a_breakpoint_needs_debug_on_its_routine() {
    let db = defined();
    let server = start(&db, &[]);
    let (mut bob, mut bob_too) = (server.connect("BOB"), server.connect("BOB"));
    bob.debug_attach(bob_too.debug_id().unwrap()).unwrap();
    let p = QualifiedName { auth: "ANN".into(), ident: "P".into() };
    assert_eq!(bob.debug_break((Kind::Procedure, &p), 4, None).unwrap_err().sqlcode, -104);
    let mut ann = server.connect("ANN");
    ann.set_autocommit(true);
    ann.query("GRANT DEBUG ON PROCEDURE P TO BOB").unwrap();
    assert_eq!(bob.debug_break((Kind::Procedure, &p), 4, None).unwrap().number, 1);
    assert!(bob_too.debug_id().is_ok());
}
