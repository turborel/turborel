//! \[DESIGN\] A blocking connection to a server (RULES.md D42): one
//! statement at a time, its rows read whole, or given to a `Sink` a batch
//! at a time (`query_into`). It speaks the protocol over any stream that
//! reads and writes, so TCP, Unix sockets, named pipes on Windows and TLS
//! (`tls`, with the feature `tls`) all carry it.
//!
//! Every failure is a `Failure`, as the server would report it: the
//! server's own, or one of the connection (SQLCODE -919, SQLSTATE class
//! 08), after which the connection takes no more. How long it waits on
//! its server, its `Timeouts` say (`timeouts.rs`).

use std::io::{BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use turborel::error::{Code, SqlError, SqlState};
use turborel::storage::Row;

use turborel_protocol::{CancelKey, Capabilities, Column, Completed, Failure, HEADER, MAX_BODY, Reply, Request, VERSION, broken};

use crate::cancel::Canceller;
pub use crate::mechanism::{Certificate, Mechanism, Password, TlsChannel, Trust};
use crate::target::Target;
use crate::timeouts::{Bound, Limit, TERMINATE_WITHIN, Timeouts, io_failed, limit, tcp_to};
use crate::told::{Offer, Told};

/// What a client's errors are: what the server reports, as it reports it.
pub type Result<T> = std::result::Result<T, Failure>;

/// What a statement gave.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// A query's columns and rows.
    Rows {
        /// The columns.
        columns: Vec<Column>,
        /// The rows, each a value for each column.
        rows: Vec<Row>,
    },
    /// Any other statement's completion: its SQLCODE, 0 or 100, and its
    /// SQLSTATE, a warning's among them.
    Done(Completed),
}

/// What takes a query's rows as they come, a batch at a time.
pub trait Sink {
    /// The columns of the rows that follow.
    fn columns(&mut self, columns: &[Column]);
    /// A batch of rows.
    fn rows(&mut self, rows: &[Row]);
}

/// What a statement streamed into a sink gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Streamed {
    /// A query: how many rows went to the sink.
    Rows(u64),
    /// Any other statement's completion: its SQLCODE, 0 or 100, and its
    /// SQLSTATE, a warning's among them.
    Done(Completed),
}

/// A connection failure, as a `Failure`.
pub(crate) fn failed(msg: &str) -> Failure {
    Failure::from(&broken(msg))
}

/// \[DESIGN\] RULES.md D43: `mechanism` refused over plain TCP, before a
/// connection is made, if what it sends is secret (SQLSTATE 28000, as the
/// server refuses it); else nothing.
///
/// # Errors
///
/// SQLSTATE 28000 if `mechanism` is secret.
pub(crate) fn in_the_clear(mechanism: &dyn Mechanism) -> Result<()> {
    if !mechanism.secret() {
        return Ok(());
    }
    let msg = format!("[DESIGN] {} is sent only over TLS, a Unix socket or a named pipe, never over plain TCP", mechanism.name());
    Err(Failure::from(&SqlError::new(Code::Connection, msg).with_state(SqlState::INVALID_AUTHORIZATION)))
}

/// A connection over `S`.
pub struct Connection<S: Read + Write> {
    pub(crate) stream: BufReader<S>,
    /// The session's authorization identifier.
    pub(crate) authorization: String,
    /// The server's product and version, as Ready gave them.
    pub(crate) server: String,
    /// The longest body read: `MAX_BODY` unless `set_max_body` says.
    pub(crate) max_body: u32,
    /// How its stream's waits are bounded, if it was made here; none of a
    /// stream given to `open`, bounded by its maker.
    bounded: Option<Limit<S>>,
    /// How long it waits on its server, as it was made.
    timeouts: Timeouts,
    /// Set once the connection is lost: it takes no more.
    pub(crate) lost: Option<Failure>,
    /// What cancels its statements, as Ready gave it.
    pub(crate) key: Option<CancelKey>,
    /// How to reach its server again, to cancel, if known.
    reach: Option<Target>,
    /// The number its next prepared statement takes.
    pub(crate) next_statement: u32,
    /// Whether its session's transaction is active, as the server last
    /// said [ISO/IEC 9075:1992 §4.28].
    pub(crate) active: bool,
    /// The SQLSTATE of the warning the last statement completed with, if
    /// it did [ISO/IEC 9075:1992 §22.1].
    pub(crate) warning: Option<String>,
    /// Whether each statement is its own transaction (`set_autocommit`).
    pub(crate) autocommit: bool,
    /// What the server granted and told it beside replies (`told.rs`).
    pub(crate) told: Told,
}

