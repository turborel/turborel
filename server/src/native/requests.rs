//! What a session's worker does with a request (RULES.md D45): its
//! statements, prepared ones among them [ISO/IEC 9075:1992 §17], and their
//! cursors. Its replies are gathered and given back with the session, so
//! a small result takes one wake of the connection's task and one write;
//! a batch's worth is sent ahead through a bounded channel. A commit or
//! rollback sent with a statement runs with it on one worker, its reply
//! with the statement's. Nothing else does: a statement's reply is never
//! held back while another may wait on a client reading it.

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

/// A request's replies: those gathered, and where a batch's worth goes.
pub(super) struct Outgoing<'a> {
    sender: &'a mpsc::Sender<Vec<Reply>>,
    /// The bytes a batch of rows is.
    batch: usize,
    gathered: Vec<Reply>,
}

impl<'a> Outgoing<'a> {
    /// None gathered yet, batches of `batch` bytes sent to `sender`.
    pub(super) fn new(sender: &'a mpsc::Sender<Vec<Reply>>, batch: usize) -> Outgoing<'a> {
        Outgoing { sender, batch, gathered: Vec::new() }
    }

    /// The replies gathered and not sent ahead: the request's last.
    pub(super) fn gathered(self) -> Vec<Reply> {
        self.gathered
    }

    /// Sends what is gathered, ahead of the rest.
    fn send(&mut self) -> std::result::Result<(), Cut> {
        self.sender.blocking_send(mem::take(&mut self.gathered)).map_err(|_| Cut::Gone)
    }

    /// Gathers the rows of `rows`, if any.
    fn rows(&mut self, rows: &mut Vec<Row>) {
        if !rows.is_empty() {
            self.gathered.push(Reply::Rows(mem::take(rows)));
        }
    }
}

impl Held {
    /// What `session` holds, as it begins.
    pub(super) fn new(session: Session) -> Held {
        Held { session, statements: HashMap::new(), cursors: HashMap::new() }
    }

    /// Does `work`, then what `then` holds if the connection is there and
    /// its check says the connection is still served, as receiving it would
    /// have asked; their replies gathered in `out`, within `max_statements`
    /// prepared statements.
    pub(super) fn handle_then(
        &mut self,
        work: Work,
        then: Option<(Work, impl FnOnce() -> bool)>,
        out: &mut Outgoing<'_>,
        max_statements: usize,
    ) {
        if self.handle(work, out, max_statements)
            && let Some((next, still)) = then
            && still()
        {
            self.handle(next, out, max_statements);
        }
    }

    /// Whether `work` is only a commit or a rollback: what may run with the
    /// statement before it (see the module's documentation).
    pub(super) fn ends_transaction(&self, work: &Work) -> bool {
        let ends =
            |sql: &str| ["COMMIT", "COMMIT WORK", "ROLLBACK", "ROLLBACK WORK"].iter().any(|end| sql.trim().eq_ignore_ascii_case(end));
        match work {
            Work::Query(sql) => ends(sql),
            Work::Execute(n, values, _) => values.is_empty() && self.statements.get(n).is_some_and(|p| ends(p.sql())),
            Work::Prepare(..) | Work::Fetch(..) | Work::Close(_) | Work::Deallocate(_) => false,
        }
    }

