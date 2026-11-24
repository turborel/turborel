//! \[DESIGN\] Export (RULES.md D195, D975): a query's rows streamed from the
//! server and written as they come, so memory holds a batch whatever the
//! result's size, by a `Layout`: as delimited text (`Delimited`), CSV [RFC
//! 4180] among it, or as native rows, the protocol's own frames, which
//! `read_native` reads back, every value as it was, of whatever type.
//!
//! A large object comes whole if it is short (SERVER.LOB_INLINE_BYTES),
//! else by its locator [ISO/IEC 9075-2:2023 §4.37.5]. Each one sent so is
//! read as its row is written, a chunk at a time, and its value written,
//! never its locator. So the rows of a query that may hold one come a
//! batch at a time, through a cursor, the large objects of a batch read
//! between its fetches, within the transaction they were sent in (`run`).
//! A large object a column holds is written a chunk at a time, as it is
//! read. One within a row, an array, a multiset or a structured value is
//! read whole, as the text of the value it is part of needs it all.

mod delimited;
mod native;
mod run;
#[cfg(test)]
mod tests;

use std::io::{self, Read, Write};
use std::sync::Arc;

use turborel::storage::Row;
use turborel::value::lob::{CHUNK_OCTETS, Lob, LobKind};
use turborel::value::types::Structured;
use turborel::value::{DataType, Value};
use turborel_protocol::{Column, Failure};

pub use delimited::{Delimited, Quoting};
pub use native::{Native, read_native};

use crate::connection::{Connection, Result};

/// How an export writes rows, as `Connection::export` takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// CSV [RFC 4180], with a header of the columns' names
    /// (`Delimited::csv`).
    Csv,
    /// The protocol's frames: a Description, Rows, a Complete (`Native`).
    Native,
}

/// \[DESIGN\] RULES.md D975: what reads the large objects of an export's
/// rows, as a `Layout` writes them.
pub trait Lobs {
    /// `lob`'s octets, a chunk at a time, each given to `each` in order. One
    /// held whole is given at once, one sent by its locator a chunk a
    /// request, its locator freed once it is read.
    ///
    /// # Errors
    ///
    /// A read that fails, which ends the export, or `each`'s.
    fn chunks(&mut self, lob: &Lob, each: &mut dyn FnMut(&[u8]) -> io::Result<()>) -> io::Result<()>;
}

/// \[DESIGN\] RULES.md D975: how an export writes a query's rows to its
/// output. A program may write any format by one.
pub trait Layout {
    /// Writes what comes before the rows, given their columns.
    ///
    /// # Errors
    ///
    /// The write's.
    fn columns(&mut self, out: &mut dyn Write, columns: &[Column]) -> io::Result<()>;

    /// Writes a batch of rows, reading each large object sent by its
    /// locator through `lobs`.
    ///
    /// # Errors
    ///
    /// The write's, or a read's.
    fn rows(&mut self, out: &mut dyn Write, rows: &[Row], lobs: &mut dyn Lobs) -> io::Result<()>;

    /// Writes what comes after the last row.
    ///
    /// # Errors
    ///
    /// The write's.
    fn end(&mut self, out: &mut dyn Write) -> io::Result<()>;
}

/// Whether `lob` was sent by its locator, its octets still the server's.
pub(crate) fn located(lob: &Lob) -> bool {
    lob.inline().is_none()
}

/// Whether `v` is or holds, however deep, a large object sent by its
/// locator.
pub fn holds_located(v: &Value) -> bool {
    match v {
        Value::Lob(l) => located(l),
        Value::Array(items) | Value::Multiset(items) | Value::Row(items) => items.iter().any(holds_located),
        Value::Structured(s) => s.attributes.iter().any(holds_located),
        Value::Null
        | Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => false,
    }
}

/// Whether a value of `t` may hold a large object: one of a large object
/// type, or of a composite type of one. A client is not told a structured
/// type's attributes, nor what an extension's value holds, so either may.
pub(crate) fn may_hold_lob(t: &DataType) -> bool {
    match t {
        DataType::Lob { .. } | DataType::Structured(_) | DataType::Extension(_) => true,
        DataType::Array(a) => may_hold_lob(&a.element),
        DataType::Multiset(m) => may_hold_lob(&m.element),
        DataType::Row(r) => r.fields.iter().any(|f| may_hold_lob(&f.data_type)),
        DataType::Distinct(d) => may_hold_lob(&d.source),
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Reference(_)
        | DataType::Json
        | DataType::Xml(_) => false,
    }
}

