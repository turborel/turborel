//! What a session's worker does with a request (RULES.md D45): its
//! statements, prepared ones among them [ISO/IEC 9075:1992 §17], and their
//! cursors. Its replies are gathered, and written a batch's worth at a
//! time, the rest with the last (`Outgoing`), so a small result takes one
//! write. A request that ends the transaction with its statement
//! \[EXTENSION\] commits once the statement is done, or rolls back if it
//! failed, its one reply saying no transaction is active, so a statement
//! that is its own transaction is one request and one reply. Nothing else
//! runs with a statement: its reply is never held back while another may
//! wait on a client reading it.

use std::collections::HashMap;
use std::mem;

use turborel::db::{Completion, Cursor, Direct, Prepared, Session};
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel::storage::Row;
use turborel::value::Value;
use turborel_protocol::{Column, Completed, Failure, Reply, row_size};

/// What a worker is asked to do.
pub(super) enum Work {
    /// A `<direct SQL statement>`: all its rows; the transaction ended
    /// with it if `ends` (`Request::Query`).
    Query { sql: String, ends: bool },
    /// [§17.6] A statement to prepare as number `.0`.
    Prepare(u32, String),
    /// [§17.10, §17.14] Statement `statement` executed with `values`; of a
    /// query, at most `rows` rows sent, all if 0; the transaction ended
    /// with it if `ends` (`Request::Execute`).
    Execute { statement: u32, values: Vec<Value>, rows: u32, ends: bool },
    /// [§17.15] At most `.1` more rows of statement `.0`'s cursor.
    Fetch(u32, u32),
    /// [§17.16] Statement `.0`'s cursor closed.
    Close(u32),
    /// [§17.7] Statement `.0` destroyed.
    Deallocate(u32),
}

impl Work {
    /// Whether the transaction ends with it.
    fn ends(&self) -> bool {
        match self {
            Work::Query { ends, .. } | Work::Execute { ends, .. } => *ends,
            Work::Prepare(..) | Work::Fetch(..) | Work::Close(_) | Work::Deallocate(_) => false,
        }
    }
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

/// A request's replies: those gathered, and what a batch's worth is sent
/// through, ahead of the rest.
pub(super) struct Outgoing<'a> {
    sink: &'a mut dyn FnMut(Vec<Reply>) -> Result<()>,
    /// The bytes a batch of rows is.
    batch: usize,
    gathered: Vec<Reply>,
}

