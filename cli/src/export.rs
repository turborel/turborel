//! \[DESIGN\] `\export` (RULES.md D216, D975) writes a table's or a query's
//! rows, streamed from the server as they come, to a file as CSV (RFC
//! 4180) or as tab-separated text, so `\import` and `--import` read them
//! back as they were. In CSV a null is written as the null text, unquoted,
//! and a value that would read back as a null, the empty string among
//! them, is quoted; in text a value cannot hold a tab or a line break.
//! `native` writes the protocol's own frames instead, every value with
//! its type, as `turborel_client::export::read_native` reads them back.
//!
//! The client library writes them (`Statements::export`), by its layouts
//! (`turborel_client::export`): a large object is written as its value, a
//! character one as its text and a binary one, as every binary string, in
//! hexits, which `\import` reads back, read a chunk at a time as it is
//! written.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use turborel_client::export::{Delimited, Layout, Native, Quoting};

use crate::run::Statements;

/// What ends a line of the file, CSV's among them, as the other files the
/// command-line client writes end theirs.
const LF: &str = "\n";

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

    /// The layout its delimited forms are written by.
    fn delimited(&self) -> Delimited {
        let quoting = if self.form == Form::Text { Quoting::Refused } else { Quoting::Quoted };
        Delimited { quoting, header: self.header, delimiter: self.delimiter, null: self.null.clone(), line_end: LF.to_owned() }
    }
}

/// Writes the rows `how` asks for over `connection` and returns how many.
///
/// # Errors
///
/// The query fails, is no query, or the file cannot be written.
pub fn export<S: Statements + ?Sized>(connection: &mut S, how: &Export) -> Result<u64, String> {
    let file = File::create(&how.file).map_err(|e| format!("cannot write {}: {e}", how.file.display()))?;
    let mut out = BufWriter::new(file);
    let (mut native, mut delimited) = (Native, how.delimited());
    let layout: &mut dyn Layout = if how.form == Form::Native { &mut native } else { &mut delimited };
    connection.export(&how.query, &mut out, layout).map_err(|f| f.to_string())
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

    /// Verifies: [DESIGN] RULES.md D216, D975; [RFC 4180 §2] a CSV field
    /// is quoted if it holds the delimiter, a quote or a line break, is
    /// empty or is the null text, so it reads back as written; text refuses
    /// a value it cannot hold; each line ends with LF.
    #[test]
    fn fields_read_back_as_written() {
        let field = |how: &Export, v: &str| {
            let mut buf = Vec::new();
            how.delimited().text_field(&mut buf, v).map(|()| String::from_utf8(buf).unwrap())
        };
        let csv = Export::parse(&args("T f")).unwrap();
        assert_eq!(field(&csv, "").unwrap(), "\"\"");
        assert_eq!(field(&csv, "a,\"b\"").unwrap(), "\"a,\"\"b\"\"\"");
        assert_eq!(field(&csv, "plain").unwrap(), "plain");
        assert_eq!(csv.delimited().line_end, "\n");
        let dashed = Export::parse(&args("T f null -")).unwrap();
        assert_eq!(field(&dashed, "").unwrap(), "\"\"", "the empty string is quoted, whatever the null text");
        assert_eq!(field(&dashed, "-").unwrap(), "\"-\"", "as is the null text");
        let text = Export::parse(&args("T f text null N")).unwrap();
        assert_eq!(field(&text, "a b").unwrap(), "a b");
        assert!(field(&text, "a\tb").is_err());
        assert!(field(&text, "N").is_err());
        assert_eq!(text.delimited().null, "N");
    }
}
