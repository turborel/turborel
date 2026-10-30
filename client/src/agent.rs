//! [ISO/IEC 9075:1992 §15] An SQL-agent: a client's connections to
//! servers, each named, one current, and the statements that make, choose
//! and end them, CONNECT, SET CONNECTION and DISCONNECT, which it runs
//! itself; every other statement goes to the current connection's
//! session. A `Connector` makes a connection to a server an address names,
//! as the user a statement names, proved as the program says (RULES.md
//! D42, D46).

use turborel::ast::{ConnectionStatement, Disconnected, Literal, ValueSpecification};
use turborel::error::{Code, SqlError, SqlState};
use turborel::lexer::authorization_identifier;
use turborel::parser::Parser;

use crate::Canceller;
use crate::connection::{Connection, Outcome, Result, Sink, Streamed};
use crate::target::Duplex;
use turborel_protocol::{Completed, Failure};

/// A connection over any transport.
pub type Connected = Connection<Box<dyn Duplex>>;

/// What makes an agent's connections.
pub trait Connector {
    /// [ISO/IEC 9075:1992 §15.1 GR8] A connection to the server `server`
    /// names, an address, or for none the default server; as `user`, or
    /// for none the user the connector takes \[SR1\].
    ///
    /// # Errors
    ///
    /// The server cannot be reached, or refuses the connection.
    fn connect(&mut self, server: Option<&str>, user: Option<&str>) -> Result<Connected>;
}

/// A connection of an agent: its name, none for the default connection.
struct Held {
    name: Option<String>,
    connection: Connected,
}

/// [ISO/IEC 9075:1992 §15] An SQL-agent's connections.
pub struct Agent<C: Connector> {
    connector: C,
    held: Vec<Held>,
    /// The current connection, if any [§15.3 GR9].
    current: Option<usize>,
}

/// `state`'s connection exception, of `msg`.
fn exception(state: SqlState, msg: &str) -> Failure {
    let code = match state.as_str().get(..2) {
        Some("28") => Code::Privilege,
        Some("0A") => Code::Unsupported,
        Some(_) | None => Code::Connection,
    };
    Failure::from(&SqlError::new(code, msg).with_state(state))
}

/// The character string `v` is, a `<simple value specification>` of a
/// connection statement, its spaces trimmed [§15.1 GR2, GR6]: a literal
/// here, there being no parameters.
fn text(v: &ValueSpecification) -> Result<String> {
    if let ValueSpecification::Literal(Literal::Character(s)) = v {
        return Ok(s.trim_matches(' ').to_owned());
    }
    Err(Failure::from(&SqlError::new(Code::SyntaxRule, "[DESIGN] a client's connection statement names by character string literals")))
}

/// `s` as an `<identifier>`, if it is one [§15.1 GR3, GR6]: a regular
/// identifier in upper case, or a delimited one's body.
fn identifier(s: &str) -> Option<String> {
    authorization_identifier(s).ok()
}

/// SQLCODE 0, of `state`.
fn done(state: SqlState) -> Completed {
    Completed { sqlcode: 0, sqlstate: state.as_str().to_owned(), active: false }
}

impl<C: Connector> Agent<C> {
    /// An agent with no connection.
    pub fn new(connector: C) -> Agent<C> {
        Agent { connector, held: Vec::new(), current: None }
    }

    /// An agent whose default connection, current, is `connection`: as a
    /// client begins, connected.
    pub fn connected(connector: C, connection: Connected) -> Agent<C> {
        Agent { connector, held: vec![Held { name: None, connection }], current: Some(0) }
    }

    /// The current connection.
    ///
    /// # Errors
    ///
    /// SQLSTATE 08003 if there is none.
    pub fn current(&mut self) -> Result<&mut Connected> {
        let index = self.current.ok_or_else(|| {
            exception(SqlState::CONNECTION_DOES_NOT_EXIST, "[ISO/IEC 9075:1992 §15.3 GR9] there is no current connection")
        })?;
        self.held
            .get_mut(index)
            .map(|h| &mut h.connection)
            .ok_or_else(|| exception(SqlState::CONNECTION_DOES_NOT_EXIST, "[DESIGN] the current connection is gone"))
    }

    /// What cancels the current connection's statement, if any.
    pub fn canceller(&self) -> Option<Canceller> {
        self.held.get(self.current?)?.connection.canceller()
    }

    /// The SQLSTATE of the warning the current connection's last statement
    /// completed with, if it did (`Connection::warning`).
    pub fn warning(&self) -> Option<&str> {
        self.held.get(self.current?)?.connection.warning()
    }

