//! What a session's worker does with a request (RULES.md D45): its
//! statements, prepared ones among them [ISO/IEC 9075:1992 §17], and their
//! cursors, the replies sent through a bounded channel as they come. Split
//! from native.rs (RULES.md R-SIZE-4).

use std::collections::HashMap;
use std::mem;

use tokio::sync::mpsc;
use turborel::db::{Completion, Cursor, Direct, Prepared, Session};
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel::storage::Row;
use turborel::value::Value;
use turborel_protocol::{Column, Completed, Failure, Reply, row_size};

/// What a worker is asked to do.
pub(super) enum Work {
    /// A `<direct SQL statement>`: all its rows.
    Query(String),
    /// [§17.6] A statement to prepare as number `.0`.
    Prepare(u32, String),
    /// [§17.10, §17.14] Statement `.0` executed with values `.1`; of a query,
    /// at most `.2` rows sent, all if 0.
    Execute(u32, Vec<Value>, u32),
    /// [§17.15] At most `.1` more rows of statement `.0`'s cursor.
    Fetch(u32, u32),
    /// [§17.16] Statement `.0`'s cursor closed.
    Close(u32),
    /// [§17.7] Statement `.0` destroyed.
    Deallocate(u32),
}

/// A cursor of a prepared statement, open between requests, the row read
/// past the last sent, which showed it had more, and the warning its rows
/// raised as it was opened, its completion's once all are sent.
struct Open {
    cursor: Cursor,
    next: Option<Row>,
    warning: Option<SqlState>,
}

/// A connection's session and what it holds between requests: moved to the
/// worker that runs each, and back.
pub(super) struct Held {
    pub(super) session: Session,
    statements: HashMap<u32, Prepared>,
    cursors: HashMap<u32, Open>,
}

/// Why rows stopped before their end.
enum Cut {
    /// The connection is gone: nothing more is sent.
    Gone,
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

/// What a worker sends a request's replies to, and the bytes a batch of
/// rows is.
pub(super) type Sending<'a> = (&'a mpsc::Sender<Reply>, usize);

impl Held {
    /// What `session` holds, as it begins.
    pub(super) fn new(session: Session) -> Held {
        Held { session, statements: HashMap::new(), cursors: HashMap::new() }
    }

    /// Does `work`, its replies sent to `sending`; within `max_statements`
    /// prepared statements.
    pub(super) fn handle(&mut self, work: Work, sending: Sending<'_>, max_statements: usize) {
        let last = match self.done(work, sending, max_statements) {
            Ok(Some(last)) => last,
            Ok(None) | Err(Cut::Gone) => return,
            Err(Cut::Failed(e)) => Reply::Error(Failure::from(&e)),
        };
        // [ISO/IEC 9075:1992 §15.1 GR1, §15.3 GR6] whether the transaction
        // is active, for the client's connection statements.
        let active = self.session.transaction().is_some();
        let last = if let Reply::Complete(c) = last {
            Reply::Complete(Completed { active, ..c })
        } else if let Reply::Error(f) = last {
            Reply::Error(Failure { active, ..f })
        } else {
            last
        };
        // A client gone hears nothing.
        let _ = sending.0.blocking_send(last);
    }

    /// Does `work`: the last reply, none if its rows ended it.
    fn done(&mut self, work: Work, sending: Sending<'_>, max_statements: usize) -> std::result::Result<Option<Reply>, Cut> {
        let failed = Cut::Failed;
        match work {
            Work::Query(sql) => match self.session.direct(&sql).map_err(failed)? {
                Direct::Done(completion) => Ok(Some(Reply::Complete(completion.into()))),
                Direct::Rows(cursor) => {
                    let mut open = Open { cursor, next: None, warning: self.session.warning() };
                    describe(&open.cursor, sending)?;
                    send_rows(&mut open, 0, sending).map(|ended| Some(ended.reply()))
                }
            },
            Work::Prepare(n, sql) => self.prepare(n, &sql, max_statements).map(Some).map_err(failed),
            Work::Execute(n, values, rows) => self.execute(n, &values, (rows, sending)),
            Work::Fetch(n, rows) => self.fetch(n, (rows, sending)),
            Work::Close(n) => {
                // [§17.16 GR1, §13.4 GR1]
                let closed = self.cursors.remove(&n).ok_or_else(|| cursor_state("[ISO/IEC 9075:1992 §13.4 GR1] the cursor is not open"));
                closed.map(|_| Some(Reply::Complete(Completion::Success.into()))).map_err(failed)
            }
            Work::Deallocate(n) => self.deallocate(n).map(|()| Some(Reply::Complete(Completion::Success.into()))).map_err(failed),
        }
    }

