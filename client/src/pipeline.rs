//! \[DESIGN\] Pipelining (RULES.md D45, D200). A `Pipeline` borrowed from
//! a connection queues requests, writes them at once, and reads one
//! outcome for each, in order, as the server answers each request alone
//! and in turn. A batch costs one round trip, not one a request.
//!
//! The requests are written a window at a time, each window at most
//! `PIPELINE_WINDOW` bytes, and its replies read before the next is
//! written. A window fits the buffers of the two sockets between client
//! and server, so it is written whole even while the server writes the
//! replies of its first requests and reads no more. A client that wrote
//! more before reading could wait on a server waiting on it.
//!
//! Each request is independent, so by default a failure is that
//! request's outcome alone and the rest run. `OnError::Skip` makes the
//! batch stop at its first failure, the requests before it kept. A
//! savepoint is set before each request but the first, within the
//! transaction, and the requests after a failed one are undone to the
//! savepoint set after it, or never sent if their window was not yet
//! written; each is `Piped::Skipped`. Its requests are of one
//! transaction, which under autocommit, of a batch that began none, is
//! committed as the batch ends. The protocol has no request that skips
//! the requests after a failure: a Query of several statements stops at
//! its first failure (RULES.md D192), but only among its own statements,
//! not among prepared executions. So the savepoints stay (`guarded`).

use std::io::{Read, Write};

use turborel::value::Value;
use turborel_protocol::{Attributes, Failure, Request};

use crate::connection::{Connection, Outcome, Result};
use crate::statements::{Statement, execute_request};

/// \[DESIGN\] The most bytes of requests written before their replies are
/// read: well within the smallest socket buffer a platform gives, so a
/// window is written whole whatever the server is doing.
pub const PIPELINE_WINDOW: usize = 64 * 1024;

/// \[DESIGN\] What the savepoints `OnError::Skip` sets are named by,
/// then each its place.
const SAVEPOINT: &str = "TURBOREL_PIPELINE";

/// What a pipeline does after a request fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnError {
    /// Every request runs; a failure is its request's outcome alone.
    #[default]
    Continue,
    /// The batch stops at its first failure: the requests after it are
    /// undone or never sent, each `Piped::Skipped`.
    Skip,
}

/// The outcome of one request of a pipeline.
#[derive(Debug, Clone)]
pub enum Piped {
    /// What the request gave.
    Done(Outcome),
    /// How the request failed, as the server reported it.
    Failed(Failure),
    /// Not run, or run and undone, after an earlier request failed under
    /// `OnError::Skip`.
    Skipped,
}

/// A request queued, and how its reply is read.
enum Queued {
    Query(String),
    Execute(Statement, Vec<Value>),
    /// Close and Deallocate of a statement by its number, of a pool's
    /// reset (`pool/reset.rs`).
    Close(u32),
    Deallocate(u32),
    /// The savepoint `OnError::Skip` sets before the request of this
    /// place, its outcome never shown.
    Savepoint(usize),
}

/// Requests queued on a connection, written at once by `run`.
pub struct Pipeline<'c, S: Read + Write> {
    connection: &'c mut Connection<S>,
    queued: Vec<Queued>,
    on_error: OnError,
}

impl<S: Read + Write> Connection<S> {
    /// A pipeline of requests on this connection, empty, `OnError::Continue`.
    pub fn pipeline(&mut self) -> Pipeline<'_, S> {
        Pipeline { connection: self, queued: Vec::new(), on_error: OnError::Continue }
    }
}

