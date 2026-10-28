//! \[EXTENSION\] Prepared statements over a connection [ISO/IEC 9075:1992
//! §17] (RULES.md D45): prepared once, their parameters' types and columns
//! described; executed, or, a query, its cursor opened and its rows read
//! a batch at a time, as often as asked.

use std::io::{Read, Write};

use turborel::params::ParameterType;
use turborel::storage::Row;
use turborel::value::Value;
use turborel_protocol::{Column, Completed, Reply, Request};

use crate::connection::{Connection, Outcome, Result};

/// A statement the server prepared, of a connection.
#[derive(Debug, Clone)]
pub struct Statement {
    number: u32,
    parameters: Vec<ParameterType>,
    columns: Vec<Column>,
}

impl Statement {
    /// [§17.9 GR4] Its dynamic parameters' types, in order.
    pub fn parameters(&self) -> &[ParameterType] {
        &self.parameters
    }

    /// [§17.9 GR3] The columns of its rows, of a query; none of another.
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

/// The request that executes `statement` with `values`, at most `rows` of
/// a query's rows sent, all if 0.
fn execute_request(statement: &Statement, values: &[Value], rows: u32) -> Request {
    Request::Execute { statement: statement.number, values: values.to_vec(), rows }
}

impl<S: Read + Write> Connection<S> {
    /// The server's Complete, after `request`.
    fn completed(&mut self, request: &Request) -> Result<Completed> {
        self.send(request)?;
        match self.receive()? {
            Reply::Complete(completed) => Ok(completed),
            Reply::Error(failure) => Err(self.failed(failure)),
            Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Description(_)
            | Reply::Rows(_)
            | Reply::Suspended => Err(self.lose("[DESIGN] the server sent no completion")),
        }
    }

    /// [ISO/IEC 9075:1992 §17.6] Prepares `sql`, a `<preparable statement>`
    /// whose dynamic parameters are written `?`.
    ///
    /// # Errors
    ///
    /// The server's: SQLSTATE 37000 if it is not preparable, or a `?` takes
    /// no type where it stands; a connection failure.
    pub fn prepare(&mut self, sql: &str) -> Result<Statement> {
        let number = self.next_statement;
        self.next_statement = self.next_statement.wrapping_add(1);
        self.send(&Request::Prepare { statement: number, sql: sql.to_owned() })?;
        match self.receive()? {
            Reply::Prepared { statement, parameters, columns } => Ok(Statement { number: statement, parameters, columns }),
            Reply::Error(failure) => Err(self.failed(failure)),
            Reply::Challenge(_) | Reply::Ready { .. } | Reply::Description(_) | Reply::Rows(_) | Reply::Suspended | Reply::Complete(_) => {
                Err(self.lose("[DESIGN] the server sent no Prepared"))
            }
        }
    }

    /// [§17.10, §17.14] Executes `statement` with `values`: a query's rows,
    /// all of them, or another statement's completion.
    ///
    /// # Errors
    ///
    /// The server's: SQLSTATE 07001 if `values` are not one for each
    /// parameter; the statement's; a connection failure.
    pub fn execute(&mut self, statement: &Statement, values: &[Value]) -> Result<Outcome> {
        if statement.columns.is_empty() {
            return self.completed(&execute_request(statement, values, 0)).map(Outcome::Done);
        }
        let batch = self.open_cursor(statement, values, 0)?;
        Ok(Outcome::Rows { columns: statement.columns.clone(), rows: batch.rows })
    }

    /// [§17.14] Opens the cursor of `statement`, a query, with `values`: at
    /// most `rows` of its rows, all if 0.
    ///
    /// # Errors
    ///
    /// The server's: 24000 if its cursor is open; those of `execute`.
    pub fn open_cursor(&mut self, statement: &Statement, values: &[Value], rows: u32) -> Result<Batch> {
        self.send(&execute_request(statement, values, rows))?;
        match self.receive()? {
            Reply::Description(_) => {}
            Reply::Error(failure) => return Err(self.failed(failure)),
            Reply::Challenge(_) | Reply::Ready { .. } | Reply::Prepared { .. } | Reply::Rows(_) | Reply::Suspended | Reply::Complete(_) => {
                return Err(self.lose("[DESIGN] the server sent no Description"));
            }
        }
        let (fetched, more) = self.rows()?;
        Ok(Batch { rows: fetched, more })
    }

    /// [§17.15] At most `rows` more rows of `statement`'s cursor, all if 0.
    ///
    /// # Errors
    ///
    /// The server's: 24000 if its cursor is not open; a connection failure.
    pub fn fetch(&mut self, statement: &Statement, rows: u32) -> Result<Batch> {
        self.send(&Request::Fetch { statement: statement.number, rows })?;
        let (fetched, more) = self.rows()?;
        Ok(Batch { rows: fetched, more })
    }

    /// [§17.16] Closes `statement`'s cursor.
    ///
    /// # Errors
    ///
    /// The server's: 24000 if it is not open; a connection failure.
    pub fn close(&mut self, statement: &Statement) -> Result<()> {
        self.completed(&Request::Close { statement: statement.number }).map(drop)
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