impl<S: Read + Write> std::fmt::Debug for Connection<S> {
    /// Where it reached, as whom, and whether it is lost: never its
    /// cancel key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("target", &self.reach)
            .field("authorization", &self.authorization)
            .field("lost", &self.lost.is_some())
            .finish_non_exhaustive()
    }
}

/// Connects to the server at `address` as `user`, authenticated by
/// `mechanism`, waiting as `Timeouts::default` says: to connect and log
/// in, `CONNECT_WITHIN` each step; then without limit.
///
/// # Errors
///
/// As `Connection::open`; a connection failure if none can be made, in
/// time; SQLSTATE 28000, before any is made, for a secret mechanism, as
/// `Password`, over plain TCP.
pub fn connect(address: impl ToSocketAddrs, user: &str, mechanism: &mut dyn Mechanism) -> Result<Connection<TcpStream>> {
    in_the_clear(mechanism)?;
    let timeouts = Timeouts::default();
    let stream = tcp_to(address, timeouts.connect)?;
    stream.set_nodelay(true).map_err(|e| failed(&format!("[DESIGN] cannot set the connection up: {e}")))?;
    let server = stream.peer_addr().map_err(|e| failed(&format!("[DESIGN] cannot set the connection up: {e}")))?;
    Ok(Connection::open_timed(stream, (user, mechanism), timeouts)?.reached_by(Target::Tcp(server)))
}

/// Connects to the server at the Unix socket `path` as `user`,
/// authenticated by `mechanism`, waiting as `Timeouts::default` says.
///
/// # Errors
///
/// As `Connection::open`; a connection failure if none can be made.
#[cfg(unix)]
pub fn connect_local(
    path: &std::path::Path,
    user: &str,
    mechanism: &mut dyn Mechanism,
) -> Result<Connection<std::os::unix::net::UnixStream>> {
    let stream = std::os::unix::net::UnixStream::connect(path).map_err(|e| io_failed("connect", &e))?;
    let opened = Connection::open_timed(stream, (user, mechanism), Timeouts::default())?;
    Ok(opened.reached_by(Target::Local(path.to_path_buf())))
}

/// Connects to the server at the named pipe `path`, as `\\.\pipe\turborel`,
/// on Windows, as `user`, authenticated by `mechanism`, waiting as
/// `Timeouts::default` says to open it while it is busy; its reads and
/// writes, the login's too, are not bounded (`timeouts.rs`).
///
/// # Errors
///
/// As `Connection::open`; a connection failure if none can be made, in
/// time.
#[cfg(windows)]
pub fn connect_local(path: &std::path::Path, user: &str, mechanism: &mut dyn Mechanism) -> Result<Connection<std::fs::File>> {
    let timeouts = Timeouts::default();
    let pipe = crate::pipe::open(path, timeouts.connect)?;
    let opened = Connection::open_timed(pipe, (user, mechanism), timeouts)?;
    Ok(opened.reached_by(Target::Local(path.to_path_buf())))
}

impl<S: Read + Write> Connection<S> {
    /// Opens a connection over `stream` as `user`, authenticated by
    /// `mechanism`. Of a stream given, the caller answers for whether it
    /// keeps a secret mechanism's bytes, as a password, private: `connect`,
    /// `connect_local`, `target::connect_to` and `tls::connect` know their
    /// transport, and refuse one over plain TCP.
    ///
    /// # Errors
    ///
    /// The server's refusal: SQLSTATE 28000 for credentials refused, 08004
    /// for a connection refused; a connection failure.
    pub fn open(stream: S, user: &str, mechanism: &mut dyn Mechanism) -> Result<Connection<S>> {
        Connection::open_offering(stream, (user, mechanism), Offer::default())
    }

    /// Opens a connection over `stream` as `open` does, asking for what
    /// `offer` asks (RULES.md D185): the server grants what it serves of
    /// it, as `granted` then says.
    ///
    /// # Errors
    ///
    /// As `open`'s.
    pub fn open_offering(stream: S, (user, mechanism): (&str, &mut dyn Mechanism), offer: Offer) -> Result<Connection<S>> {
        let mut connection = Connection::unopened(BufReader::new(stream));
        let hello = Request::Hello {
            version: VERSION,
            user: user.to_owned(),
            mechanism: mechanism.name().to_owned(),
            initial: mechanism.initial(),
            capabilities: offer.capabilities,
            attributes: offer.attributes,
        };
        connection.send(&hello)?;
        connection.logged_in(mechanism, offer.compress_from)
    }