impl<S: Read + Write> Pipeline<'_, S> {
    /// Queues `sql`, run as `Connection::query` runs it.
    pub fn query(&mut self, sql: &str) -> &mut Self {
        self.queued.push(Queued::Query(sql.to_owned()));
        self
    }

    /// Queues an execution of `statement` with `values`, as
    /// `Connection::execute` runs it: all of a query's rows.
    pub fn execute(&mut self, statement: &Statement, values: &[Value]) -> &mut Self {
        self.queued.push(Queued::Execute(statement.clone(), values.to_vec()));
        self
    }

    /// Queues the closing of statement `number`'s cursor, then its
    /// destruction: a pool's reset of what a borrower left.
    pub(crate) fn release(&mut self, number: u32) -> &mut Self {
        self.queued.push(Queued::Close(number));
        self.queued.push(Queued::Deallocate(number));
        self
    }

    /// What the batch does after a request fails.
    pub fn on_error(&mut self, on_error: OnError) -> &mut Self {
        self.on_error = on_error;
        self
    }

    /// How many requests are queued.
    pub fn len(&self) -> usize {
        self.queued.len()
    }

    /// Whether none is queued.
    pub fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    /// Writes the requests, a window at a time, and reads one outcome for
    /// each, in the order they were queued.
    ///
    /// # Errors
    ///
    /// A connection failure, once the connection is lost: what the
    /// requests answered before it is not given. Under `OnError::Skip`,
    /// the failure of undoing the requests after a failed one.
    pub fn run(self) -> Result<Vec<Piped>> {
        let Pipeline { connection, queued, on_error } = self;
        let skip = on_error == OnError::Skip;
        let began = connection.in_transaction();
        let sent = if skip { guarded(queued) } else { queued };
        let ends = connection.autocommit && !skip;
        let mut outcomes = Vec::with_capacity(sent.len());
        let attributes = connection.told.attributes();
        for window in windows(&sent, (ends, &attributes)) {
            if skip && outcomes.iter().any(|o| matches!(o, Piped::Failed(_))) {
                outcomes.extend(window.iter().map(|_| Piped::Skipped));
                continue;
            }
            outcomes.extend(read_window(connection, &window)?);
        }
        if skip {
            settle(connection, &sent, &mut outcomes, began)?;
        }
        let shown = sent.iter().zip(outcomes).filter(|(q, _)| !matches!(q, Queued::Savepoint(_)));
        Ok(shown.map(|(_, o)| o).collect())
    }
}

/// The savepoint `OnError::Skip` sets before the `n`th request.
fn savepoint(n: usize) -> String {
    format!("{SAVEPOINT}_{n}")
}

/// `queued`, a savepoint of its own before each request but the first:
/// so the requests after a failed one are undone, and none before it.
fn guarded(queued: Vec<Queued>) -> Vec<Queued> {
    let mut sent = Vec::with_capacity(queued.len().saturating_mul(2));
    for (n, q) in queued.into_iter().enumerate() {
        if n > 0 {
            sent.push(Queued::Savepoint(n));
        }
        sent.push(q);
    }
    sent
}

/// The requests of `queued`, the transaction ended with each if `ends.0`,
/// carrying the attributes `ends.1`,
/// grouped into windows of at most `PIPELINE_WINDOW` bytes: a request
/// longer than that alone in its own.
fn windows<'q>(queued: &'q [Queued], ends: (bool, &Attributes)) -> Vec<Vec<(&'q Queued, Vec<u8>)>> {
    let mut windows: Vec<Vec<(&Queued, Vec<u8>)>> = Vec::new();
    let mut filled = PIPELINE_WINDOW;
    for q in queued {
        let mut bytes = Vec::new();
        request(q, ends).encode(&mut bytes);
        filled = filled.saturating_add(bytes.len());
        if filled > PIPELINE_WINDOW {
            windows.push(Vec::new());
            filled = bytes.len();
        }
        if let Some(last) = windows.last_mut() {
            last.push((q, bytes));
        }
    }
    windows
}

/// The request `q` sends, the transaction ended with it if `ends`, a
/// statement of the program's carrying `attributes`.
fn request(q: &Queued, (ends, attributes): (bool, &Attributes)) -> Request {
    match q {
        Queued::Query(sql) => Request::Query { sql: sql.clone(), ends, attributes: attributes.clone(), multi: false },
        Queued::Execute(statement, values) => execute_request(statement, (0, values), 0, (ends, attributes.clone())),
        Queued::Close(statement) => Request::Close { statement: *statement, cursor: 0 },
        Queued::Deallocate(statement) => Request::Deallocate { statement: *statement },
        Queued::Savepoint(n) => {
            Request::Query { sql: format!("SAVEPOINT {}", savepoint(*n)), ends: false, attributes: Vec::new(), multi: false }
        }
    }
}