impl<'a> Outgoing<'a> {
    /// None gathered yet, batches of `batch` bytes sent through `sink`.
    pub(super) fn new(sink: &'a mut dyn FnMut(Vec<Reply>) -> Result<()>, batch: usize) -> Outgoing<'a> {
        Outgoing { sink, batch, gathered: Vec::new() }
    }

    /// Sends the replies gathered and not sent ahead: the request's last.
    ///
    /// # Errors
    ///
    /// The sink's: the connection is gone.
    pub(super) fn finish(self) -> Result<()> {
        if self.gathered.is_empty() {
            return Ok(());
        }
        (self.sink)(self.gathered)
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
    /// What `session` holds, as it begins.
    pub(super) fn new(session: Session) -> Held {
        Held { session, statements: HashMap::new(), cursors: HashMap::new() }
    }

    /// Does `work`, its replies gathered in `out`, within `max_statements`
    /// prepared statements; the transaction ended after it if the request
    /// says so (`ended`).
    ///
    /// # Errors
    ///
    /// A write of replies failed: the connection is gone.
    pub(super) fn handle(&mut self, work: Work, out: &mut Outgoing<'_>, max_statements: usize) -> Result<()> {
        let ends = work.ends();
        let last = match self.done(work, out, max_statements) {
            Ok(Some(last)) => last,
            Ok(None) => return Ok(()),
            Err(Cut::Gone(e)) => return Err(e),
            Err(Cut::Failed(e)) => Reply::Error(Failure::from(&e)),
        };
        let last = if ends { self.ended(last) } else { last };
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
        Ok(())
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
            | Reply::Prepared { .. }
            | Reply::Description(_)
            | Reply::Rows(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. } => {
                return last;
            }
        };
        match outcome {
            Ok(()) => last,
            Err(e) => Reply::Error(Failure::from(&e)),
        }
    }

    /// Does `work`: the last reply, none if its rows ended it.
    fn done(&mut self, work: Work, out: &mut Outgoing<'_>, max_statements: usize) -> std::result::Result<Option<Reply>, Cut> {
        let failed = Cut::Failed;
        match work {
            Work::Query { sql, .. } => match self.session.direct(&sql).map_err(failed)? {
                Direct::Done(completion) => Ok(Some(Reply::Complete(completion.into()))),
                Direct::Rows(cursor) => {
                    let mut open = Open { cursor, next: None, warning: self.session.warning() };
                    describe(&open.cursor, out);
                    send_rows(&mut open, 0, out).map(|ended| Some(ended.reply()))
                }
            },
            Work::Prepare(n, sql) => self.prepare(n, &sql, max_statements).map(Some).map_err(failed),
            Work::Execute { statement, values, rows, .. } => self.execute(statement, &values, (rows, out)),
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
    /// cursor opened, at most `rows` of it sent, of the columns Prepared
    /// gave: no Description.
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

    /// Runs `work` in `held`, with batches too large for anything to be
    /// sent ahead: the replies gathered.
    fn run(held: &mut Held, work: Work) -> Vec<Reply> {
        let mut sink = |_: Vec<Reply>| Ok(());
        let mut out = Outgoing::new(&mut sink, 1 << 20);
        held.handle(work, &mut out, 8).unwrap();
        out.gathered
    }

    /// Whether `replies` is one completion, of a transaction `active` or
    /// not.
    fn completed(replies: &[Reply], active: bool) -> bool {
        matches!(replies, [Reply::Complete(Completed { sqlcode: 0, active: a, .. })] if *a == active)
    }

    /// The work that inserts `k`, the transaction ended with it if `ends`.
    fn insert(k: i32, ends: bool) -> Work {
        Work::Query { sql: format!("INSERT INTO T VALUES ({k})"), ends }
    }

    /// Verifies: [EXTENSION] [DESIGN] RULES.md D45: a request that ends the
    /// transaction with its statement commits it once the statement is
    /// done, or rolls it back if the statement failed, its one reply saying
    /// no transaction is active; one that does not leaves it active.
    #[test]
    fn a_request_that_ends_its_transaction_commits_or_rolls_back() {
        let mut held = held();
        assert!(completed(&run(&mut held, insert(1_i32, true)), false), "committed");
        assert!(held.session.transaction().is_none());
        assert!(completed(&run(&mut held, insert(2_i32, false)), true), "left active");
        let failed = run(&mut held, Work::Query { sql: "INSERT INTO T VALUES ('x')".into(), ends: true });
        assert!(matches!(failed.as_slice(), [Reply::Error(Failure { active: false, .. })]), "{failed:?}");
        assert!(held.session.transaction().is_none(), "rolled back");
        let counted = run(&mut held, Work::Query { sql: "SELECT COUNT(*) FROM T".into(), ends: true });
        assert!(matches!(&counted[1], Reply::Rows(rows) if rows[0][0].to_string() == "1"), "the second insert undone: {counted:?}");
        assert!(matches!(counted.as_slice(), [Reply::Description(_), Reply::Rows(_), Reply::Complete(Completed { active: false, .. })]));
    }

    /// Verifies: [EXTENSION] [DESIGN] RULES.md D45: a prepared query's
    /// execution sends its rows with no Description; its cursor left open,
    /// the transaction its request would end stays active, through the
    /// fetches after; one whose rows are all sent ends it.
    #[test]
    fn a_cursor_left_open_keeps_the_transaction_its_request_would_end() {
        let mut held = held();
        run(&mut held, Work::Prepare(1, "SELECT K FROM T".into()));
        run(&mut held, insert(3_i32, false));
        run(&mut held, insert(4_i32, false));
        let opened = run(&mut held, Work::Execute { statement: 1, values: Vec::new(), rows: 1, ends: true });
        assert!(matches!(opened.as_slice(), [Reply::Rows(_), Reply::Suspended]), "no Description: {opened:?}");
        assert!(held.session.transaction().is_some(), "its cursor open");
        let rest = run(&mut held, Work::Fetch(1, 0));
        assert!(matches!(rest.as_slice(), [Reply::Rows(_), Reply::Complete(Completed { active: true, .. })]), "{rest:?}");
        assert!(completed(&run(&mut held, Work::Execute { statement: 1, values: Vec::new(), rows: 0, ends: true })[1..], false));
    }
}
