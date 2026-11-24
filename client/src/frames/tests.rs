//! Unit tests of a connection's frames against a scripted server stream:
//! a reply holding a value the client cannot take read past with the rest
//! of its request's, a malformed one losing the connection (RULES.md
//! D393); and Terminate bounded on a stream that bounds itself, a Notice
//! before its Error read past (D394).

use std::cell::Cell;
use std::collections::VecDeque;
use std::io;
use std::rc::Rc;
use std::time::Duration;

use turborel_protocol::{CancelKey, Completed, Diagnostics, Notice};

use super::*;
use crate::connection::{Outcome, Trust};
use crate::timeouts::{Bound, TERMINATE_WITHIN, Timeouts};

/// The kinds of the replies the tests craft byte by byte.
const DESCRIPTION: u8 = b'D';
const ROWS: u8 = b'B';
const COMPLETE: u8 = b'K';
/// The tags of the values and types the tests craft.
const CHARACTER: u8 = 1;
const TRUTH: u8 = 14;
const JSON: u8 = 25;
/// A JSON document's version, and its null and array nodes.
const JSON_VERSION: u8 = 1;
const JSON_NULL: u8 = 0;
const JSON_ARRAY: u8 = 10;
/// One deeper than a JSON document nests.
const TOO_DEEP: usize = 129;
/// The tail tags of a Complete the tests write.
const MORE: u8 = 5;
const CONDITIONS: u8 = 17;
/// A read timeout shorter than the default.
const SHORT: Duration = Duration::from_millis(100);

/// A server's stream, in memory. Each request written is answered by the
/// next bytes of its script, Hello's by the first; past the script it
/// closes, or stalls, a read failing as a socket's would once its bound
/// ran out, or saying it would wait without one.
struct Scripted {
    script: VecDeque<Vec<u8>>,
    readable: VecDeque<u8>,
    written: Vec<u8>,
    closes: bool,
    /// The last bound set on it, shared so a test reads it once the
    /// connection is gone.
    bound: Rc<Cell<Option<Duration>>>,
}

impl Scripted {
    fn new(script: Vec<Vec<u8>>, closes: bool) -> Scripted {
        Scripted { script: script.into(), readable: VecDeque::new(), written: Vec::new(), closes, bound: Rc::default() }
    }
}

impl Read for Scripted {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.readable.is_empty() && !self.closes {
            return Err(match self.bound.get() {
                Some(_) => io::Error::from(io::ErrorKind::TimedOut),
                None => io::Error::other("a read without a bound would wait for ever"),
            });
        }
        let n = out.len().min(self.readable.len());
        for (at, byte) in self.readable.drain(..n).enumerate() {
            out[at] = byte;
        }
        Ok(n)
    }
}

