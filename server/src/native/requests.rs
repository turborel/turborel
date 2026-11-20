//! What a session's worker does with a request (RULES.md D45): its
//! statements, prepared ones among them [ISO/IEC 9075:1992 §17], and their
//! cursors. Its replies are gathered, and written a batch's worth at a
//! time, the rest with the last (`Outgoing`), so a small result takes one
//! write, and the small results of pipelined requests one between them
//! (`native/lent.rs`, RULES.md D231). A request that ends the transaction with its statement
//! \[EXTENSION\] commits once the statement is done, or rolls back if it
//! failed, its one reply saying no transaction is active, so a statement
//! that is its own transaction is one request and one reply. Nothing else
//! runs with a statement: its reply is never held back while another may
//! wait on a client reading it.

use std::collections::HashMap;
use std::mem;
use std::sync::Arc;
use std::time::Instant;

use turborel::db::{Completion, Cursor, Database, Direct, Prepared, Session};
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel::storage::Row;
use turborel::value::Value;
use turborel_protocol::{Attributes, Capabilities, Column, Completed, Described, Failure, LobRequest, Reply, Timing, row_size};

use super::copy::{self, Copy};
use super::status::Status;
use crate::settings::DEFAULT_LOB_INLINE_BYTES;
use crate::shown::Requests;

mod replied;
mod timed;

/// The number of a prepared statement and of one of its cursors, the
/// client's to choose (RULES.md D188); 0 for its one cursor.
pub(super) type At = (u32, u32);

/// What a worker is asked to do.
pub(super) enum Work {
    /// A `<direct SQL statement>`: all its rows; the transaction ended
    /// with it if `ends`; several, run in order, if `multi` (RULES.md
    /// D192); of the request's `attributes` (D191).
    Query { sql: String, ends: bool, attributes: Option<Arc<Attributes>>, multi: bool },
    /// [§17.6] A statement to prepare as number `.0`.
    Prepare(u32, String),
    /// [§17.10, §17.14] Statement `at.0` executed with `values`, its
    /// cursor, of a query, numbered `at.1`; at most `rows` rows sent, all
    /// if 0; the transaction ended with it if `ends`.
    Execute { at: At, values: Vec<Value>, rows: u32, ends: bool, attributes: Option<Arc<Attributes>> },
    /// [§17.15] At most `.1` more rows of the cursor `.0`.
    Fetch(At, u32),
    /// [§17.16] The cursor `.0` closed.
    Close(At),
    /// [§17.7] Statement `.0` destroyed.
    Deallocate(u32),
    /// \[DESIGN\] What a client asks of a large object by its locator
    /// (`native/lobs.rs`).
    Lob(LobRequest),
    /// \[DESIGN\] A prepared statement or an open cursor described.
    Describe(Described),
    /// \[DESIGN\] A copy into `table`'s `columns` begun (`native/copy.rs`).
    CopyIn { table: String, columns: Vec<String>, ends: bool },
    /// Rows of the copy.
    CopyData(Vec<Row>),
    /// The copy's end, abandoned if `.0`.
    CopyEnd(bool),
}

impl Work {
    /// Whether it is of a copy under way.
    fn copies(&self) -> bool {
        match self {
            Work::CopyData(_) | Work::CopyEnd(_) => true,
            Work::Query { .. }
            | Work::Prepare(..)
            | Work::Execute { .. }
            | Work::Fetch(..)
            | Work::Close(_)
            | Work::Deallocate(_)
            | Work::Lob(_)
            | Work::Describe(_)
            | Work::CopyIn { .. } => false,
        }
    }
}

/// A cursor of a prepared statement, open between requests, the row read
/// past the last sent, which showed it had more, the warning its rows
/// raised as it was opened, its completion's once all are sent, the
/// attributes of the Execute that opened it, its fetches' too, the rows
/// sent so far, and whether the transaction ends as it does (RULES.md
/// D199).
struct Open {
    cursor: Cursor,
    next: Option<Row>,
    warning: Option<SqlState>,
    attributes: Option<Arc<Attributes>>,
    sent: u64,
    ends: bool,
    /// The time of the requests that read it so far (RULES.md D240).
    spent: Timing,
}