    /// Whether the current connection is lost: none is not.
    pub fn is_lost(&self) -> bool {
        self.current.and_then(|i| self.held.get(i)).is_some_and(|h| h.connection.is_lost())
    }

    /// The current connection's name, none for the default one or for no
    /// connection.
    pub fn current_name(&self) -> Option<&str> {
        self.held.get(self.current?)?.name.as_deref()
    }

    /// Runs `sql`: a connection statement itself, its completion; any
    /// other in the current connection's session, as `Connection::query`.
    ///
    /// # Errors
    ///
    /// A connection statement's, or the statement's.
    pub fn query(&mut self, sql: &str) -> Result<Outcome> {
        if let Some(done) = self.connection_statement(sql)? {
            return Ok(Outcome::Done(done));
        }
        self.current()?.query(sql)
    }

    /// Runs `sql` as `query` does, a query's rows given to `sink`, as
    /// `Connection::query_into`.
    ///
    /// # Errors
    ///
    /// As `query`'s.
    pub fn query_into(&mut self, sql: &str, sink: &mut dyn Sink) -> Result<Streamed> {
        if let Some(done) = self.connection_statement(sql)? {
            return Ok(Streamed::Done(done));
        }
        self.current()?.query_into(sql, sink)
    }

    /// [ISO/IEC 9075:1992 §15] `sql`, if a connection statement, run: its
    /// completion; none if it is another.
    ///
    /// # Errors
    ///
    /// The statement's: as each rule says.
    pub fn connection_statement(&mut self, sql: &str) -> Result<Option<Completed>> {
        let Some(statement) = Parser::connection_statement(sql).map_err(|e| Failure::from(&e))? else { return Ok(None) };
        match statement {
            ConnectionStatement::Connect { server, name, user } => self.connect(Some(&server), name.as_ref(), user.as_ref()).map(Some),
            ConnectionStatement::ConnectDefault => self.connect(None, None, None).map(Some),
            ConnectionStatement::Set(name) => self.set_connection(name.as_ref()).map(Some),
            ConnectionStatement::Disconnect(object) => self.disconnect(&object).map(Some),
        }
    }

    /// [§15.1 GR1, §15.2 GR1] Refuses to leave the current connection's
    /// active transaction: there are no transactions of more than one
    /// server.
    fn not_in_transaction(&self) -> Result<()> {
        let active = self.current.and_then(|i| self.held.get(i)).is_some_and(|h| h.connection.in_transaction());
        if active {
            return Err(exception(
                SqlState::MULTIPLE_SERVER_TRANSACTIONS,
                "[ISO/IEC 9075:1992 §15.1 GR1, §15.2 GR1] the current connection's transaction is active, and none spans servers",
            ));
        }
        Ok(())
    }

    /// The connection named `name`, none for the default one, if held.
    fn find(&self, name: Option<&str>) -> Option<usize> {
        self.held.iter().position(|h| h.name.as_deref() == name)
    }

    /// [ISO/IEC 9075:1992 §15.1] CONNECT TO `server` \[AS `name`\] \[USER
    /// `user`\], or for no server TO DEFAULT: the connection made current,
    /// the one current before dormant \[GR9\].
    ///
    /// # Errors
    ///
    /// SQLSTATE 0A001 in an active transaction \[GR1\]; 28000 if the user is
    /// no `<authorization identifier>` \[GR3\]; 2E000 if the name is no
    /// `<identifier>` \[GR6\]; 08002 if it is in use \[GR7\]; 08001 if the
    /// server cannot be reached \[GR10\]; 08004 if it refuses \[GR11\].
    fn connect(
        &mut self,
        server: Option<&ValueSpecification>,
        name: Option<&ValueSpecification>,
        user: Option<&ValueSpecification>,
    ) -> Result<Completed> {
        self.not_in_transaction()?;
        let user = user.map(text).transpose()?;
        if let Some(u) = &user
            && identifier(u).is_none()
        {
            return Err(exception(
                SqlState::INVALID_AUTHORIZATION,
                &format!("[ISO/IEC 9075:1992 §15.1 GR3] '{u}' is not an <authorization identifier>"),
            ));
        }
        let address = server.map(text).transpose()?;
        let named = match (name, &address) {
            (Some(n), _) => Some(text(n)?),
            (None, Some(a)) => Some(a.clone()),
            (None, None) => None,
        };
        let identified = named
            .map(|n| {
                identifier(&n).ok_or_else(|| {
                    exception(SqlState::INVALID_CONNECTION_NAME, &format!("[ISO/IEC 9075:1992 §15.1 GR6] '{n}' is not an <identifier>"))
                })
            })
            .transpose()?;
        if self.find(identified.as_deref()).is_some() {
            let what = identified.as_deref().map_or_else(|| "the default connection".to_owned(), |n| format!("the connection {n}"));
            return Err(exception(SqlState::CONNECTION_NAME_IN_USE, &format!("[ISO/IEC 9075:1992 §15.1 GR7] {what} is established")));
        }
        let connection = self.connector.connect(address.as_deref(), user.as_deref()).map_err(|failure| {
            let state = if failure.sqlstate == SqlState::CONNECTION_FAILURE.as_str() {
                SqlState::UNABLE_TO_CONNECT
            } else {
                SqlState::CONNECTION_REJECTED
            };
            exception(state, &format!("[ISO/IEC 9075:1992 §15.1 GR10, GR11] {}", failure.message))
        })?;
        self.held.push(Held { name: identified, connection });
        self.current = Some(self.held.len().saturating_sub(1));
        Ok(done(SqlState::SUCCESSFUL_COMPLETION))
    }