impl Write for Scripted {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(bytes);
        while let Some(end) = frame_end(&self.written) {
            self.written.drain(..end);
            self.readable.extend(self.script.pop_front().unwrap_or_default());
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Where the first frame of `bytes` ends, if it is whole.
fn frame_end(bytes: &[u8]) -> Option<usize> {
    let ([_, length @ ..], _) = bytes.split_first_chunk::<HEADER>()?;
    let end = usize::try_from(u32::from_le_bytes(*length)).ok()?.checked_add(HEADER)?;
    (end <= bytes.len()).then_some(end)
}

impl Bound for Scripted {
    fn bound(&self, within: Option<Duration>) -> io::Result<()> {
        self.bound.set(within);
        Ok(())
    }
    fn gone(&self) -> bool {
        false
    }
}

/// `replies`, encoded one after another.
fn encoded(replies: &[Reply]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for reply in replies {
        reply.encode(&mut bytes);
    }
    bytes
}

/// Ready, granting `capabilities`.
fn ready(capabilities: &[&str]) -> Vec<u8> {
    let capabilities = capabilities.iter().map(|&c| c.to_owned()).collect();
    encoded(&[Reply::Ready { authorization: "X".into(), key: CancelKey([0; 16]), capabilities, server: "turborel".into() }])
}

/// A Complete of `sqlcode` and `sqlstate`.
fn complete(sqlcode: i32, sqlstate: &str) -> Reply {
    Reply::Complete(Completed { sqlcode, sqlstate: sqlstate.into(), ..Completed::default() })
}

/// The Complete of no data, which the statement after a bad one gets.
fn no_data() -> Vec<u8> {
    encoded(&[complete(100, "02000")])
}

/// A frame of `kind` whose body is `body`.
fn frame(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![kind];
    bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(body);
    bytes
}

/// A Rows reply of one row of one value, written as `value`.
fn row_of(value: &[u8]) -> Vec<u8> {
    let mut body = [1_u32.to_le_bytes(), 1_u32.to_le_bytes()].concat();
    body.extend_from_slice(value);
    frame(ROWS, &body)
}

/// A JSON value of the stored form `document`.
fn json(document: &[u8]) -> Vec<u8> {
    let mut value = vec![JSON];
    value.extend_from_slice(&u32::try_from(document.len()).unwrap().to_le_bytes());
    value.extend_from_slice(document);
    value
}

/// A JSON document of arrays nested `depth` deep around a null.
fn nested(depth: usize) -> Vec<u8> {
    let mut node = vec![JSON_NULL];
    for _ in 0..depth {
        let mut array = vec![JSON_ARRAY];
        array.extend_from_slice(&1_u32.to_le_bytes());
        array.extend_from_slice(&u32::try_from(node.len()).unwrap().to_le_bytes());
        array.extend_from_slice(&0_u32.to_le_bytes());
        array.extend_from_slice(&node);
        node = array;
    }
    [vec![JSON_VERSION], node].concat()
}

/// A Description of one CHARACTER column whose units byte is `units`.
fn character_column(units: u8) -> Vec<u8> {
    let mut body = 1_u32.to_le_bytes().to_vec();
    body.push(1);
    body.extend_from_slice(&1_u32.to_le_bytes());
    body.push(b'C');
    body.push(CHARACTER);
    body.extend_from_slice(&1_u32.to_le_bytes());
    body.push(units);
    body.push(0);
    frame(DESCRIPTION, &body)
}

/// A Complete whose one condition is malformed, saying more replies
/// follow if `more`.
fn complete_with_bad_condition(more: bool) -> Vec<u8> {
    let mut body = 0_u32.to_le_bytes().to_vec();
    body.extend_from_slice(&5_u32.to_le_bytes());
    body.extend_from_slice(b"00000");
    body.push(0);
    if more {
        body.push(MORE);
    }
    body.push(CONDITIONS);
    body.extend_from_slice(&1_u32.to_le_bytes());
    body.extend_from_slice(&1_u32.to_le_bytes());
    body.push(0xFF);
    frame(COMPLETE, &body)
}

/// A connection, granted `capabilities`, whose server answers each
/// request after Hello by the next of `script`, then closes.
fn connected(capabilities: &[&str], script: Vec<Vec<u8>>) -> Connection<Scripted> {
    let stream = Scripted::new([vec![ready(capabilities)], script].concat(), true);
    Connection::open(stream, "X", &mut Trust).unwrap()
}

/// Runs a statement answered by `bad`, then one answered by its own
/// Complete of no data. The first fails, but not the connection, and the
/// second gets its own reply.
fn bad_then_own(what: &str, bad: Vec<u8>) {
    let mut c = connected(&[], vec![bad, no_data()]);
    let failure = c.query("SELECT V FROM T").unwrap_err();
    assert!(!failure.ends_connection(), "{what}: {failure:?}");
    assert!(!c.is_lost(), "{what}");
    let Outcome::Done(done) = c.query("DELETE FROM T").unwrap() else { panic!("{what}: rows for a DELETE") };
    assert_eq!((done.sqlcode, done.sqlstate.as_str()), (100_i32, "02000"), "{what}: the next statement's own reply");
}

/// Verifies: [DESIGN] RULES.md D393: a reply whose frame is whole but
/// holds a value the client cannot take, damaged JSON, JSON nested too
/// deep, a truth byte not 0 or 1, a bad units byte or a malformed
/// condition, fails its statement alone, the rest of the statement's
/// replies read past, so the next statement takes its own reply and not
/// a stale Complete; a Complete that says more follow read past too.
/// Regression test (R-TEST-7). The client kept the connection with the
/// statement's replies unread, and the next statement took the bad one's
/// Complete as its own.
#[test]
fn a_bad_value_is_read_past_with_its_statements_replies() {
    let end = encoded(&[complete(0, "00000")]);
    let description = encoded(&[Reply::Description(Vec::new())]);
    let bad_json = json(&[JSON_VERSION, 0xEE]);
    bad_then_own("damaged JSON", [description.clone(), row_of(&bad_json), row_of(&[TRUTH, 1]), end.clone()].concat());
    let deep = json(&nested(TOO_DEEP));
    assert_eq!(nested(TOO_DEEP - 1).len() + 13, nested(TOO_DEEP).len(), "each array 13 octets");
    bad_then_own("JSON too deep", [description.clone(), row_of(&deep), end.clone()].concat());
    bad_then_own("a truth byte of 2", [description, row_of(&[TRUTH, 2]), end.clone()].concat());
    bad_then_own("a units byte of 7", [character_column(7), row_of(&[TRUTH, 1]), end.clone()].concat());
    bad_then_own("a malformed condition", complete_with_bad_condition(false));
    bad_then_own("a condition, more to come", [complete_with_bad_condition(true), end].concat());
}

/// Verifies: [DESIGN] RULES.md D393: a cursor's batch holding a bad value
/// is read past up to its Suspended, and the statement after gets its own
/// reply; a Describe of a cursor, answered by a Description alone, reads
/// nothing past it; a Complete with a bad condition still tells whether a
/// transaction is active.
#[test]
fn a_cursor_or_a_describe_with_a_bad_value_reads_only_its_own() {
    let prepared = encoded(&[Reply::Prepared { statement: 0, parameters: Vec::new(), columns: Vec::new(), called: None }]);
    let batch = [row_of(&[TRUTH, 3]), encoded(&[Reply::Suspended])].concat();
    let script = vec![prepared, batch, character_column(9), no_data()];
    let mut c = connected(&[turborel_protocol::capability::DESCRIBE], script);
    let statement = c.prepare("SELECT V FROM T").unwrap();
    assert!(!c.open_cursor(&statement, &[], 1).unwrap_err().ends_connection());
    assert!(!c.describe_cursor((&statement, 0)).unwrap_err().ends_connection());
    assert!(!c.is_lost());
    let Outcome::Done(done) = c.query("DELETE FROM T").unwrap() else { panic!("rows for a DELETE") };
    assert_eq!(done.sqlcode, 100_i32, "its own reply");
    let mut active = complete_with_bad_condition(false);
    active[HEADER + 13] = 1;
    let mut committing = connected(&[], vec![active]);
    assert!(committing.query("COMMIT").is_err());
    assert!(committing.in_transaction(), "as the Complete's head said");
}

/// Verifies: [DESIGN] RULES.md D393: a malformed frame, a flag neither 0
/// nor 1 or bytes after its last field, loses the connection (SQLSTATE
/// 08006), and the next statement is told so.
#[test]
fn a_malformed_frame_loses_the_connection() {
    let mut flagged = encoded(&[complete(0, "00000")]);
    *flagged.last_mut().unwrap() = 2;
    let mut longer = encoded(&[complete(0, "00000")]);
    longer[1] += 1;
    longer.push(0);
    for (what, bad) in [("a flag of 2", flagged), ("a byte past its end", longer)] {
        let mut c = connected(&[], vec![bad, no_data()]);
        assert_eq!(c.query("DELETE FROM T").unwrap_err().sqlstate, "08006", "{what}");
        assert!(c.is_lost(), "{what}");
        assert_eq!(c.query("DELETE FROM T").unwrap_err().sqlstate, "08006", "{what}: told so after");
    }
}

/// Verifies: [DESIGN] RULES.md D394: Terminate on a stream that bounds
/// itself, given to `open_within`, waits at most its read timeout, else
/// `TERMINATE_WITHIN`, for a server that never answers nor closes, and
/// is told the server did not answer in time. Regression test
/// (R-TEST-7). A program's stream could not be bounded, and Terminate
/// waited on it without limit.
#[test]
fn terminate_waits_on_a_bounded_stream_at_most_its_limit() {
    for (read, limit) in [(None, TERMINATE_WITHIN), (Some(SHORT), SHORT)] {
        let stream = Scripted::new(vec![ready(&[])], false);
        let bound = Rc::clone(&stream.bound);
        let c = Connection::open_within(stream, ("X", &mut Trust), Timeouts { connect: None, read }).unwrap();
        let failure = c.terminate().unwrap_err();
        assert!(failure.message.contains("did not answer in time"), "{failure:?}");
        assert_eq!(bound.get(), Some(limit));
    }
}

/// Verifies: [DESIGN] RULES.md D394: a Notice and a Status before the
/// Error that ends Terminate are read past, and the Error reported; a
/// Notice before the close ends it well. Regression test (R-TEST-7).
/// The bytes after a Notice were taken as its own, and Terminate failed
/// with SQLSTATE 08006 in place of the server's Error.
#[test]
fn terminate_reads_past_a_notice_to_its_error() {
    let notice = Reply::Notice(Notice { sqlstate: "01000".into(), message: "a warning".into(), diagnostics: Diagnostics::default() });
    let status = Reply::Status { name: "READ_ONLY".into(), value: "NO".into() };
    let error = Failure { sqlstate: "40002".into(), ..Failure::from(&turborel_protocol::broken("the commit failed")) };
    let c = connected(&[], vec![encoded(&[notice.clone(), status, Reply::Error(error.clone())])]);
    assert_eq!(c.terminate().unwrap_err(), error);
    connected(&[], vec![encoded(&[notice])]).terminate().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D1032, D1035: an Event that comes among
/// a request's replies is kept, not taken as its reply, and `debug_wait`
/// gives it, then one the server sends later; a wait on a stream that has
/// nothing more gives none within its time, its timeout put back after,
/// and the connection goes on.
#[test]
fn an_event_is_kept_until_waited_for() {
    use turborel::debugger::Event;
    let script = vec![
        ready(&[turborel_protocol::capability::DEBUGGER]),
        encoded(&[Reply::Event(Event::TimedOut), complete(0, "00000"), Reply::Event(Event::Cancelled)]),
    ];
    let stream = Scripted::new(script, false);
    let read = Some(TERMINATE_WITHIN);
    let mut c = Connection::open_within(stream, ("X", &mut Trust), Timeouts { connect: None, read }).unwrap();
    c.debug_interrupt().unwrap();
    assert!(matches!(c.debug_wait(Some(SHORT)).unwrap(), Some(Event::TimedOut)), "kept");
    assert!(matches!(c.debug_wait(Some(SHORT)).unwrap(), Some(Event::Cancelled)), "read");
    assert!(c.debug_wait(Some(SHORT)).unwrap().is_none(), "none came");
    assert_eq!(c.read_timeout(), read, "its timeout put back");
    assert!(!c.is_lost());
}

/// Verifies: [EXTENSION] RULES.md D393, D1032: an Event among the replies
/// read past after a bad value ends no request, so the next statement
/// still takes its own reply, and the Event is kept for a wait.
#[test]
fn an_event_read_past_ends_no_request() {
    use turborel::debugger::Event;
    let description = encoded(&[Reply::Description(Vec::new())]);
    let event = encoded(&[Reply::Event(Event::Ended)]);
    let bad = [description, row_of(&[TRUTH, 2]), event, encoded(&[complete(0, "00000")])].concat();
    let mut c = connected(&[turborel_protocol::capability::DEBUGGER], vec![bad, no_data()]);
    assert!(!c.query("SELECT V FROM T").unwrap_err().ends_connection());
    let Outcome::Done(done) = c.query("DELETE FROM T").unwrap() else { panic!("rows for a DELETE") };
    assert_eq!(done.sqlcode, 100_i32, "its own reply, not the bad statement's");
    assert!(matches!(c.debug_wait(Some(SHORT)).unwrap(), Some(Event::Ended)), "kept");
}