impl Open {
    /// `cursor`, just opened, its rows raising `warning`.
    fn new(cursor: Cursor, warning: Option<SqlState>) -> Open {
        Open { cursor, next: None, warning, attributes: None, sent: 0, ends: false, spent: Timing::default() }
    }
}

/// What a request's work is, as its completion counts it (RULES.md D197).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Counted {
    /// A statement run: its rows, as ROW_COUNT gives them, or a query's.
    Statement,
    /// A cursor's rows fetched: the query's, so far.
    Fetched,
    /// A copy: its rows, whatever was granted.
    Copy,
    /// Anything else: none.
    Uncounted,
}

/// What a session's requests reach beyond it: the database it is of,
/// which a statement waits on to have applied a commit (`native/position.rs`),
/// and the attributes of the requests running (`shown/requests.rs`).
#[derive(Clone)]
pub(super) struct Serves {
    pub(super) db: Database,
    pub(super) requests: Arc<Requests>,
}

/// A connection's session and what it holds between requests: moved to the
/// worker that runs each, and back.
pub(super) struct Held {
    pub(super) session: Session,
    statements: HashMap<u32, Prepared>,
    cursors: HashMap<At, Open>,
    copy: Option<Copy>,
    /// What the client was granted (RULES.md D185).
    pub(super) granted: Capabilities,
    /// What it was last told of the session's settings (`native/status.rs`).
    pub(super) status: Status,
    pub(super) serves: Serves,
    /// When the request was read, if its time is told (`requests/timed.rs`).
    arrived: Option<Instant>,
    /// The clock of the statement running, if its time is told.
    clock: Option<timed::Clock>,
    /// The time of a cursor's earlier requests, or a copy's.
    carried: Timing,
}

/// Why rows stopped before their end.
enum Cut {
    /// The connection is gone, a write having failed so: nothing more is
    /// sent.
    Gone(SqlError),
    /// Fetching failed.
    Failed(SqlError),
}

/// [ISO/IEC 9075:1992 §17.10 GR1] No prepared statement of number `n`.
fn no_statement(n: u32) -> SqlError {
    SqlError::new(Code::Dynamic, format!("[ISO/IEC 9075:1992 §17.10 GR1] there is no prepared statement {n}"))
        .with_state(SqlState::INVALID_STATEMENT_NAME)
}

/// An invalid cursor state (SQLSTATE 24000): `msg`.
fn cursor_state(msg: &str) -> SqlError {
    SqlError::new(Code::Cursor, msg)
}

/// The cursor is not open [ISO/IEC 9075:1992 §13.3 GR1, §13.4 GR1].
fn not_open() -> SqlError {
    cursor_state("[ISO/IEC 9075:1992 §13.3 GR1, §13.4 GR1] the cursor is not open")
}

/// The reply that describes `prepared`, statement `n`.
fn prepared_reply(n: u32, prepared: &Prepared) -> Reply {
    Reply::Prepared {
        statement: n,
        parameters: prepared.parameters().to_vec(),
        columns: prepared.columns().iter().map(Column::from).collect(),
    }
}

/// A request's replies: those gathered, and what a batch's worth is sent
/// through, ahead of the rest.
pub(super) struct Outgoing<'a> {
    sink: &'a mut dyn FnMut(Vec<Reply>) -> Result<()>,
    /// The bytes a batch of rows is.
    batch: usize,
    /// SERVER.LOB_INLINE_BYTES: the longest large object sent whole.
    inline: usize,
    /// Whether a locator was sent (`native/lobs.rs`).
    located: bool,
    pub(super) gathered: Vec<Reply>,
}

impl<'a> Outgoing<'a> {
    /// None gathered yet, batches of `batch` bytes sent through `sink`.
    pub(super) fn new(sink: &'a mut dyn FnMut(Vec<Reply>) -> Result<()>, batch: usize) -> Outgoing<'a> {
        Outgoing { sink, batch, inline: DEFAULT_LOB_INLINE_BYTES, located: false, gathered: Vec::new() }
    }

    /// As it is, large objects of at most `bytes` octets sent whole.
    #[must_use]
    pub(super) fn inlining(self, bytes: usize) -> Outgoing<'a> {
        Outgoing { inline: bytes, ..self }
    }