    /// A connection over `stream` that takes nothing, lost as it is made:
    /// what stands in a `Pooled` for the connection it gives back
    /// (`pool.rs`).
    pub(crate) fn stand_in(stream: S) -> Connection<S> {
        let mut connection = Connection::unopened(BufReader::with_capacity(0, stream));
        connection.lost = Some(failed("[DESIGN] the connection went back to its pool"));
        connection
    }

    /// A connection over `stream`, not yet logged in.
    fn unopened(stream: BufReader<S>) -> Connection<S> {
        Connection {
            stream,
            authorization: String::new(),
            server: String::new(),
            max_body: MAX_BODY,
            bounded: None,
            timeouts: Timeouts { connect: None, read: None },
            lost: None,
            key: None,
            reach: None,
            next_statement: 0,
            active: false,
            warning: None,
            autocommit: false,
            told: Told::default(),
        }
    }

    /// It, logged in by `mechanism` once Hello is sent: each challenge
    /// answered until Ready, requests of at least `compress_from` bytes
    /// compressed from then on if compression was granted.
    fn logged_in(mut self, mechanism: &mut dyn Mechanism, compress_from: usize) -> Result<Connection<S>> {
        loop {
            match self.receive()? {
                Reply::Challenge(challenge) => {
                    let answer = mechanism.answer(&challenge)?;
                    self.send(&Request::Answer(answer))?;
                }
                Reply::Ready { authorization, key, capabilities, server } => {
                    mechanism.ready()?;
                    self.ready((authorization, key, &capabilities, server), compress_from);
                    return Ok(self);
                }
                Reply::Error(failure) => {
                    mechanism.refused();
                    return Err(failure);
                }
                Reply::Prepared { .. }
                | Reply::Description(_)
                | Reply::Rows(_)
                | Reply::Suspended
                | Reply::Complete(_)
                | Reply::Lob(_)
                | Reply::Notice(_)
                | Reply::Status { .. }
                | Reply::Pong => {
                    return Err(self.lose("[DESIGN] the server sent no Ready"));
                }
            }
        }
    }

    /// Takes Ready's `authorization`, cancel key, capabilities granted and
    /// server: compressing requests of at least `compress_from` bytes if
    /// compression is among them.
    pub(crate) fn ready(&mut self, (authorization, key, granted, server): (String, CancelKey, &[String], String), compress_from: usize) {
        (self.authorization, self.server, self.key) = (authorization, server, Some(key));
        self.told.granted = Capabilities::of(granted);
        self.told.compress = self.told.granted.has(turborel_protocol::capability::COMPRESSION).then_some(compress_from);
    }

    /// Opens a connection over `stream`, as `open`, its waits bounded on
    /// the stream itself as `timeouts` say: each step of the login by
    /// `connect`, each after by `read`.
    ///
    /// # Errors
    ///
    /// As `open`'s; a connection failure if a wait runs out.
    pub(crate) fn open_timed(stream: S, login: (&str, &mut dyn Mechanism), timeouts: Timeouts) -> Result<Connection<S>>
    where
        S: Bound,
    {
        Connection::open_timed_offering(stream, login, (timeouts, Offer::default()))
    }

    /// Opens a connection over `stream` as `open_timed` does, asking for
    /// what `offer` asks, as `open_offering`.
    ///
    /// # Errors
    ///
    /// As `open_timed`'s.
    pub(crate) fn open_timed_offering(
        stream: S,
        login: (&str, &mut dyn Mechanism),
        (timeouts, offer): (Timeouts, Offer),
    ) -> Result<Connection<S>>
    where
        S: Bound,
    {
        limit(&stream, timeouts.connect)?;
        let mut connection = Connection::open_offering(stream, login, offer)?;
        limit(connection.stream.get_ref(), timeouts.read)?;
        connection.bounded = Some(limit::<S>);
        connection.timeouts = timeouts;
        Ok(connection)
    }

    /// How long each read or write of the server waits from now on, as
    /// `Timeouts::read`; none, no limit.
    ///
    /// # Errors
    ///
    /// A connection failure if the system cannot set it, or the connection
    /// was made over a stream given to `open`, whose maker bounds it.
    pub fn set_read_timeout(&mut self, within: Option<Duration>) -> Result<()> {
        let Some(bounded) = self.bounded else { return Err(failed("[DESIGN] a stream given to open is bounded by its maker")) };
        bounded(self.stream.get_ref(), within)?;
        self.timeouts.read = within;
        Ok(())
    }