    /// [ISO/IEC 9075:1992 §15.2] SET CONNECTION `name`, or DEFAULT for
    /// none: that connection current, the one current before dormant.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0A001 in an active transaction \[GR1\]; 08003 if there is no
    /// such connection \[GR2\].
    fn set_connection(&mut self, name: Option<&ValueSpecification>) -> Result<Completed> {
        self.not_in_transaction()?;
        let name = name.map(text).transpose()?.map(|n| identifier(&n).unwrap_or(n));
        let Some(index) = self.find(name.as_deref()) else {
            let what = name.map_or_else(|| "no default connection".to_owned(), |n| format!("no connection {n}"));
            return Err(exception(SqlState::CONNECTION_DOES_NOT_EXIST, &format!("[ISO/IEC 9075:1992 §15.2 GR2] there is {what}")));
        };
        self.current = Some(index);
        Ok(done(SqlState::SUCCESSFUL_COMPLETION))
    }

    /// [ISO/IEC 9075:1992 §15.3] DISCONNECT `object`: each connection it
    /// names ended; none current after, if the current one was \[GR9\].
    ///
    /// # Errors
    ///
    /// SQLSTATE 08003 if it names no connection held \[GR1-GR3\]; 25000 if
    /// one's transaction is active \[GR6\]. An error ending one is a warning,
    /// 01002 \[GR8\].
    fn disconnect(&mut self, object: &Disconnected) -> Result<Completed> {
        let missing =
            |what: String| exception(SqlState::CONNECTION_DOES_NOT_EXIST, &format!("[ISO/IEC 9075:1992 §15.3 GR1-GR3] there is {what}"));
        let ended: Vec<usize> = match object {
            Disconnected::All => (0..self.held.len()).collect(),
            Disconnected::Current => vec![self.current.ok_or_else(|| missing("no current connection".to_owned()))?],
            Disconnected::Default => vec![self.find(None).ok_or_else(|| missing("no default connection".to_owned()))?],
            Disconnected::Named(v) => {
                let n = text(v)?;
                let name = identifier(&n).unwrap_or(n);
                vec![self.find(Some(&name)).ok_or_else(|| missing(format!("no connection {name}")))?]
            }
        };
        if ended.iter().any(|&i| self.held.get(i).is_some_and(|h| h.connection.in_transaction())) {
            let e = SqlError::new(Code::TransactionState, "[ISO/IEC 9075:1992 §15.3 GR6] a connection's transaction is active");
            return Err(Failure::from(&e));
        }
        let current = self.current.and_then(|i| self.held.get(i)).map(|h| h.name.clone());
        let mut warned = false;
        let mut kept = Vec::new();
        for (i, h) in std::mem::take(&mut self.held).into_iter().enumerate() {
            if ended.contains(&i) {
                warned |= h.connection.terminate().is_err();
            } else {
                kept.push(h);
            }
        }
        self.held = kept;
        self.current = current.and_then(|name| self.find(name.as_deref()));
        Ok(done(if warned { SqlState::DISCONNECT_ERROR } else { SqlState::SUCCESSFUL_COMPLETION }))
    }

    /// Ends every connection, as the client ends.
    ///
    /// # Errors
    ///
    /// The first failure ending one; every one is ended still.
    pub fn terminate(self) -> Result<()> {
        let mut first = None;
        for h in self.held {
            if let Err(e) = h.connection.terminate() {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(()), Err)
    }
}