/// `v`, each large object in it sent by its locator, depth first, given to
/// `each` for the value to stand in its place; `v` itself if it holds none.
///
/// # Errors
///
/// `each`'s.
pub fn map_located(v: &Value, each: &mut dyn FnMut(&Lob) -> io::Result<Lob>) -> io::Result<Value> {
    if !holds_located(v) {
        return Ok(v.clone());
    }
    let mut all = |items: &[Value]| items.iter().map(|i| map_located(i, each)).collect::<io::Result<Vec<_>>>().map(Arc::<[Value]>::from);
    Ok(match v {
        Value::Lob(l) => Value::Lob(each(l)?),
        Value::Array(items) => Value::Array(all(items)?),
        Value::Multiset(items) => Value::Multiset(all(items)?),
        Value::Row(items) => Value::Row(all(items)?),
        Value::Structured(s) => Value::Structured(Arc::new(Structured::new(Arc::clone(&s.of), all(&s.attributes)?))),
        Value::Null
        | Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => v.clone(),
    })
}

/// The large object of `kind` whose octets are `bytes`, held whole.
///
/// # Errors
///
/// `InvalidData` if a character one's octets are not UTF-8.
pub(crate) fn held(kind: LobKind, bytes: &[u8]) -> io::Result<Lob> {
    match kind {
        LobKind::Binary => Ok(Lob::of_octets(bytes)),
        LobKind::Character => std::str::from_utf8(bytes).map(Lob::of_text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
    }
}

/// `v`, each large object in it sent by its locator read whole through
/// `lobs`, as a value written as one text needs it.
///
/// # Errors
///
/// A read's.
pub fn read_whole(v: &Value, lobs: &mut dyn Lobs) -> io::Result<Value> {
    map_located(v, &mut |l| {
        let mut bytes = Vec::with_capacity(usize::try_from(l.octets()).unwrap_or(0));
        lobs.chunks(l, &mut |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        })?;
        held(l.kind(), &bytes)
    })
}

/// The large objects of an export, read over its connection, the first
/// read that failed kept as the export's failure.
pub(crate) struct Reading<'c, S: Read + Write> {
    pub(crate) connection: &'c mut Connection<S>,
    pub(crate) failure: Option<Failure>,
}

impl<S: Read + Write> Reading<'_, S> {
    /// `failure`, kept, as the I/O error that ends the layout's work.
    fn failed(&mut self, failure: Failure) -> io::Error {
        let e = io::Error::other(failure.to_string());
        self.failure = Some(failure);
        e
    }
}

impl<S: Read + Write> Lobs for Reading<'_, S> {
    fn chunks(&mut self, lob: &Lob, each: &mut dyn FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
        if let Some(bytes) = lob.inline() {
            return each(bytes);
        }
        let wanted = u64::try_from(CHUNK_OCTETS).unwrap_or(u64::MAX);
        let mut at = 0_u64;
        while at < lob.octets() {
            let chunk = match self.connection.read_lob(lob, (at, wanted), false) {
                Ok(chunk) => chunk,
                Err(failure) => return Err(self.failed(failure)),
            };
            if chunk.is_empty() {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the server sent less of a large object than its length"));
            }
            at = at.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            each(&chunk)?;
        }
        // Read, its locator is of no more use. The server lets go of what it
        // holds for it, so an export of many holds no more than a batch's.
        match self.connection.free_lob(lob) {
            Ok(_) => Ok(()),
            Err(failure) => Err(self.failed(failure)),
        }
    }
}

impl<S: Read + Write> Connection<S> {
    /// \[DESIGN\] RULES.md D195, D975: runs the query `sql` and writes its
    /// rows to `out` as `format` says, as they come: how many. Its large
    /// objects are written as their values.
    ///
    /// # Errors
    ///
    /// As `export_with`'s.
    pub fn export(&mut self, sql: &str, out: &mut dyn Write, format: ExportFormat) -> Result<u64> {
        match format {
            ExportFormat::Csv => self.export_with(sql, out, &mut Delimited::csv()),
            ExportFormat::Native => self.export_with(sql, out, &mut Native),
        }
    }
}
