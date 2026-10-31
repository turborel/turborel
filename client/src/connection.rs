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

use turborel_protocol::{CancelKey, Column, Completed, Failure, HEADER, MAX_BODY, Reply, Request, VERSION, broken};

use crate::cancel::Canceller;
use crate::target::Target;
use crate::timeouts::{Socket, TERMINATE_WITHIN, Timeouts, io_failed, tcp_to};

/// \[DESIGN\] The most of a reply's body reserved before its bytes
/// arrive; past it, the buffer grows as they do.
const RESERVED: usize = 64 * 1024;

/// What a client's errors are: what the server reports, as it reports it.
pub type Result<T> = std::result::Result<T, Failure>;

/// \[DESIGN\] A client's side of a mechanism of authentication: its name,
/// its first bytes, and its answer to each challenge. The server's
/// `Authenticator` of the same name is the other side.
pub trait Mechanism {
    /// The mechanism's name, as the server knows it.
    fn name(&self) -> &str;
    /// The bytes sent with Hello.
    fn initial(&mut self) -> Vec<u8>;
    /// The answer to the server's challenge `challenge`.
    ///
    /// # Errors
    ///
    /// If it has none: the connection is then given up.
    fn answer(&mut self, challenge: &[u8]) -> Result<Vec<u8>>;
    /// Whether what it sends gives away what proves the user, as a
    /// password does, or lets one who overhears it guess that at leisure,
    /// as SCRAM's proof does: if so, a connection over plain TCP refuses it
    /// before anything is sent (`connect`, `target::connect_to`). None does
    /// by default.
    fn secret(&self) -> bool {
        false
    }
    /// Whether the exchange may end as the server sends Ready: a mechanism
    /// by which the server proves itself too, as SCRAM's, refuses one that
    /// did not.
    ///
    /// # Errors
    ///
    /// If the server has not proved itself: the connection is then given
    /// up. None does by default.
    fn ready(&self) -> Result<()> {
        Ok(())
    }
}

/// \[DESIGN\] The client's side of `trust`: nothing to prove, so a server
/// takes it only from a client on its own machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct Trust;

/// The name of the mechanism `Trust` is a side of.
pub const TRUST: &str = "trust";

impl Mechanism for Trust {
    fn name(&self) -> &str {
        TRUST
    }
    fn initial(&mut self) -> Vec<u8> {
        Vec::new()
    }
    fn answer(&mut self, _: &[u8]) -> Result<Vec<u8>> {
        Err(failed("[DESIGN] trust has no answer to a challenge"))
    }
}

/// \[DESIGN\] The client's side of `password`: the password, sent whole,
/// so only over TLS, a Unix socket or a named pipe. A connection over
/// plain TCP refuses it before it sends anything, as the server would
/// after.
#[derive(Clone)]
pub struct Password(pub String);

/// The name of the mechanism `Password` is a side of.
pub const PASSWORD: &str = "password";

impl std::fmt::Debug for Password {
    /// Never the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Password(..)")
    }
}

impl Mechanism for Password {
    fn name(&self) -> &str {
        PASSWORD
    }
    fn initial(&mut self) -> Vec<u8> {
        self.0.as_bytes().to_vec()
    }
    fn answer(&mut self, _: &[u8]) -> Result<Vec<u8>> {
        Err(failed("[DESIGN] password has no answer to a challenge"))
    }
    fn secret(&self) -> bool {
        true
    }
}

/// \[DESIGN\] The client's side of `certificate`: nothing sent, the
/// certificate presented over TLS (`tls::presenting`, with the feature
/// `tls`) proving the user its common name names.
#[derive(Debug, Clone, Copy, Default)]
pub struct Certificate;

/// The name of the mechanism `Certificate` is a side of.
pub const CERTIFICATE: &str = "certificate";