    /// The replies gathered and not sent ahead: the request's last, for
    /// the wire to send, or to queue before the next pipelined request's.
    pub(super) fn finish(self) -> Vec<Reply> {
        self.gathered
    }

    /// Sends what is gathered, ahead of the rest.
    fn send(&mut self) -> std::result::Result<(), Cut> {
        (self.sink)(mem::take(&mut self.gathered)).map_err(Cut::Gone)
    }

    /// Gathers the rows of `rows`, if any.
    fn rows(&mut self, rows: &mut Vec<Row>) {
        if !rows.is_empty() {
            self.gathered.push(Reply::Rows(mem::take(rows)));
        }
    }
}

impl Held {
    /// What `session` holds, as it begins, its client granted `granted`.
    pub(super) fn new(session: Session, serves: Serves, granted: Capabilities) -> Held {
        let (statements, cursors, status) = (HashMap::new(), HashMap::new(), Status::default());
        Held { session, statements, cursors, copy: None, granted, status, serves, arrived: None, clock: None, carried: Timing::default() }
    }

    /// Abandons the copy under way, if one is, everything it did undone:
    /// as the client terminates the session, so a copy is never committed
    /// in part.
    pub(super) fn abandon_copy(&mut self) {
        if let Some(copy) = self.copy.take() {
            // A failed undo has marked its tables damaged (RULES.md D19).
            let _ = copy.end(&self.session, true);
        }
    }

    /// `session` in place of this one's session, holding nothing yet, the
    /// client's grants kept (RULES.md D189): the old one.
    pub(super) fn renew(&mut self, session: Session) -> Session {
        self.cursors.clear();
        self.statements.clear();
        (self.copy, self.status, self.carried) = (None, Status::default(), Timing::default());
        mem::replace(&mut self.session, session)
    }

    /// Does `work`, its replies gathered in `out`, within `max_statements`
    /// prepared statements: one statement, or each of several in order.
    ///
    /// # Errors
    ///
    /// A write of replies failed: the connection is gone; or a copy's
    /// request came out of turn.
    pub(super) fn handle(&mut self, work: Work, out: &mut Outgoing<'_>, max_statements: usize) -> Result<()> {
        if self.copy.is_some() != work.copies() {
            return Err(copy::out_of_turn());
        }
        if let Work::Query { sql, ends, attributes, multi: true } = work {
            return self.several(&sql, ends, attributes.as_ref(), out);
        }
        self.one(work, out, (max_statements, false)).map(drop)
    }

    /// Does `work`, one statement's: its last reply gathered, after its
    /// notices and the settings it changed (`Held::told`); followed by
    /// another statement's if `more`; whether it failed. The transaction
    /// is ended after it if the request says so (`ended`).
    ///
    /// # Errors
    ///
    /// A write of replies failed: the connection is gone.
    pub(super) fn one(&mut self, work: Work, out: &mut Outgoing<'_>, (max_statements, more): (usize, bool)) -> Result<bool> {
        self.start();
        let ends = self.ends(&work);
        let counted = Counted::of(&work);
        let attributes = self.attributes(&work);
        let requests = Arc::clone(&self.serves.requests);
        let running = requests.running(attributes.as_ref());
        let served = Arc::clone(&self.session.canceller());
        served.served().set_trace(crate::shown::trace(attributes.as_deref()));
        let done = self.done(work, out, max_statements);
        served.served().set_trace(None);
        drop(running);
        let last = match done {
            Ok(Some(last)) => last,
            Ok(None) => {
                self.kept();
                return Ok(false);
            }
            Err(Cut::Gone(e)) => return Err(e),
            Err(Cut::Failed(e)) => Reply::Error(Failure::from(&e)),
        };
        // Counted before the transaction ends, as the statement left them.
        let last = self.counted(last, counted);
        // A locator sent is valid until the transaction ends: it is kept.
        let last = if ends && !out.located { self.ended(last) } else { last };
        let last = self.timed(last, counted);
        let last = self.finished(last, more);
        let last = self.told(last, out);
        let failed = matches!(last, Reply::Error(_));
        out.gathered.push(last);
        Ok(failed)
    }

