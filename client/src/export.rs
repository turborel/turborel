//! \[DESIGN\] Export (RULES.md D195): a query's rows streamed from the
//! server and written as they come, a batch at a time, so memory holds a
//! batch whatever the result's size. As CSV [RFC 4180]: a header of the
//! columns' names, then a record for each row, a field quoted where it
//! holds a comma, a quote, a line break or spaces at either end, a quote
//! in it written twice; a null an empty field, an empty string `""`. Or
//! as native rows: the protocol's own frames, a Description, then Rows,
//! then a Complete, which `read_native` reads back, every value as it
//! was, of whatever type.

use std::io::{Read, Write};

use turborel::storage::Row;
use turborel::value::Value;
use turborel_protocol::{Column, Completed, Failure, HEADER, MAX_BODY, Reply};

use crate::connection::{Connection, Result, Sink, Streamed, failed};

/// How an export writes rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// CSV [RFC 4180], with a header of the columns' names.
    Csv,
    /// The protocol's frames: a Description, Rows, a Complete.
    Native,
}

/// A query's rows, written to `out` as they come.
struct Writer<'w> {
    out: &'w mut dyn Write,
    format: ExportFormat,
    /// The first write that failed, which ends the export.
    failed: Option<std::io::Error>,
}

/// `field`, as CSV writes it: quoted where it must be.
fn csv_field(field: &str) -> String {
    let quoted = field.is_empty()
        || field.contains([',', '"', '\n', '\r'])
        || field.starts_with(char::is_whitespace)
        || field.ends_with(char::is_whitespace);
    if quoted { format!("\"{}\"", field.replace('"', "\"\"")) } else { field.to_owned() }
}

/// `v` as a CSV field: a null empty and unquoted.
fn csv_value(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Char(s) => csv_field(s),
        other @ (Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Lob(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Json(_)
        | Value::Extension(_)
        | Value::Xml(_)) => csv_field(&other.to_string()),
    }
}

/// A CSV record of `fields`, ended by CR LF [RFC 4180 §2].
fn csv_record(fields: impl Iterator<Item = String>) -> Vec<u8> {
    let mut line = fields.collect::<Vec<_>>().join(",");
    line.push_str("\r\n");
    line.into_bytes()
}

impl Writer<'_> {
    /// Writes `bytes`, unless a write failed before.
    fn write(&mut self, bytes: &[u8]) {
        if self.failed.is_none() {
            self.failed = self.out.write_all(bytes).err();
        }
    }

    /// Writes `reply` as a frame.
    fn frame(&mut self, reply: &Reply) {
        let mut bytes = Vec::new();
        reply.encode(&mut bytes);
        self.write(&bytes);
    }
}

impl Sink for Writer<'_> {
    fn columns(&mut self, columns: &[Column]) {
        match self.format {
            ExportFormat::Csv => {
                let names = columns.iter().map(|c| csv_field(c.name.as_deref().unwrap_or_default()));
                self.write(&csv_record(names));
            }
            ExportFormat::Native => self.frame(&Reply::Description(columns.to_vec())),
        }
    }

    fn rows(&mut self, rows: &[Row]) {
        match self.format {
            ExportFormat::Csv => {
                let bytes: Vec<u8> = rows.iter().flat_map(|row| csv_record(row.iter().map(csv_value))).collect();
                self.write(&bytes);
            }
            ExportFormat::Native => self.frame(&Reply::Rows(rows.to_vec())),
        }
    }
}

impl<S: Read + Write> Connection<S> {
    /// \[DESIGN\] RULES.md D195: runs the query `sql` and writes its rows to
    /// `out` as `format` says, as they come: how many.
    ///
    /// # Errors
    ///
    /// The query's, as `query_into`'s; a statement that is no query; a
    /// write to `out` that failed (SQLSTATE 08006 is not the connection's
    /// then, but the export's, which ends).
    pub fn export(&mut self, sql: &str, out: &mut dyn Write, format: ExportFormat) -> Result<u64> {
        let mut writer = Writer { out, format, failed: None };
        let streamed = self.query_into(sql, &mut writer)?;
        let Streamed::Rows(count) = streamed else { return Err(failed("[DESIGN] an export runs a query")) };
        if format == ExportFormat::Native {
            writer.frame(&Reply::Complete(Completed::of(0, "00000")));
        }
        let flushed = writer.failed.map_or_else(|| writer.out.flush(), Err);
        flushed.map_err(|e| failed(&format!("[DESIGN] cannot write the export: {e}")))?;
        Ok(count)
    }
}

/// Reads native rows (`ExportFormat::Native`) from `input`: the columns,
/// and the rows.
///
/// # Errors
///
/// SQLSTATE 08006 for input that is not native rows, or cut short.
pub fn read_native(input: &mut impl Read) -> Result<(Vec<Column>, Vec<Row>)> {
    let not_native = |why: &str| failed(&format!("[DESIGN] not native rows: {why}"));
    let mut next = || -> Result<Reply> {
        let mut head = [0_u8; HEADER];
        input.read_exact(&mut head).map_err(|e| not_native(&e.to_string()))?;
        let (kind, length) = turborel_protocol::header(head, MAX_BODY).map_err(|e| Failure::from(&e))?;
        let mut body = vec![0; length];
        input.read_exact(&mut body).map_err(|e| not_native(&e.to_string()))?;
        Reply::decode(kind, &body).map_err(|e| Failure::from(&e))
    };
    let Reply::Description(columns) = next()? else { return Err(not_native("no Description first")) };
    let mut rows = Vec::new();
    loop {
        match next()? {
            Reply::Rows(batch) => rows.extend(batch),
            Reply::Complete(_) => return Ok((columns, rows)),
            Reply::Error(_)
            | Reply::Lob(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Description(_)
            | Reply::Suspended
            | Reply::Notice(_)
            | Reply::Status { .. }
            | Reply::Pong
            | Reply::Debug(_)
            | Reply::Event(_) => return Err(not_native("a frame other than Rows before the Complete")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] [RFC 4180 §2] RULES.md D195: a CSV field is
    /// quoted where it holds a comma, a quote, a line break or spaces at
    /// either end, its quotes doubled; an empty string quoted, a null
    /// empty; each record ended by CR LF.
    #[test]
    fn csv_fields_are_quoted_where_they_must_be() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field(" pad"), "\" pad\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_value(&Value::Char(String::new().into())), "\"\"");
        assert_eq!(csv_value(&Value::Null), "");
        assert_eq!(csv_record(["1".to_owned(), String::new()].into_iter()), b"1,\r\n");
    }
}