    /// Does `work`, its replies gathered in `out`; within `max_statements`
    /// prepared statements. Whether the connection is still there.
    fn handle(&mut self, work: Work, out: &mut Outgoing<'_>, max_statements: usize) -> bool {
        let last = match self.done(work, out, max_statements) {
            Ok(Some(last)) => last,
            Ok(None) => return true,
            Err(Cut::Gone) => return false,
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
        out.gathered.push(last);
        true
    }

    /// Does `work`: the last reply, none if its rows ended it.
    fn done(&mut self, work: Work, out: &mut Outgoing<'_>, max_statements: usize) -> std::result::Result<Option<Reply>, Cut> {
        let failed = Cut::Failed;
        match work {
            Work::Query(sql) => match self.session.direct(&sql).map_err(failed)? {
                Direct::Done(completion) => Ok(Some(Reply::Complete(completion.into()))),
                Direct::Rows(cursor) => {
                    let mut open = Open { cursor, next: None, warning: self.session.warning() };
                    describe(&open.cursor, out);
                    send_rows(&mut open, 0, out).map(|ended| Some(ended.reply()))
                }
            },
            Work::Prepare(n, sql) => self.prepare(n, &sql, max_statements).map(Some).map_err(failed),
            Work::Execute(n, values, rows) => self.execute(n, &values, (rows, out)),
            Work::Fetch(n, rows) => self.fetch(n, (rows, out)),
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
    fn execute(&mut self, n: u32, values: &[Value], (rows, out): (u32, &mut Outgoing<'_>)) -> std::result::Result<Option<Reply>, Cut> {
        let prepared = self.statements.get(&n).ok_or_else(|| Cut::Failed(no_statement(n)))?;
        if !prepared.is_query() {
            return self.session.execute_prepared(prepared, values).map(|c| Some(Reply::Complete(c.into()))).map_err(Cut::Failed);
        }
        // [§17.14 GR6, §13.2 GR1]
        if self.cursors.contains_key(&n) {
            return Err(Cut::Failed(cursor_state("[ISO/IEC 9075:1992 §13.2 GR1] the statement's cursor is open")));
        }
        let cursor = self.session.open_prepared(prepared, values).map_err(Cut::Failed)?;
        describe(&cursor, out);
        let warning = self.session.warning();
        self.sent(n, Open { cursor, next: None, warning }, (rows, out))
    }

    /// [§17.15] At most `rows` more rows of statement `n`'s cursor: SQLSTATE
    /// 24000 if it is not open [§13.3 GR1].
    fn fetch(&mut self, n: u32, (rows, out): (u32, &mut Outgoing<'_>)) -> std::result::Result<Option<Reply>, Cut> {
        let open =
            self.cursors.remove(&n).ok_or_else(|| Cut::Failed(cursor_state("[ISO/IEC 9075:1992 §13.3 GR1] the cursor is not open")))?;
        self.sent(n, open, (rows, out))
    }

    /// Sends at most `rows` rows of `open`, statement `n`'s cursor: kept
    /// open if it has more, closed once all are sent.
    fn sent(&mut self, n: u32, mut open: Open, (rows, out): (u32, &mut Outgoing<'_>)) -> std::result::Result<Option<Reply>, Cut> {
        let ended = send_rows(&mut open, rows, out)?;
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

/// Gathers the columns of `cursor`'s rows.
fn describe(cursor: &Cursor, out: &mut Outgoing<'_>) {
    out.gathered.push(Reply::Description(cursor.columns.iter().map(Column::from).collect()));
}

/// Gathers at most `limit` rows of `open`, all if 0, sent ahead in batches
/// of about `out`'s bytes; reads one row past them, to tell if it has more.
fn send_rows(open: &mut Open, limit: u32, out: &mut Outgoing<'_>) -> std::result::Result<Ended, Cut> {
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
        bytes = bytes.saturating_add(row_size(&row));
        rows.push(row);
        if bytes >= out.batch {
            out.rows(&mut rows);
            out.send()?;
            bytes = 0;
        }
    }
    out.rows(&mut rows);
    Ok(Ended::Done(sent > 0, open.warning))
}

#[cfg(test)]
mod tests {
    use turborel::db::Database;

    use super::*;

    /// A session of user X, of a database whose table X.T is empty.
    fn held() -> Held {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL)").unwrap();
        Held::new(db.session("X"))
    }

    /// Runs `work` then a commit, if `still`, in `held`, with a channel of
    /// room for what is sent ahead: the replies gathered.
    fn run_then_commit(held: &mut Held, work: Work, still: bool) -> Vec<Reply> {
        let (sender, _receiver) = mpsc::channel(4);
        let mut out = Outgoing::new(&sender, 1 << 20);
        held.handle_then(work, Some((Work::Query("COMMIT".into()), || still)), &mut out, 8);
        out.gathered()
    }

    /// Verifies: [DESIGN] RULES.md D45: a commit sent with a statement runs
    /// with it, both replies gathered, while the connection is still
    /// served; if it is not, an operator having ended it or the server
    /// stopping, the commit does not run: the transaction stays open, to
    /// be rolled back, as the commit would not have been received.
    #[test]
    fn a_commit_sent_with_a_statement_runs_only_while_still_served() {
        let insert = || Work::Query("INSERT INTO T VALUES (1)".into());
        let mut gone = held();
        assert_eq!(run_then_commit(&mut gone, insert(), false).len(), 1, "the statement's alone");
        assert!(gone.session.transaction().is_some(), "not committed");
        let mut served = held();
        assert_eq!(run_then_commit(&mut served, insert(), true).len(), 2, "both, together");
        assert!(served.session.transaction().is_none(), "committed");
    }

    /// Verifies: [DESIGN] RULES.md D45: only a commit or a rollback, by
    /// direct text or a prepared statement given no values, runs with the
    /// statement before it; any other statement's reply is sent before
    /// the next runs, so no reply waits on what may wait on its client.
    #[test]
    fn only_a_commit_or_a_rollback_runs_with_a_statement() {
        let mut held = held();
        let query = |sql: &str| Work::Query(sql.into());
        for sql in ["COMMIT", " commit work ", "ROLLBACK", "rollback work"] {
            assert!(held.ends_transaction(&query(sql)), "{sql}");
        }
        for sql in ["SELECT K FROM T", "INSERT INTO T VALUES (1)", "ROLLBACK TO SAVEPOINT S"] {
            assert!(!held.ends_transaction(&query(sql)), "{sql}");
        }
        let (sender, _receiver) = mpsc::channel(4);
        let mut out = Outgoing::new(&sender, 1 << 20);
        held.handle(Work::Prepare(1, "COMMIT".into()), &mut out, 8);
        held.handle(Work::Prepare(2, "SELECT K FROM T".into()), &mut out, 8);
        assert!(held.ends_transaction(&Work::Execute(1, Vec::new(), 0)));
        assert!(!held.ends_transaction(&Work::Execute(2, Vec::new(), 0)));
        assert!(!held.ends_transaction(&Work::Execute(9, Vec::new(), 0)), "no such statement");
        assert!(!held.ends_transaction(&Work::Fetch(1, 0)));
    }
}
