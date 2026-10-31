//! \[DESIGN\] A source's records inserted into a table (RULES.md D59):
//! through one prepared INSERT, its columns the source's headings, or
//! else the table's, as a prepared query's description gives them
//! [ISO/IEC 9075:1992 §17.6, §17.9 GR3]; each value cast by the server to
//! its column's type [§17.6 GR5, §17.9 GR5f, g], but a string for a
//! character string column, which is assigned to it as INSERT assigns
//! [§13.8 GR4b iii], so one too long is refused, not cut. A batch of rows
//! is committed at a time; a failure rolls the batch back and stops, or,
//! if asked, a bad row is skipped and reported, and the import goes on.

use std::fmt;
use std::io::{Read, Write};

use turborel::error::{Code, SqlError};
use turborel::lexer::{Tok, Token, delim, lex};
use turborel::params::ParameterType;
use turborel::value::{Value, assign};
use turborel_protocol::Failure;

use super::{Heading, Position, Record, Source};
use crate::connection::Connection;
use crate::statements::Statement;

/// \[DESIGN\] How many rows an import commits at a time, unless it is
/// told: enough that a commit's cost is shared by many rows, few enough
/// that a failure rolls back little.
pub const DEFAULT_BATCH: u64 = 1000;

/// Ends a batch [ISO/IEC 9075:1992 §14.3].
const COMMIT: &str = "COMMIT";
/// Undoes a batch [ISO/IEC 9075:1992 §14.4].
const ROLLBACK: &str = "ROLLBACK";
/// The most parts of a `<table name>`: catalog, schema and table
/// [ISO/IEC 9075:1992 §5.4].
const MAX_NAME_PARTS: usize = 3;
/// The rule by which INSERT assigns a value to its column.
const INSERT_ASSIGNS: &str = "ISO/IEC 9075:1992 §13.8 GR4b iii";

/// What an import is to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// The `<table name>` the rows go into, as SQL writes it.
    pub table: String,
    /// How many rows are committed at a time; 0 for all at once.
    pub batch: u64,
    /// Whether a row refused, by the server or as a string too long for
    /// its column, or of too many or too few fields, is skipped and
    /// reported, rather than ending the import.
    pub skip: bool,
}

impl Import {
    /// An import into `table`, `DEFAULT_BATCH` rows at a time, stopping at
    /// the first row that fails.
    pub fn into(table: &str) -> Import {
        Import { table: table.to_owned(), batch: DEFAULT_BATCH, skip: false }
    }
}

/// What an import did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Imported {
    /// How many rows were inserted and committed.
    pub rows: u64,
    /// How many were skipped, each reported.
    pub rejected: u64,
}

/// Why a row, or an import, failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    /// The source's: it cannot be read, or a record does not fit the
    /// columns.
    Input(String),
    /// An SQL condition: the server's, or of a string too long for its
    /// column.
    Sql(Box<Failure>),
}

impl Cause {
    /// The SQL condition `failure`, of a statement the import made, not
    /// the user: so without its position in that statement.
    fn sql(failure: Failure) -> Cause {
        Cause::Sql(Box::new(Failure { position: None, ..failure }))
    }
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cause::Input(message) => f.write_str(message),
            Cause::Sql(failure) => failure.fmt(f),
        }
    }
}

/// A row skipped: where it began, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// Where its record began.
    pub position: Position,
    /// Why it failed.
    pub cause: Cause,
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}; skipped", self.position, self.cause)
    }
}

/// Why an import stopped, and what of it was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError {
    /// Where the record that stopped it began, if one did.
    pub position: Option<Position>,
    /// Why it stopped.
    pub cause: Cause,
    /// How many rows were committed before.
    pub committed: u64,
    /// How many of the batch under way were rolled back.
    pub rolled_back: u64,
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(position) = self.position {
            write!(f, "{position}: ")?;
        }
        write!(f, "{}; {} committed, {} rolled back", self.cause, rows(self.committed), self.rolled_back)
    }
}

impl std::error::Error for ImportError {}

/// An error of an import before any row: nothing committed.
fn before(cause: Cause) -> ImportError {
    ImportError { position: None, cause, committed: 0, rolled_back: 0 }
}

/// `n` rows, in words: "1 row", "2 rows".
pub fn rows(n: u64) -> String {
    format!("{n} row{}", if n == 1 { "" } else { "s" })
}

