//! \[DESIGN\] `\export` (RULES.md D216) writes a table's or a query's
//! rows, streamed from the server as they come, to a file as CSV (RFC
//! 4180) or as tab-separated text, so `\import` and `--import` read them
//! back as they were. In CSV a null is written as the null text, unquoted,
//! and a value that would read back as a null, the empty string among
//! them, is quoted; in text a value cannot hold a tab or a line break.
//!
//! The rows come by a query (`Statements::query_into`), the protocol's
//! export of a query's rows as they come. `native` writes them as the
//! protocol's own frames instead (`Statements::export_native`), every
//! value with its type, as `turborel_client::export::read_native` reads
//! them back.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use turborel::storage::Row;
use turborel::value::Value;
use turborel_client::Column;
use turborel_client::connection::{Sink, Streamed};

use crate::run::Statements;

/// How a file is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// CSV, as RFC 4180 writes it.
    Csv,
    /// Tab-separated text.
    Text,
    /// The protocol's own frames, its columns then its rows, which keep
    /// every value's type (RULES.md D195).
    Native,
}

/// What `\export` writes, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    /// The query whose rows are written.
    pub query: String,
    /// The file.
    pub file: PathBuf,
    /// How it is written.
    pub form: Form,
    /// Whether its first line names the columns.
    pub header: bool,
    /// The character between fields.
    pub delimiter: char,
    /// What a null is written as.
    pub null: String,
}

/// The one character `text` is.
///
/// # Errors
///
/// It is not one character.
pub fn one_char(text: &str) -> Result<char, String> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Ok(c),
        (None, _) | (Some(_), Some(_)) => Err(format!("a delimiter is one character, not {text:?}")),
    }
}

impl Export {
    /// What `args` ask for, which is a table or a query in parentheses, a
    /// file, and options.
    ///
    /// # Errors
    ///
    /// What is wrong with them.
    pub fn parse(args: &[String]) -> Result<Export, String> {
        let [what, file, options @ ..] = args else { return Err("\\export {TABLE | (QUERY)} FILE [OPTION...]".to_owned()) };
        let query = match what.strip_prefix('(').and_then(|q| q.strip_suffix(')')) {
            Some(query) => query.trim().to_owned(),
            None => format!("TABLE {what}"),
        };
        let mut how = Export { query, file: PathBuf::from(file), form: Form::Csv, header: false, delimiter: ',', null: String::new() };
        let mut each = options.iter().map(String::as_str);
        while let Some(option) = each.next() {
            let mut value = || each.next().ok_or_else(|| format!("\\export: {option} needs a value"));
            match option {
                "csv" => (how.form, how.delimiter) = (Form::Csv, ','),
                "text" => (how.form, how.delimiter) = (Form::Text, '\t'),
                "native" => how.form = Form::Native,
                "header" => how.header = true,
                "delimiter" => how.delimiter = one_char(value()?)?,
                "null" => value()?.clone_into(&mut how.null),
                other => return Err(format!("\\export: {other} is no option")),
            }
        }
        Ok(how)
    }

    /// `value` as a field of the file.
    fn field(&self, value: &Value) -> Result<String, String> {
        if matches!(value, Value::Null) {
            return Ok(self.null.clone());
        }
        let text = value.to_string();
        match self.form {
            Form::Csv => {
                let quote = text.is_empty() || text == self.null || text.contains([self.delimiter, '"', '\r', '\n']);
                Ok(if quote { format!("\"{}\"", text.replace('"', "\"\"")) } else { text })
            }
            Form::Text => {
                if text.contains([self.delimiter, '\r', '\n']) || text == self.null {
                    return Err(format!(
                        "{text:?} holds the delimiter or a line break, or is the null text: text cannot hold it; export as csv"
                    ));
                }
                Ok(text)
            }
            // Native rows are written as frames, never as fields.
            Form::Native => Ok(text),
        }
    }
}

/// What writes the rows to the file as they come.
struct Writer<'e> {
    how: &'e Export,
    out: BufWriter<File>,
    /// The first failure. Nothing more is written after it.
    failed: Option<String>,
}