    /// How long each read or write of the server waits, as the connection
    /// was made or `set_read_timeout` last said; none, no limit.
    pub fn read_timeout(&self) -> Option<Duration> {
        self.timeouts.read
    }

    /// Takes replies of at most `bytes` bytes from now on: `MAX_BODY`, the
    /// protocol's longest, unless this says. A longer one loses the
    /// connection (SQLSTATE 08006).
    pub fn set_max_body(&mut self, bytes: u32) {
        self.max_body = bytes;
    }

    /// The session's authorization identifier, as the server authenticated
    /// it.
    pub fn authorization(&self) -> &str {
        &self.authorization
    }

    /// The server's product, version and codename, as "turborel 0.0.0
    /// (alpha)".
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Whether the session's transaction is active, as the server said
    /// after the last statement [ISO/IEC 9075:1992 §4.28]: so a client's
    /// connection statements may refuse to leave it [§15.1 GR1, §15.3 GR6].
    pub fn in_transaction(&self) -> bool {
        self.active
    }

    /// \[EXTENSION\] Whether each statement `query`, `query_into` and
    /// `execute` run is its own transaction from now on, if `on`: the
    /// server commits it once the statement succeeds, or rolls it back
    /// if it fails, within the request, so no COMMIT need be sent
    /// (RULES.md D45). Off as a connection opens: a transaction begins
    /// with the first statement and lasts until COMMIT or ROLLBACK
    /// [ISO/IEC 9075:1992 §4.28]. A cursor `open_cursor` opens is read
    /// within its transaction, which ends as the cursor does if the server
    /// granted `fetch-ends` (RULES.md D199).
    pub fn set_autocommit(&mut self, on: bool) {
        self.autocommit = on;
    }

    /// Whether each statement is its own transaction (`set_autocommit`).
    pub fn autocommit(&self) -> bool {
        self.autocommit
    }

    /// The SQLSTATE of the warning the last statement completed with, if it
    /// did [ISO/IEC 9075:1992 §22.1, class 01], as its Complete said: of a
    /// query too, once all its rows were read, its outcome being its rows.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    /// How to reach the server again: so a `Canceller` connects to it.
    pub(crate) fn reached_by(mut self, reach: Target) -> Self {
        self.reach = Some(reach);
        self
    }

    /// Where the connection reached its server, and how: none for a
    /// connection made over a stream given to `open`. A `Target::Tls` is
    /// a connection TLS secures.
    pub fn target(&self) -> Option<&Target> {
        self.reach.as_ref()
    }

    /// What cancels the statement this connection runs, from another
    /// thread: none for a connection made over a stream given to `open`,
    /// whose server it cannot reach again.
    pub fn canceller(&self) -> Option<Canceller> {
        Some(Canceller { target: self.reach.clone()?, key: self.key? })
    }

    /// Runs `sql`, a `<direct SQL statement>` [ISO/IEC 9075:1992 §20.1], in
    /// the connection's session: a query's rows, or another statement's
    /// completion; the transaction ended with it under `set_autocommit`.
    ///
    /// # Errors
    ///
    /// The statement's, as the server reports it; a connection failure.
    pub fn query(&mut self, sql: &str) -> Result<Outcome> {
        self.send(&Request::Query { sql: sql.to_owned(), ends: self.autocommit, attributes: self.told.attributes(), multi: false })?;
        self.query_reply()
    }

