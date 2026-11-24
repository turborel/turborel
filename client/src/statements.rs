//! \[EXTENSION\] Prepared statements over a connection [ISO/IEC 9075:1992
//! §17] (RULES.md D45): prepared once, their parameters' types and columns
//! described; executed, or, a query, its cursor opened and its rows read
//! a batch at a time, as often as asked. Granted `cursors`, a statement
//! has several cursors open at once, each numbered by the caller; granted
//! `describe`, a statement or an open cursor is described again
//! (RULES.md D188).

use std::io::{Read, Write};

use turborel::catalog::QualifiedName;
use turborel::params::{DynamicParameter, ParameterType};
use turborel::storage::Row;
use turborel::value::Value;
use turborel_protocol::{Attributes, Called, Column, Completed, Described, Reply, Request};

use crate::connection::{Connection, Outcome, Result};
use crate::frames::Answer;

/// A statement the server prepared, of a connection.
#[derive(Debug, Clone)]
pub struct Statement {
    pub(crate) number: u32,
    inputs: Vec<ParameterType>,
    columns: Vec<Column>,
    called: Option<Called>,
}

impl Statement {
    /// [ISO/IEC 9075-2:2023 §20.10 GR7] (RULES.md D977, D978) Each of its
    /// dynamic parameters, in order: its type, and, of a CALL, the mode
    /// and ordinal position of the parameter whose argument holds it
    /// [GR7e], as a server granting `parameter-modes` tells them. Told
    /// none, each is an input one.
    pub fn parameters(&self) -> Vec<DynamicParameter> {
        match &self.called {
            Some(called) => called.parameters.clone(),
            None => self.inputs.iter().cloned().map(DynamicParameter::input).collect(),
        }
    }

    /// [§17.9 GR4] The types of the dynamic parameters an execution gives
    /// values for, in order: of a CALL not those of output parameters
    /// alone.
    pub fn inputs(&self) -> &[ParameterType] {
        &self.inputs
    }

    /// [ISO/IEC 9075-2:2023 §20.10 GR7e vi] Of a CALL of an SQL-invoked
    /// procedure, the procedure's specific name, as a server granting
    /// `parameter-modes` tells it.
    pub fn procedure(&self) -> Option<&QualifiedName> {
        self.called.as_ref().map(|c| &c.procedure)
    }

    /// [§17.9 GR3] The columns of its rows, of a query; of a CALL, one for
    /// each `?` of an output parameter, whose values `execute` gives as a
    /// row [ISO/IEC 9075-2:2023 §20.7 GR7a]; none of another.
    pub fn columns(&self) -> &[Column] {
        &self.columns
    }
}

/// Rows of a cursor, and whether it has more.
#[derive(Debug, Clone)]
pub struct Batch {
    /// The rows.
    pub rows: Vec<Row>,
    /// Whether the cursor has more, which `fetch` reads: closed if not.
    pub more: bool,
}

/// The request that executes `statement` with `values`, its cursor
/// numbered `cursor`, at most `rows` of a query's rows sent, all if 0; the
/// transaction ended with it if `ends`; carrying `attributes`.
pub(crate) fn execute_request(
    statement: &Statement,
    (cursor, values): (u32, &[Value]),
    rows: u32,
    (ends, attributes): (bool, Attributes),
) -> Request {
    Request::Execute { statement: statement.number, cursor, values: values.to_vec(), rows, ends, attributes }
}

impl<S: Read + Write> Connection<S> {
    /// The server's Complete, after `request`.
    pub(crate) fn completed(&mut self, request: &Request) -> Result<Completed> {
        self.send(request)?;
        self.completion()
    }