impl Mechanism for Certificate {
    fn name(&self) -> &str {
        CERTIFICATE
    }
    fn initial(&mut self) -> Vec<u8> {
        Vec::new()
    }
    fn answer(&mut self, _: &[u8]) -> Result<Vec<u8>> {
        Err(failed("[DESIGN] certificate has no answer to a challenge"))
    }
}

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
    stream: BufReader<S>,
    /// The session's authorization identifier.
    authorization: String,
    /// The server's product and version, as Ready gave them.
    server: String,
    /// The longest body read: `MAX_BODY` unless `set_max_body` says.
    max_body: u32,
    /// A handle on its socket, to bound its waits, if it was made here.
    socket: Option<Socket>,
    /// How long it waits on its server, as it was made.
    timeouts: Timeouts,
    /// Set once the connection is lost: it takes no more.
    lost: Option<Failure>,
    /// What cancels its statements, as Ready gave it.
    key: Option<CancelKey>,
    /// How to reach its server again, to cancel, if known.
    reach: Option<Target>,
    /// The number its next prepared statement takes.
    pub(crate) next_statement: u32,
    /// Whether its session's transaction is active, as the server last
    /// said [ISO/IEC 9075:1992 §4.28].
    active: bool,
    /// The SQLSTATE of the warning the last statement completed with, if
    /// it did [ISO/IEC 9075:1992 §22.1].
    warning: Option<String>,
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
    let socket = Socket::tcp(&stream)?;
    Ok(Connection::open_timed(stream, socket, (user, mechanism), timeouts)?.reached_by(Target::Tcp(server)))
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
    let socket = Socket::local(&stream)?;
    let opened = Connection::open_timed(stream, socket, (user, mechanism), Timeouts::default())?;
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
    let opened = Connection::open_timed(pipe, Socket::Pipe, (user, mechanism), timeouts)?;
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
        let mut connection = Connection {
            stream: BufReader::new(stream),
            authorization: String::new(),
            server: String::new(),
            max_body: MAX_BODY,
            socket: None,
            timeouts: Timeouts { connect: None, read: None },
            lost: None,
            key: None,
            reach: None,
            next_statement: 0,
            active: false,
            warning: None,
        };
        let hello = Request::Hello {
            version: VERSION,
            user: user.to_owned(),
            mechanism: mechanism.name().to_owned(),
            initial: mechanism.initial(),
            capabilities: Vec::new(),
        };
        connection.send(&hello)?;
        loop {
            match connection.receive()? {
                Reply::Challenge(challenge) => {
                    let answer = mechanism.answer(&challenge)?;
                    connection.send(&Request::Answer(answer))?;
                }
                Reply::Ready { authorization, key, capabilities: _, server } => {
                    mechanism.ready()?;
                    connection.authorization = authorization;
                    connection.server = server;
                    connection.key = Some(key);
                    return Ok(connection);
                }
                Reply::Error(failure) => return Err(failure),
                Reply::Prepared { .. } | Reply::Description(_) | Reply::Rows(_) | Reply::Suspended | Reply::Complete(_) => {
                    return Err(connection.lose("[DESIGN] the server sent no Ready"));
                }
            }
        }
    }

    /// Opens a connection over `stream`, as `open`, its waits bounded
    /// through `socket`, a handle on its socket, as `timeouts` say: each
    /// step of the login by `connect`, each after by `read`.
    ///
    /// # Errors
    ///
    /// As `open`'s; a connection failure if a wait runs out.
    pub(crate) fn open_timed(
        stream: S,
        socket: Socket,
        (user, mechanism): (&str, &mut dyn Mechanism),
        timeouts: Timeouts,
    ) -> Result<Connection<S>> {
        socket.limit(timeouts.connect)?;
        let mut connection = Connection::open(stream, user, mechanism)?;
        socket.limit(timeouts.read)?;
        connection.socket = Some(socket);
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
        let Some(socket) = &self.socket else { return Err(failed("[DESIGN] a stream given to open is bounded by its maker")) };
        socket.limit(within)?;
        self.timeouts.read = within;
        Ok(())
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

    /// What cancels the statement this connection runs, from another
    /// thread: none for a connection made over a stream given to `open`,
    /// whose server it cannot reach again.
    pub fn canceller(&self) -> Option<Canceller> {
        Some(Canceller { target: self.reach.clone()?, key: self.key? })
    }

    /// Runs `sql`, a `<direct SQL statement>` [ISO/IEC 9075:1992 §20.1], in
    /// the connection's session: a query's rows, or another statement's
    /// completion.
    ///
    /// # Errors
    ///
    /// The statement's, as the server reports it; a connection failure.
    pub fn query(&mut self, sql: &str) -> Result<Outcome> {
        self.send(&Request::Query(sql.to_owned()))?;
        let columns = match self.receive()? {
            Reply::Complete(completed) => return Ok(Outcome::Done(completed)),
            Reply::Description(columns) => columns,
            Reply::Error(failure) => return Err(self.failed(failure)),
            Reply::Challenge(_) | Reply::Ready { .. } | Reply::Prepared { .. } | Reply::Rows(_) | Reply::Suspended => {
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
        self.send(&Request::Query(sql.to_owned()))?;
        match self.receive()? {
            Reply::Complete(completed) => return Ok(Streamed::Done(completed)),
            Reply::Description(columns) => sink.columns(&columns),
            Reply::Error(failure) => return Err(self.failed(failure)),
            Reply::Challenge(_) | Reply::Ready { .. } | Reply::Prepared { .. } | Reply::Rows(_) | Reply::Suspended => {
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
                Reply::Challenge(_) | Reply::Ready { .. } | Reply::Prepared { .. } | Reply::Description(_) | Reply::Suspended => {
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
                Reply::Challenge(_) | Reply::Ready { .. } | Reply::Prepared { .. } | Reply::Description(_) => {
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
        if let Some(socket) = &self.socket {
            socket.limit(Some(self.timeouts.read.unwrap_or(TERMINATE_WITHIN)))?;
        }
        let most = u64::from(self.max_body).saturating_add(u64::try_from(HEADER).unwrap_or(u64::MAX));
        let mut rest = Vec::new();
        self.stream.by_ref().take(most).read_to_end(&mut rest).map_err(|e| io_failed("receive", &e))?;
        let Some((head, body)) = rest.split_first_chunk::<HEADER>() else { return Ok(()) };
        let (kind, _) = turborel_protocol::header(*head, self.max_body).map_err(|e| Failure::from(&e))?;
        match Reply::decode(kind, body).map_err(|e| Failure::from(&e))? {
            Reply::Error(failure) => Err(failure),
            Reply::Challenge(_)
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

    pub(crate) fn send(&mut self, request: &Request) -> Result<()> {
        if let Some(lost) = &self.lost {
            return Err(lost.clone());
        }
        let mut bytes = Vec::new();
        request.encode(&mut bytes);
        let stream = self.stream.get_mut();
        let sent = stream.write_all(&bytes).and_then(|()| stream.flush());
        sent.map_err(|e| self.lost_by(io_failed("send", &e)))
    }

    /// Loses the connection, for `failure`.
    fn lost_by(&mut self, failure: Failure) -> Failure {
        self.lost = Some(failure.clone());
        failure
    }

    pub(crate) fn receive(&mut self) -> Result<Reply> {
        let mut head = [0_u8; HEADER];
        if let Err(e) = self.stream.read_exact(&mut head) {
            return Err(self.lost_by(io_failed("receive", &e)));
        }
        let (kind, length) = turborel_protocol::header(head, self.max_body).map_err(|e| self.failed(Failure::from(&e)))?;
        // Read as its bytes arrive: a header alone reserves little.
        let mut body = Vec::with_capacity(length.min(RESERVED));
        let wanted = u64::try_from(length).unwrap_or(u64::MAX);
        if let Err(e) = self.stream.by_ref().take(wanted).read_to_end(&mut body) {
            return Err(self.lost_by(io_failed("receive", &e)));
        }
        if body.len() != length {
            return Err(self.lose("[DESIGN] cannot receive: the connection closed within a reply"));
        }
        let reply = Reply::decode(kind, &body).map_err(|e| self.failed(Failure::from(&e)))?;
        // A query's rows are read in a transaction; a statement's end says
        // whether it goes on, and whether it warned.
        match &reply {
            Reply::Complete(c) => (self.active, self.warning) = (c.active, c.is_warning().then(|| c.sqlstate.clone())),
            Reply::Error(f) => (self.active, self.warning) = (f.active, None),
            Reply::Description(_) => (self.active, self.warning) = (true, None),
            Reply::Rows(_) | Reply::Suspended => self.active = true,
            Reply::Challenge(_) | Reply::Ready { .. } | Reply::Prepared { .. } => {}
        }
        Ok(reply)
    }
}