    /// The reply to a Query sent: a query's rows, or another statement's
    /// completion.
    ///
    /// # Errors
    ///
    /// As `query`'s.
    pub(crate) fn query_reply(&mut self) -> Result<Outcome> {
        let columns = match self.receive()? {
            Reply::Complete(completed) => return Ok(Outcome::Done(completed)),
            Reply::Description(columns) => columns,
            Reply::Error(failure) => return Err(self.failed(failure)),
            Reply::Lob(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Rows(_)
            | Reply::Suspended
            | Reply::Notice(_)
            | Reply::Status { .. }
            | Reply::Pong => {
                return Err(self.lose("[DESIGN] the server sent no result"));
            }
        };
        let (rows, _) = self.rows()?;
        Ok(Outcome::Rows { columns, rows })
    }

    /// Runs `sql` as `query` does, a query's columns and rows given to
    /// `sink` as they come, so no more than a batch is held.
    ///
    /// # Errors
    ///
    /// As `query`'s; a query's failure after some rows went to `sink`.
    pub fn query_into(&mut self, sql: &str, sink: &mut dyn Sink) -> Result<Streamed> {
        self.send(&Request::Query { sql: sql.to_owned(), ends: self.autocommit, attributes: self.told.attributes(), multi: false })?;
        match self.receive()? {
            Reply::Complete(completed) => return Ok(Streamed::Done(completed)),
            Reply::Description(columns) => sink.columns(&columns),
            Reply::Error(failure) => return Err(self.failed(failure)),
            Reply::Lob(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Rows(_)
            | Reply::Suspended
            | Reply::Notice(_)
            | Reply::Status { .. }
            | Reply::Pong => {
                return Err(self.lose("[DESIGN] the server sent no result"));
            }
        }
        let mut count = 0_u64;
        loop {
            match self.receive()? {
                Reply::Rows(batch) => {
                    count = count.saturating_add(u64::try_from(batch.len()).unwrap_or(u64::MAX));
                    sink.rows(&batch);
                }
                Reply::Complete(_) => return Ok(Streamed::Rows(count)),
                Reply::Error(failure) => return Err(self.failed(failure)),
                Reply::Lob(_)
                | Reply::Challenge(_)
                | Reply::Ready { .. }
                | Reply::Prepared { .. }
                | Reply::Description(_)
                | Reply::Suspended
                | Reply::Notice(_)
                | Reply::Status { .. }
                | Reply::Pong => {
                    return Err(self.lose("[DESIGN] the server broke off its rows"));
                }
            }
        }
    }

    /// The rows the server sends, until it says whether the cursor has
    /// more: the rows, and whether it has.
    pub(crate) fn rows(&mut self) -> Result<(Vec<Row>, bool)> {
        let mut rows = Vec::new();
        loop {
            match self.receive()? {
                Reply::Rows(batch) => rows.extend(batch),
                Reply::Complete(_) => return Ok((rows, false)),
                Reply::Suspended => return Ok((rows, true)),
                Reply::Error(failure) => return Err(self.failed(failure)),
                Reply::Lob(_)
                | Reply::Challenge(_)
                | Reply::Ready { .. }
                | Reply::Prepared { .. }
                | Reply::Description(_)
                | Reply::Notice(_)
                | Reply::Status { .. }
                | Reply::Pong => {
                    return Err(self.lose("[DESIGN] the server broke off its rows"));
                }
            }
        }
    }

    /// Ends the connection, once the server has ended its session as
    /// SESSION.COMMIT_AT_END says and closed it: waiting for it at most as its read
    /// timeout says, else `TERMINATE_WITHIN`, and taking at most a reply's
    /// bytes.
    ///
    /// # Errors
    ///
    /// A connection failure, if Terminate cannot be sent, or the server
    /// does not close in time; the server's error, if it sends one rather
    /// than close: a commit that fails as SESSION.COMMIT_AT_END ends the session,
    /// its transaction rolled back (SQLSTATE 40002 for a deferred
    /// constraint not satisfied).
    pub fn terminate(mut self) -> Result<()> {
        self.send(&Request::Terminate)?;
        if let Some(bounded) = self.bounded {
            bounded(self.stream.get_ref(), Some(self.timeouts.read.unwrap_or(TERMINATE_WITHIN)))?;
        }
        let most = u64::from(self.max_body).saturating_add(u64::try_from(HEADER).unwrap_or(u64::MAX));
        let mut rest = Vec::new();
        self.stream.by_ref().take(most).read_to_end(&mut rest).map_err(|e| io_failed("receive", &e))?;
        let Some((head, body)) = rest.split_first_chunk::<HEADER>() else { return Ok(()) };
        let (kind, _) = turborel_protocol::header(*head, self.max_body).map_err(|e| Failure::from(&e))?;
        match Reply::decode(kind, body).map_err(|e| Failure::from(&e))? {
            Reply::Error(failure) => Err(failure),
            // A notice or a status comes before the session's end is told.
            Reply::Notice(_) | Reply::Status { .. } | Reply::Pong => Ok(()),
            Reply::Lob(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Description(_)
            | Reply::Rows(_)
            | Reply::Suspended
            | Reply::Complete(_) => Err(failed("[DESIGN] the server answered Terminate")),
        }
    }

    /// Whether the connection is lost: it takes no more.
    pub fn is_lost(&self) -> bool {
        self.lost.is_some()
    }

    /// `failure`, the connection lost if it ends it.
    pub(crate) fn failed(&mut self, failure: Failure) -> Failure {
        if failure.ends_connection() {
            self.lost = Some(failure.clone());
        }
        failure
    }

    /// Loses the connection, for a connection failure `msg`.
    pub(crate) fn lose(&mut self, msg: &str) -> Failure {
        let failure = failed(msg);
        self.lost = Some(failure.clone());
        failure
    }
}