/// `name` as a `<delimited identifier>` [ISO/IEC 9075:1992 §5.2], a
/// quote in it written twice.
fn delimited(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// [ISO/IEC 9075:1992 §5.4] `text`, a `<table name>`, as SQL text of
/// delimited identifiers, so nothing else is taken for one.
fn table_name(text: &str) -> Result<String, String> {
    let wrong = || format!("[ISO/IEC 9075:1992 §5.4] {text} is not a <table name>");
    let tokens = lex(text).map_err(|_| wrong())?;
    let mut rest = tokens.iter();
    let mut parts = Vec::new();
    loop {
        let Some(Token { tok: Tok::Identifier(name), .. }) = rest.next() else { return Err(wrong()) };
        parts.push(delimited(name));
        let next = rest.next();
        if matches!(next, None | Some(Token { tok: Tok::End, .. })) {
            break;
        }
        if !matches!(next, Some(Token { tok: Tok::Delimiter(delim::PERIOD), .. })) || parts.len() == MAX_NAME_PARTS {
            return Err(wrong());
        }
    }
    Ok(parts.join("."))
}

/// The column `heading` names: the `<identifier>` it is, if it is one, a
/// regular one in upper case [ISO/IEC 9075:1992 §5.2 SR10], and so a key
/// word, as it would be one but for being reserved; else its name as
/// written.
fn column_name(heading: &Heading) -> String {
    match lex(&heading.name).as_deref() {
        Ok([Token { tok: Tok::Identifier(name), .. }, Token { tok: Tok::End, .. }]) => name.clone(),
        Ok([Token { tok: Tok::KeyWord(word), .. }, Token { tok: Tok::End, .. }]) => (*word).to_owned(),
        Ok(_) | Err(_) => heading.name.clone(),
    }
}

/// `values` as they are sent for parameters of `types`: a character
/// string for a character string column assigned to it as INSERT assigns
/// it [ISO/IEC 9075:1992 §13.8 GR4b iii, §9.2 GR3b, GR3e], so one too
/// long is refused, where the CAST of a parameter's value would cut it,
/// with a warning [§6.10 GR5c ii, GR6c ii; §17.9 GR5g]; any other value
/// as it is, for the server to cast.
fn assigned(values: &[Value], types: &[ParameterType]) -> Result<Vec<Value>, Failure> {
    let each = values.iter().zip(types).map(|(v, t)| {
        let to = t.data_type();
        let string = matches!(v, Value::Char(_)) && to.is_character();
        if string { assign(v.clone(), &to, INSERT_ASSIGNS).map_err(|e| Failure::from(&e)) } else { Ok(v.clone()) }
    });
    each.collect()
}

/// Whether `failure` is of a statement cancelled (RULES.md D44).
fn cancelled(failure: &Failure) -> bool {
    failure.sqlcode == SqlError::new(Code::Cancelled, "").sqlcode()
}

/// \[DESIGN\] RULES.md D59: inserts `source`'s records into the table
/// `how` names, over `connection`, as `how` says; each row skipped is
/// given to `rejected`. The connection's session must have no
/// transaction active, as an import commits its batches.
///
/// # Errors
///
/// Why it stopped, where, and how many rows were committed: a transaction
/// active; the table not named right, or not found; the source's
/// headings naming no columns of it; a record that cannot be read, or of
/// too many or too few fields; a row refused, by the server or as a
/// string too long for its column; a commit that fails. The batch under
/// way is rolled back.
pub fn import<S: Read + Write, R: Source>(
    connection: &mut Connection<S>,
    source: &mut R,
    how: &Import,
    rejected: &mut impl FnMut(Rejected),
) -> Result<Imported, ImportError> {
    let insert = prepared(connection, source, how)?;
    let mut run = Run { connection, insert, how, committed: 0, in_batch: 0, rejected: 0 };
    let done = run.rows(source, rejected);
    let Run { connection: over, insert: statement, .. } = run;
    if over.is_lost() {
        return done;
    }
    // Let go of even if the import stopped, so the server holds nothing.
    let freed = over.deallocate(statement);
    let imported = done?;
    freed.map_err(|f| ImportError { position: None, cause: Cause::sql(f), committed: imported.rows, rolled_back: 0 })?;
    Ok(imported)
}

/// The INSERT that puts `source`'s records into the table `how` names,
/// prepared over `connection`.
fn prepared<S: Read + Write, R: Source>(connection: &mut Connection<S>, source: &mut R, how: &Import) -> Result<Statement, ImportError> {
    if connection.in_transaction() {
        let msg = "[DESIGN] an import begins with no transaction active, as it commits its batches";
        return Err(before(Cause::Input(msg.to_owned())));
    }
    let table = table_name(&how.table).map_err(|msg| before(Cause::Input(msg)))?;
    let headings = source.headings().map_err(|e| ImportError { position: Some(e.position), ..before(Cause::Input(e.message)) })?;
    let columns = match headings {
        Some(headings) => headings.iter().map(column_name).collect(),
        None => described(connection, &table)?,
    };
    let names: Vec<String> = columns.iter().map(|c| delimited(c)).collect();
    let marks = vec!["?"; names.len()].join(", ");
    let sql = format!("INSERT INTO {table} ({}) VALUES ({marks})", names.join(", "));
    connection.prepare(&sql).map_err(|f| before(Cause::sql(f)))
}

/// [ISO/IEC 9075:1992 §17.9 GR3] The names of `table`'s columns, in
/// order, as the description of a query of them gives them.
fn described<S: Read + Write>(connection: &mut Connection<S>, table: &str) -> Result<Vec<String>, ImportError> {
    let query = connection.prepare(&format!("SELECT * FROM {table}")).map_err(|f| before(Cause::sql(f)))?;
    let names = query.columns().iter().map(|c| c.name.clone().unwrap_or_default()).collect();
    connection.deallocate(query).map_err(|f| before(Cause::sql(f)))?;
    Ok(names)
}

/// An import under way.
struct Run<'a, S: Read + Write> {
    connection: &'a mut Connection<S>,
    insert: Statement,
    how: &'a Import,
    /// Rows committed.
    committed: u64,
    /// Rows inserted in the batch under way.
    in_batch: u64,
    /// Rows skipped.
    rejected: u64,
}