    /// Whether the transaction ends with `work`.
    fn ends(&self, work: &Work) -> bool {
        match work {
            Work::Query { ends, .. } | Work::Execute { ends, .. } => *ends,
            Work::CopyEnd(_) => self.copy.as_ref().is_some_and(Copy::ends),
            Work::Fetch(at, _) | Work::Close(at) => self.cursors.get(at).is_some_and(|open| open.ends),
            Work::Prepare(..) | Work::Deallocate(_) | Work::Lob(_) | Work::Describe(_) | Work::CopyIn { .. } | Work::CopyData(_) => false,
        }
    }

    /// The attributes `work` runs with: its own, or those of the Execute
    /// that opened the cursor it fetches from.
    fn attributes(&self, work: &Work) -> Option<Arc<Attributes>> {
        match work {
            Work::Query { attributes, .. } | Work::Execute { attributes, .. } => attributes.clone(),
            Work::Fetch(at, _) => self.cursors.get(at).and_then(|open| open.attributes.clone()),
            Work::Prepare(..)
            | Work::Close(_)
            | Work::Deallocate(_)
            | Work::Lob(_)
            | Work::Describe(_)
            | Work::CopyIn { .. }
            | Work::CopyData(_)
            | Work::CopyEnd(_) => None,
        }
    }

