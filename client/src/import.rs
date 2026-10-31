//! \[DESIGN\] Import (RULES.md D59): rows read from a `Source` and inserted
//! into a table over a connection, through a prepared INSERT whose
//! dynamic parameters take the table's columns' types [ISO/IEC 9075:1992
//! §17.6 GR5], a batch of rows to each transaction. A source is any type
//! that yields records of values, one at a time, so memory is bounded by
//! a record, whatever the source's size; `Delimited` reads CSV [RFC 4180]
//! and tab-separated text.

mod delimited;
mod insert;

use std::fmt;

use turborel::value::{DataType, Value};

pub use delimited::{DEFAULT_MAX_RECORD, Delimited, Dialect};
pub use insert::{Cause, DEFAULT_BATCH, Import, ImportError, Imported, Rejected, import, rows};

/// Where in a source a record began: its number, the first 1, and the
/// line it began on, the first 1, if the source is made of lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Position {
    /// The record's number among the source's records of data, a header
    /// not counted.
    pub record: u64,
    /// The line it began on, of a source made of lines.
    pub line: Option<u64>,
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "record {}", self.record)?;
        if let Some(line) = self.line {
            write!(f, ", line {line}")?;
        }
        Ok(())
    }
}

/// What a source says of one of its fields, if it says: a name, which
/// names a column of the table, and a type, if it knows one.
#[derive(Debug, Clone, PartialEq)]
pub struct Heading {
    /// The name: read as an `<identifier>` if it is one, a regular one in
    /// upper case, else as written.
    pub name: String,
    /// The type of its values, if the source knows it.
    pub data_type: Option<DataType>,
}

/// A record of a source: a value for each of its fields, and where it
/// began.
#[derive(Debug, Clone)]
pub struct Record {
    /// The values, one a field. A value is cast to its column's type as
    /// it is inserted [ISO/IEC 9075:1992 §17.9 GR5f, g], so a source that
    /// knows no types gives character strings, and null.
    pub values: Vec<Value>,
    /// Where it began.
    pub position: Position,
}

/// Why a source cannot give its next record: what is wrong, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    /// Where the record began.
    pub position: Position,
    /// What is wrong, in plain words.
    pub message: String,
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.position, self.message)
    }
}

impl std::error::Error for SourceError {}

/// \[DESIGN\] RULES.md D59: where rows to import come from. A program may
/// implement it for any format; `Delimited` reads CSV and tab-separated
/// text.
///
/// The contract: `headings` is asked once, before the first record; then
/// `record` until it gives none, and none again if asked after. A source
/// holds no more than a record at a time, however long it is, so memory
/// is bounded; and it refuses a record past a bound of its own, rather
/// than read the rest of its input looking for its end.
pub trait Source {
    /// Its fields' names and types, if it knows them, as of a header row;
    /// none, if it does not, its fields then taken as the table's columns
    /// in order.
    ///
    /// # Errors
    ///
    /// Why it cannot be read, and where.
    fn headings(&mut self) -> Result<Option<Vec<Heading>>, SourceError>;

    /// The next record; none at the end.
    ///
    /// # Errors
    ///
    /// Why it cannot be read, and where: a source cannot go on after.
    fn record(&mut self) -> Result<Option<Record>, SourceError>;
}