    /// The server's Complete, of a request sent.
    pub(crate) fn completion(&mut self) -> Result<Completed> {
        match self.receive()? {
            Reply::Complete(completed) => Ok(completed),
            Reply::Error(failure) => Err(self.failed(failure)),
            Reply::Lob(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Description(_)
            | Reply::Rows(_)
            | Reply::Suspended
            | Reply::Notice(_)
            | Reply::Status { .. }
            | Reply::Pong => Err(self.lose("[DESIGN] the server sent no completion")),
        }
    }

    /// [ISO/IEC 9075:1992 §17.6] Prepares `sql`, a `<preparable statement>`
    /// whose dynamic parameters are written `?`.
    ///
    /// # Errors
    ///
    /// The server's: SQLSTATE 42000 if it is not preparable, or a `?` takes
    /// no type where it stands; a connection failure.
    pub fn prepare(&mut self, sql: &str) -> Result<Statement> {
        let number = self.next_statement;
        self.next_statement = self.next_statement.wrapping_add(1);
        self.send(&Request::Prepare { statement: number, sql: sql.to_owned() })?;
        self.prepared()
    }

    /// The statement the server's Prepared describes.
    fn prepared(&mut self) -> Result<Statement> {
        match self.receive()? {
            Reply::Prepared { statement, parameters, columns, called } => {
                Ok(Statement { number: statement, inputs: parameters, columns, called })
            }
            Reply::Error(failure) => Err(self.failed(failure)),
            Reply::Lob(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Description(_)
            | Reply::Rows(_)
            | Reply::Suspended
            | Reply::Complete(_)
            | Reply::Notice(_)
            | Reply::Status { .. }
            | Reply::Pong => Err(self.lose("[DESIGN] the server sent no Prepared")),
        }
    }

    /// [§17.10, §17.14] Executes `statement` with `values`: a query's rows,
    /// all of them; a CALL's output parameters' values, a row of its
    /// columns; or another statement's completion; the transaction ended
    /// with it under `set_autocommit`.
    ///
    /// # Errors
    ///
    /// The server's: SQLSTATE 07001 if `values` are not one for each
    /// parameter; the statement's; a connection failure.
    pub fn execute(&mut self, statement: &Statement, values: &[Value]) -> Result<Outcome> {
        let request = execute_request(statement, (0, values), 0, (self.autocommit, self.told.attributes()));
        self.send(&request)?;
        self.execute_reply(statement)
    }

    /// The reply to an Execute of `statement` sent: a query's rows, all of
    /// them, or another statement's completion.
    ///
    /// # Errors
    ///
    /// As `execute`'s.
    pub(crate) fn execute_reply(&mut self, statement: &Statement) -> Result<Outcome> {
        if statement.columns.is_empty() {
            return self.completion().map(Outcome::Done);
        }
        let (rows, _) = self.rows()?;
        Ok(Outcome::Rows { columns: statement.columns.clone(), rows })
    }

    /// [§17.14] Opens the cursor of `statement`, a query, with `values`: at
    /// most `rows` of its rows, all if 0. The cursor is read within its
    /// transaction, which `set_autocommit` leaves; but under autocommit,
    /// the server granting `fetch-ends`, the transaction ends as the cursor
    /// does: committed once its last row is read or it is closed, rolled
    /// back if a fetch fails (RULES.md D199).
    ///
    /// # Errors
    ///
    /// The server's: 24000 if its cursor is open; those of `execute`.
    pub fn open_cursor(&mut self, statement: &Statement, values: &[Value], rows: u32) -> Result<Batch> {
        self.open_cursor_numbered((statement, 0), values, rows)
    }

    /// \[DESIGN\] RULES.md D188: opens the cursor numbered `at.1` of the
    /// statement `at.0`, as `open_cursor` opens its one: a statement has
    /// several open at once, each of its own number, once `cursors` is
    /// granted; 0 is the one `open_cursor` opens.
    ///
    /// # Errors
    ///
    /// As `open_cursor`'s; a connection failure for a number other than 0
    /// if `cursors` was not granted.
    pub fn open_cursor_numbered(&mut self, (statement, cursor): (&Statement, u32), values: &[Value], rows: u32) -> Result<Batch> {
        self.needs(cursor == 0, turborel_protocol::capability::CURSORS)?;
        // Under autocommit, granted `fetch-ends`, the transaction ends as
        // the cursor does (RULES.md D199).
        let ends = self.autocommit && self.told.granted.has(turborel_protocol::capability::FETCH_ENDS);
        self.send(&execute_request(statement, (cursor, values), rows, (ends, self.told.attributes())))?;
        let (fetched, more) = self.rows()?;
        Ok(Batch { rows: fetched, more })
    }

    /// [§17.15] At most `rows` more rows of `statement`'s cursor, all if 0.
    ///
    /// # Errors
    ///
    /// The server's: 24000 if its cursor is not open; a connection failure.
    pub fn fetch(&mut self, statement: &Statement, rows: u32) -> Result<Batch> {
        self.fetch_numbered((statement, 0), rows)
    }

    /// At most `rows` more rows of the cursor numbered `at.1` of the
    /// statement `at.0`, all if 0 (`open_cursor_numbered`).
    ///
    /// # Errors
    ///
    /// As `fetch`'s.
    pub fn fetch_numbered(&mut self, (statement, cursor): (&Statement, u32), rows: u32) -> Result<Batch> {
        self.needs(cursor == 0, turborel_protocol::capability::CURSORS)?;
        self.send(&Request::Fetch { statement: statement.number, cursor, rows })?;
        let (fetched, more) = self.rows()?;
        Ok(Batch { rows: fetched, more })
    }

    /// [§17.16] Closes `statement`'s cursor.
    ///
    /// # Errors
    ///
    /// The server's: 24000 if it is not open; a connection failure.
    pub fn close(&mut self, statement: &Statement) -> Result<()> {
        self.close_numbered((statement, 0))
    }

    /// Closes the cursor numbered `at.1` of the statement `at.0`.
    ///
    /// # Errors
    ///
    /// As `close`'s.
    pub fn close_numbered(&mut self, (statement, cursor): (&Statement, u32)) -> Result<()> {
        self.needs(cursor == 0, turborel_protocol::capability::CURSORS)?;
        self.completed(&Request::Close { statement: statement.number, cursor }).map(drop)
    }

    /// \[DESIGN\] RULES.md D188: `statement` described again by the server:
    /// its parameters' types and its columns, as they are now.
    ///
    /// # Errors
    ///
    /// The server's: 26000 if the statement is no more; a connection
    /// failure if `describe` was not granted.
    pub fn describe(&mut self, statement: &Statement) -> Result<Statement> {
        self.needs(false, turborel_protocol::capability::DESCRIBE)?;
        self.send(&Request::Describe(Described::Statement(statement.number)))?;
        self.prepared()
    }

    /// \[DESIGN\] RULES.md D188: the columns of the open cursor numbered
    /// `at.1` of the statement `at.0`.
    ///
    /// # Errors
    ///
    /// The server's: 24000 if it is not open; a connection failure if
    /// `describe` was not granted.
    pub fn describe_cursor(&mut self, (statement, cursor): (&Statement, u32)) -> Result<Vec<Column>> {
        self.needs(false, turborel_protocol::capability::DESCRIBE)?;
        self.send(&Request::Describe(Described::Cursor { statement: statement.number, cursor }))?;
        // Its Description is its last reply, as no rows follow it.
        match self.receive_of(Answer::One)? {
            Reply::Description(columns) => Ok(columns),
            Reply::Error(failure) => Err(self.failed(failure)),
            Reply::Lob(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Rows(_)
            | Reply::Suspended
            | Reply::Complete(_)
            | Reply::Notice(_)
            | Reply::Status { .. }
            | Reply::Pong => Err(self.lose("[DESIGN] the server sent no Description")),
        }
    }

    /// [§17.7] Destroys `statement`.
    ///
    /// # Errors
    ///
    /// The server's: 24000 if its cursor is open; a connection failure.
    pub fn deallocate(&mut self, statement: Statement) -> Result<()> {
        let number = statement.number;
        // Destroyed: of no use after.
        drop(statement);
        self.completed(&Request::Deallocate { statement: number }).map(drop)
    }
}