    /// \[EXTENSION\] `last`, the reply of a statement whose request ends the
    /// transaction with it, once it is ended: committed after a
    /// completion, the commit's failure in its place if it fails; rolled
    /// back after a failure, the rollback's failure in its place if that
    /// fails, the tables then damaged (RULES.md D19). A query's cursor
    /// left open, its rows not all sent, keeps the transaction.
    fn ended(&mut self, last: Reply) -> Reply {
        let outcome = match &last {
            Reply::Complete(_) => self.session.commit(),
            Reply::Error(_) => self.session.rollback(),
            Reply::Suspended
            | Reply::Lob(_)
            | Reply::Prepared { .. }
            | Reply::Description(_)
            | Reply::Rows(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Notice(_)
            | Reply::Status { .. }
            | Reply::Pong => {
                return last;
            }
        };
        match outcome {
            Ok(()) => last,
            Err(e) => Reply::Error(Failure::from(&e)),
        }
    }

    /// Does `work`: the last reply, none if it has none.
    fn done(&mut self, work: Work, out: &mut Outgoing<'_>, max_statements: usize) -> std::result::Result<Option<Reply>, Cut> {
        let failed = Cut::Failed;
        match work {
            Work::Query { sql, .. } => {
                super::position::applied(&self.session, &self.serves.db, Some(&sql)).map_err(failed)?;
                self.direct(&sql, out).map(Some)
            }
            Work::Prepare(n, sql) => self.prepare(n, &sql, max_statements).map(Some).map_err(failed),
            Work::Execute { at, values, rows, attributes, ends } => {
                super::position::applied(&self.session, &self.serves.db, None).map_err(failed)?;
                let ends = ends && self.granted.has(turborel_protocol::capability::FETCH_ENDS);
                self.execute(at, &values, (rows, out), (attributes, ends))
            }
            Work::Fetch(at, rows) => self.fetch(at, (rows, out)),
            Work::Close(at) => {
                // [§17.16 GR1, §13.4 GR1]
                let closed = self.cursors.remove(&at).ok_or_else(not_open);
                closed.map(|_| Some(Reply::Complete(Completion::Success.into()))).map_err(failed)
            }
            Work::Deallocate(n) => self.deallocate(n).map(|()| Some(Reply::Complete(Completion::Success.into()))).map_err(failed),
            Work::Lob(request) => self.lob(request).map(Some).map_err(failed),
            Work::Describe(what) => self.describe(what).map(Some).map_err(failed),
            Work::CopyIn { table, columns, ends } => {
                self.copy = Some(Copy::begin(&self.session, (&table, &columns), ends));
                Ok(None)
            }
            Work::CopyData(rows) => {
                if let Some(copy) = self.copy.as_mut() {
                    copy.rows(&self.session, rows);
                }
                Ok(None)
            }
            Work::CopyEnd(abandon) => {
                let copy = self.copy.take().ok_or_else(|| Cut::Gone(copy::out_of_turn()))?;
                copy.end(&self.session, abandon).map(|c| Some(Reply::Complete(c))).map_err(failed)
            }
        }
    }

    /// Runs `sql` directly: its rows, after their Description, then its
    /// completion.
    fn direct(&mut self, sql: &str, out: &mut Outgoing<'_>) -> std::result::Result<Reply, Cut> {
        match self.session.direct(sql).map_err(Cut::Failed)? {
            Direct::Done(completion) => Ok(Reply::Complete(completion.into())),
            Direct::Rows(cursor) => {
                let mut open = Open::new(cursor, self.session.warning());
                describe(&open.cursor, out);
                send_rows(&self.session, &mut open, 0, out).map(|ended| ended.reply())
            }
        }
    }

    /// \[DESIGN\] RULES.md D188: describes a prepared statement, its
    /// parameters and columns, or an open cursor, its columns.
    fn describe(&self, what: Described) -> Result<Reply> {
        match what {
            Described::Statement(n) => self.statements.get(&n).map(|p| prepared_reply(n, p)).ok_or_else(|| no_statement(n)),
            Described::Cursor { statement, cursor } => {
                let open = self.cursors.get(&(statement, cursor)).ok_or_else(not_open)?;
                Ok(Reply::Description(open.cursor.columns.iter().map(Column::from).collect()))
            }
        }
    }

    /// [§17.7] Destroys statement `n`: SQLSTATE 26000 if there is none
    /// \[GR1\], 24000 if its cursor is open \[GR2\].
    fn deallocate(&mut self, n: u32) -> Result<()> {
        if self.cursors.keys().any(|(statement, _)| *statement == n) {
            return Err(cursor_state("[ISO/IEC 9075:1992 §17.7 GR2] a cursor of the statement is open: close it first"));
        }
        self.statements.remove(&n).map(drop).ok_or_else(|| no_statement(n))
    }

    /// [§17.6] Prepares `sql` as statement `n`, destroying any it had of
    /// that number \[GR6\]; at most `max_statements` at once.
    fn prepare(&mut self, n: u32, sql: &str, max_statements: usize) -> Result<Reply> {
        if self.statements.contains_key(&n) {
            self.deallocate(n)?;
        } else if self.statements.len() >= max_statements {
            let msg = format!("[DESIGN] a connection holds at most {max_statements} prepared statements (SERVER.MAX_STATEMENTS)");
            return Err(SqlError::new(Code::Resources, msg));
        }
        let prepared = self.session.prepare(sql)?;
        let reply = prepared_reply(n, &prepared);
        self.statements.insert(n, prepared);
        Ok(reply)
    }

    /// [§17.10, §17.14] Executes statement `at.0` with `values`; a
    /// query's cursor opened as `at.1`, of `attributes`, the transaction
    /// ending as it does if `ends`, at most `rows` of it sent, of the
    /// columns Prepared gave, with no Description. A CALL's output dynamic
    /// parameters are sent as a row of those columns before its Complete.
    fn execute(
        &mut self,
        at: At,
        values: &[Value],
        (rows, out): (u32, &mut Outgoing<'_>),
        (attributes, ends): (Option<Arc<Attributes>>, bool),
    ) -> std::result::Result<Option<Reply>, Cut> {
        let prepared = self.statements.get(&at.0).ok_or_else(|| Cut::Failed(no_statement(at.0)))?;
        if !prepared.is_query() {
            let (done, outputs) = self.session.execute_prepared_out(prepared, values).map_err(Cut::Failed)?;
            // [ISO/IEC 9075-2:2023 §20.7 GR7a] a CALL's output dynamic
            // parameters, a row of the columns Prepared gave.
            if prepared.has_outputs() {
                let row = super::lobs::placed_row(&self.session, outputs, out.inline, &mut out.located).map_err(Cut::Failed)?;
                out.rows(&mut vec![row]);
            }
            return Ok(Some(Reply::Complete(done.into())));
        }
        // [§17.14 GR6, §13.2 GR1]
        if self.cursors.contains_key(&at) {
            return Err(Cut::Failed(cursor_state("[ISO/IEC 9075:1992 §13.2 GR1] the cursor is open")));
        }
        let cursor = self.session.open_prepared(prepared, values).map_err(Cut::Failed)?;
        let warning = self.session.warning();
        self.sent(at, Open { attributes, ends, ..Open::new(cursor, warning) }, (rows, out))
    }

    /// [§17.15] At most `rows` more rows of the cursor `at`: SQLSTATE
    /// 24000 if it is not open [§13.3 GR1].
    fn fetch(&mut self, at: At, (rows, out): (u32, &mut Outgoing<'_>)) -> std::result::Result<Option<Reply>, Cut> {
        let open = self.cursors.remove(&at).ok_or_else(|| Cut::Failed(not_open()))?;
        self.sent(at, open, (rows, out))
    }

    /// Sends at most `rows` rows of `open`, the cursor `at`: kept open if
    /// it has more, closed once all are sent.
    fn sent(&mut self, at: At, mut open: Open, (rows, out): (u32, &mut Outgoing<'_>)) -> std::result::Result<Option<Reply>, Cut> {
        let ended = send_rows(&self.session, &mut open, rows, out)?;
        if let Ended::More = ended {
            open.spent = self.so_far().map_or(open.spent, |t| open.spent.plus(t));
            self.cursors.insert(at, open);
        } else {
            self.carried = open.spent;
        }
        Ok(Some(ended.reply()))
    }
}

/// How rows sent ended.
enum Ended {
    /// The cursor has more.
    More,
    /// It has no more: how many were sent, so far [ISO/IEC 9075:1992 §20.2
    /// GR3], whether any were by this request, and the warning its rows
    /// raised, if any.
    Done(u64, bool, Option<SqlState>),
}

impl Ended {
    /// The reply that says so: a completion with the warning, if any
    /// [ISO/IEC 9075:1992 §3.3.4.1], where rows were sent.
    fn reply(&self) -> Reply {
        match self {
            Ended::More => Reply::Suspended,
            Ended::Done(all, true, warning) => {
                Reply::Complete(Completed { rows: Some(*all), ..Completion::Success.warned(*warning).into() })
            }
            Ended::Done(all, false, _) => Reply::Complete(Completed { rows: Some(*all), ..Completion::NoData.into() }),
        }
    }
}

/// Gathers the columns of `cursor`'s rows.
fn describe(cursor: &Cursor, out: &mut Outgoing<'_>) {
    out.gathered.push(Reply::Description(cursor.columns.iter().map(Column::from).collect()));
}

/// Gathers at most `limit` rows of `open`, all if 0, sent ahead in batches
/// of about `out`'s bytes, each large object placed for `session`
/// (`native/lobs.rs`); reads one row past them, to tell if it has more.
fn send_rows(session: &Session, open: &mut Open, limit: u32, out: &mut Outgoing<'_>) -> std::result::Result<Ended, Cut> {
    let limit = if limit == 0 { usize::MAX } else { usize::try_from(limit).unwrap_or(usize::MAX) };
    let (mut sent, mut rows, mut bytes): (usize, Vec<Row>, usize) = (0, Vec::new(), 0);
    loop {
        let row = match open.next.take() {
            Some(row) => Some(row),
            None => open.cursor.fetch().map_err(Cut::Failed)?,
        };
        let Some(row) = row else { break };
        if sent == limit {
            open.next = Some(row);
            out.rows(&mut rows);
            return Ok(Ended::More);
        }
        sent = sent.saturating_add(1);
        open.sent = open.sent.saturating_add(1);
        let row = super::lobs::placed_row(session, row, out.inline, &mut out.located).map_err(Cut::Failed)?;
        bytes = bytes.saturating_add(row_size(&row));
        rows.push(row);
        if bytes >= out.batch {
            out.rows(&mut rows);
            out.send()?;
            bytes = 0;
        }
    }
    out.rows(&mut rows);
    Ok(Ended::Done(open.sent, sent > 0, open.warning))
}

#[cfg(test)]
mod tests;