/// Writes `window` at once and reads its outcomes.
fn read_window<S: Read + Write>(connection: &mut Connection<S>, window: &[(&Queued, Vec<u8>)]) -> Result<Vec<Piped>> {
    let bytes: Vec<u8> = window.iter().flat_map(|(_, b)| b.iter().copied()).collect();
    connection.write_encoded(&bytes)?;
    let mut outcomes = Vec::with_capacity(window.len());
    for (q, _) in window {
        let reply = match q {
            Queued::Query(_) | Queued::Savepoint(_) => connection.query_reply(),
            Queued::Execute(statement, _) => connection.execute_reply(statement),
            Queued::Close(_) | Queued::Deallocate(_) => connection.completion().map(Outcome::Done),
        };
        outcomes.push(match reply {
            Ok(outcome) => Piped::Done(outcome),
            Err(failure) => {
                if connection.is_lost() {
                    return Err(failure);
                }
                Piped::Failed(failure)
            }
        });
    }
    Ok(outcomes)
}

/// Ends a batch of `sent` run under `OnError::Skip`, of `outcomes`: the
/// requests after the first that failed undone, to the savepoint set
/// after it, and each marked `Skipped`; the savepoints released; and,
/// under autocommit of a batch that began no transaction, its work
/// committed.
fn settle<S: Read + Write>(connection: &mut Connection<S>, sent: &[Queued], outcomes: &mut [Piped], began: bool) -> Result<()> {
    let failed = outcomes.iter().position(|o| matches!(o, Piped::Failed(_)));
    let mut statements = Vec::new();
    if let Some(at) = failed {
        let after = (sent.get(at.saturating_add(1)), outcomes.get(at.saturating_add(1)));
        if let (Some(Queued::Savepoint(n)), Some(Piped::Done(_))) = after {
            statements.push(format!("ROLLBACK TO SAVEPOINT {}", savepoint(*n)));
        }
        outcomes.iter_mut().skip(at.saturating_add(1)).for_each(|o| *o = Piped::Skipped);
    }
    let first = (sent.get(1), outcomes.get(1));
    if let (Some(Queued::Savepoint(n)), Some(Piped::Done(_))) = first {
        statements.push(format!("RELEASE SAVEPOINT {}", savepoint(*n)));
    }
    if !began && connection.autocommit {
        statements.push("COMMIT".to_owned());
    }
    statements.iter().try_for_each(|sql| within(connection, sql))
}

/// Runs `sql` within the transaction, whatever `set_autocommit` says: a
/// statement of the batch's own, which must not end it.
fn within<S: Read + Write>(connection: &mut Connection<S>, sql: &str) -> Result<()> {
    connection.send(&Request::Query { sql: sql.to_owned(), ends: false, attributes: Vec::new(), multi: false })?;
    connection.query_reply().map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D200: requests are grouped into windows
    /// of at most `PIPELINE_WINDOW` bytes, in order, a request longer than
    /// a window alone in one; none queued, no window.
    #[test]
    fn requests_are_written_a_window_at_a_time() {
        let small = Queued::Query("VALUES 1".to_owned());
        let half = Queued::Query("x".repeat(PIPELINE_WINDOW.checked_div(2).unwrap()));
        let long = Queued::Query("y".repeat(PIPELINE_WINDOW));
        let queued = [Queued::Query("a".to_owned()), small, half, long, Queued::Query("z".to_owned())];
        let sizes: Vec<usize> = windows(&queued, (false, &Vec::new())).iter().map(Vec::len).collect();
        assert_eq!(sizes, [3, 1, 1]);
        let all: usize = windows(&queued, (true, &Vec::new())).iter().flatten().map(|(_, b)| b.len()).sum();
        let mut whole = Vec::new();
        for q in &queued {
            request(q, (true, &Vec::new())).encode(&mut whole);
        }
        assert_eq!(all, whole.len(), "every request, each once");
        assert!(windows(&[], (false, &Vec::new())).is_empty());
        assert_eq!(OnError::default(), OnError::Continue);
    }
}