impl Writer<'_> {
    /// Writes the line of `fields`, unless writing failed.
    fn line(&mut self, fields: Result<Vec<String>, String>) {
        if self.failed.is_some() {
            return;
        }
        let delimiter = self.how.delimiter.to_string();
        let written = fields.and_then(|f| writeln!(self.out, "{}", f.join(&delimiter)).map_err(|e| e.to_string()));
        self.failed = written.err();
    }
}

impl Sink for Writer<'_> {
    fn columns(&mut self, columns: &[Column]) {
        if self.how.header {
            let names = columns.iter().map(|c| self.how.field(&Value::Char(c.name.clone().unwrap_or_default().into()))).collect();
            self.line(names);
        }
    }

    fn rows(&mut self, rows: &[Row]) {
        for row in rows {
            let fields = row.iter().map(|v| self.how.field(v)).collect();
            self.line(fields);
        }
    }
}

/// Writes the rows `how` asks for over `connection` and returns how many.
///
/// # Errors
///
/// The query fails, gives no rows, or the file cannot be written.
pub fn export<S: Statements + ?Sized>(connection: &mut S, how: &Export) -> Result<u64, String> {
    let file = File::create(&how.file).map_err(|e| format!("cannot write {}: {e}", how.file.display()))?;
    if how.form == Form::Native {
        let mut out = BufWriter::new(file);
        return connection.export_native(&how.query, &mut out).map_err(|f| f.to_string());
    }
    let mut writer = Writer { how, out: BufWriter::new(file), failed: None };
    let streamed = connection.query_into(&how.query, &mut writer).map_err(|f| f.to_string())?;
    if let Some(e) = writer.failed {
        return Err(e);
    }
    writer.out.flush().map_err(|e: io::Error| e.to_string())?;
    match streamed {
        Streamed::Rows(count) => Ok(count),
        Streamed::Done(_) => Err("the statement gave no rows".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_owned).collect()
    }

    /// Verifies: [DESIGN] RULES.md D216: a table is exported by TABLE, a
    /// query in parentheses as written; options set the form, header,
    /// delimiter and null; wrong ones are refused.
    #[test]
    fn exports_are_read_from_their_arguments() {
        let how = Export::parse(&args("STAFF out.csv header null - delimiter ;")).unwrap();
        assert_eq!((how.query.as_str(), how.header, how.null.as_str(), how.delimiter), ("TABLE STAFF", true, "-", ';'));
        let query = Export::parse(&["(SELECT 1)".to_owned(), "o".to_owned(), "text".to_owned()]).unwrap();
        assert_eq!((query.query.as_str(), query.form, query.delimiter), ("SELECT 1", Form::Text, '\t'));
        assert_eq!(Export::parse(&args("T f native")).unwrap().form, Form::Native);
        assert!(Export::parse(&args("T")).is_err());
        assert!(Export::parse(&args("T f nope")).is_err());
        assert!(Export::parse(&args("T f delimiter")).is_err());
        assert!(Export::parse(&args("T f delimiter ab")).is_err());
    }

    /// Verifies: [DESIGN] RULES.md D216; [RFC 4180 §2] a CSV field is
    /// quoted if it holds the delimiter, a quote or a line break, is empty
    /// or is the null text, so it reads back as written; a null is the
    /// null text unquoted; text refuses a value it cannot hold.
    #[test]
    fn fields_read_back_as_written() {
        let csv = Export::parse(&args("T f")).unwrap();
        let field = |how: &Export, v: &str| how.field(&Value::Char(v.into()));
        assert_eq!(csv.field(&Value::Null).unwrap(), "");
        assert_eq!(field(&csv, "").unwrap(), "\"\"");
        assert_eq!(field(&csv, "a,\"b\"").unwrap(), "\"a,\"\"b\"\"\"");
        assert_eq!(field(&csv, "plain").unwrap(), "plain");
        let dashed = Export::parse(&args("T f null -")).unwrap();
        assert_eq!(field(&dashed, "").unwrap(), "\"\"", "the empty string is quoted, whatever the null text");
        let text = Export::parse(&args("T f text null N")).unwrap();
        assert_eq!(field(&text, "a b").unwrap(), "a b");
        assert!(field(&text, "a\tb").is_err());
        assert!(field(&text, "N").is_err());
        assert_eq!(text.field(&Value::Null).unwrap(), "N");
    }
}