impl<S: Read + Write> Run<'_, S> {
    /// Inserts each of `source`'s records, then commits the last batch.
    fn rows<R: Source>(&mut self, source: &mut R, rejected: &mut impl FnMut(Rejected)) -> Result<Imported, ImportError> {
        loop {
            let record = source.record().map_err(|e| self.stop(Some(e.position), Cause::Input(e.message)))?;
            let Some(record) = record else { break };
            self.row(&record, rejected)?;
        }
        self.commit()?;
        Ok(Imported { rows: self.committed, rejected: self.rejected })
    }

    /// Inserts `record`, committing its batch if it ends one.
    fn row(&mut self, record: &Record, rejected: &mut impl FnMut(Rejected)) -> Result<(), ImportError> {
        let wanted = self.insert.parameters().len();
        if record.values.len() != wanted {
            let msg = format!("[DESIGN] {} fields, for {wanted} columns", record.values.len());
            return self.reject(Rejected { position: record.position, cause: Cause::Input(msg) }, rejected);
        }
        let values = assigned(&record.values, self.insert.parameters());
        match values.and_then(|v| self.connection.execute(&self.insert, &v)) {
            Ok(_) => {
                self.in_batch = self.in_batch.saturating_add(1);
                if self.in_batch == self.how.batch { self.commit() } else { Ok(()) }
            }
            Err(failure) => {
                // A row is skipped only if it alone was undone: the
                // batch's transaction goes on, and the connection.
                let alone = self.in_batch == 0 || self.connection.in_transaction();
                if !alone || failure.ends_connection() || cancelled(&failure) {
                    return Err(self.stop(Some(record.position), Cause::sql(failure)));
                }
                self.reject(Rejected { position: record.position, cause: Cause::sql(failure) }, rejected)
            }
        }
    }

    /// Skips the row `row` and reports it to `rejected`, if the import
    /// skips bad rows; else stops.
    fn reject(&mut self, row: Rejected, rejected: &mut impl FnMut(Rejected)) -> Result<(), ImportError> {
        if !self.how.skip {
            return Err(self.stop(Some(row.position), row.cause));
        }
        self.rejected = self.rejected.saturating_add(1);
        rejected(row);
        Ok(())
    }

    /// Stops the import at `position`, for `cause`: the batch under way
    /// rolled back.
    fn stop(&mut self, position: Option<Position>, cause: Cause) -> ImportError {
        if !self.connection.is_lost() && self.connection.in_transaction() {
            // The import has failed already: a rollback that fails too
            // loses the connection, which ends the session's transaction.
            let _ = self.connection.query(ROLLBACK);
        }
        ImportError { position, cause, committed: self.committed, rolled_back: self.in_batch }
    }

    /// Commits the batch under way, if a transaction is active.
    fn commit(&mut self) -> Result<(), ImportError> {
        if self.connection.in_transaction() {
            self.connection.query(COMMIT).map_err(|f| ImportError {
                position: None,
                cause: Cause::sql(f),
                committed: self.committed,
                rolled_back: self.in_batch,
            })?;
        }
        self.committed = self.committed.saturating_add(self.in_batch);
        self.in_batch = 0;
        Ok(())
    }
}