    /// [§17.7] Destroys statement `n`: SQLSTATE 26000 if there is none
    /// \[GR1\], 24000 if its cursor is open \[GR2\].
    fn deallocate(&mut self, n: u32) -> Result<()> {
        if self.cursors.contains_key(&n) {
            return Err(cursor_state("[ISO/IEC 9075:1992 §17.7 GR2] the statement's cursor is open: close it first"));
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
        let reply = Reply::Prepared {
            statement: n,
            parameters: prepared.parameters().to_vec(),
            columns: prepared.columns().iter().map(Column::from).collect(),
        };
        self.statements.insert(n, prepared);
        Ok(reply)
    }

    /// [§17.10, §17.14] Executes statement `n` with `values`; a query's
    /// cursor opened, at most `rows` of it sent.
    fn execute(&mut self, n: u32, values: &[Value], (rows, sending): (u32, Sending<'_>)) -> std::result::Result<Option<Reply>, Cut> {
        let prepared = self.statements.get(&n).ok_or_else(|| Cut::Failed(no_statement(n)))?;
        if !prepared.is_query() {
            return self.session.execute_prepared(prepared, values).map(|c| Some(Reply::Complete(c.into()))).map_err(Cut::Failed);
        }
        // [§17.14 GR6, §13.2 GR1]
        if self.cursors.contains_key(&n) {
            return Err(Cut::Failed(cursor_state("[ISO/IEC 9075:1992 §13.2 GR1] the statement's cursor is open")));
        }
        let cursor = self.session.open_prepared(prepared, values).map_err(Cut::Failed)?;
        describe(&cursor, sending)?;
        let warning = self.session.warning();
        self.sent(n, Open { cursor, next: None, warning }, (rows, sending))
    }

    /// [§17.15] At most `rows` more rows of statement `n`'s cursor: SQLSTATE
    /// 24000 if it is not open [§13.3 GR1].
    fn fetch(&mut self, n: u32, (rows, sending): (u32, Sending<'_>)) -> std::result::Result<Option<Reply>, Cut> {
        let open =
            self.cursors.remove(&n).ok_or_else(|| Cut::Failed(cursor_state("[ISO/IEC 9075:1992 §13.3 GR1] the cursor is not open")))?;
        self.sent(n, open, (rows, sending))
    }

    /// Sends at most `rows` rows of `open`, statement `n`'s cursor: kept
    /// open if it has more, closed once all are sent.
    fn sent(&mut self, n: u32, mut open: Open, (rows, sending): (u32, Sending<'_>)) -> std::result::Result<Option<Reply>, Cut> {
        let ended = send_rows(&mut open, rows, sending)?;
        if let Ended::More = ended {
            self.cursors.insert(n, open);
        }
        Ok(Some(ended.reply()))
    }
}

/// How rows sent ended.
enum Ended {
    /// The cursor has more.
    More,
    /// It has no more: whether any were sent [ISO/IEC 9075:1992 §20.2 GR3],
    /// and the warning its rows raised, if any.
    Done(bool, Option<SqlState>),
}

impl Ended {
    /// The reply that says so: a completion with the warning, if any
    /// [ISO/IEC 9075:1992 §3.3.4.1], where rows were sent.
    fn reply(&self) -> Reply {
        match self {
            Ended::More => Reply::Suspended,
            Ended::Done(true, warning) => Reply::Complete(Completion::Success.warned(*warning).into()),
            Ended::Done(false, _) => Reply::Complete(Completion::NoData.into()),
        }
    }
}

/// Sends the columns of `cursor`'s rows.
fn describe(cursor: &Cursor, (sender, _): Sending<'_>) -> std::result::Result<(), Cut> {
    sender.blocking_send(Reply::Description(cursor.columns.iter().map(Column::from).collect())).map_err(|_| Cut::Gone)
}

/// Sends at most `limit` rows of `open`, all if 0, in batches of about the
/// bytes of `sending`; reads one row past them, to tell if it has more.
fn send_rows(open: &mut Open, limit: u32, (sender, batch): Sending<'_>) -> std::result::Result<Ended, Cut> {
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
            flush(&mut rows, sender)?;
            return Ok(Ended::More);
        }
        sent = sent.saturating_add(1);
        bytes = bytes.saturating_add(row_size(&row));
        rows.push(row);
        if bytes >= batch {
            flush(&mut rows, sender)?;
            bytes = 0;
        }
    }
    flush(&mut rows, sender)?;
    Ok(Ended::Done(sent > 0, open.warning))
}

/// Sends the rows of `rows`, if any.
fn flush(rows: &mut Vec<Row>, sender: &mpsc::Sender<Reply>) -> std::result::Result<(), Cut> {
    if rows.is_empty() {
        return Ok(());
    }
    sender.blocking_send(Reply::Rows(mem::take(rows))).map_err(|_| Cut::Gone)
}
