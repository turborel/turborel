//! \[DESIGN\] Delimited text as a `Source` (RULES.md D59): CSV as RFC
//! 4180 §2 defines it, and tab-separated text, each a record a line,
//! fields split at a delimiter. Read a line at a time from a buffered
//! reader, a record at most `max_record` bytes, so a file of any size is
//! read in bounded memory, and a quote left open is refused at the bound
//! rather than read to the end of the file.
//!
//! CSV [RFC 4180 §2]: a record ends at a line break, CR LF or LF alone,
//! the last one's optional (rules 1, 2); a header row may come first (rule
//! 3); a field may be enclosed in double quotes, and must be to hold the
//! delimiter, a quote or a line break, a quote in it written twice (rules
//! 5-7). A quote in a field not enclosed, or anything but the delimiter
//! after a closing quote, is refused. Tab-separated text has no quoting: a
//! field holds no tab and no line break.
//!
//! A field written as the dialect's text for null, not enclosed in quotes,
//! is null: by default an empty one, so `,,` is a null, and `,"",` the
//! empty string. Text is UTF-8; a byte order mark beginning the input is
//! passed over.

use std::io::{BufRead, Read};

use turborel::value::Value;

use super::{Heading, Position, Record, Source, SourceError};

#[cfg(test)]
mod tests;

/// \[DESIGN\] The longest record read by default, in bytes: the protocol's
/// longest body (`turborel_protocol::MAX_BODY`), as a longer record could
/// not be sent.
pub const DEFAULT_MAX_RECORD: usize = 16 * 1024 * 1024;

/// [RFC 4180 §2 rule 4] CSV's delimiter.
const COMMA: u8 = b',';
/// The delimiter of tab-separated text.
const TAB: u8 = b'\t';
/// [RFC 4180 §2 rules 5-7] What encloses a field.
const QUOTE: u8 = b'"';
/// [RFC 4180 §2 rule 1] What ends a line, after an optional CR.
const LF: u8 = b'\n';
/// [RFC 4180 §2 rule 1] What may come before LF at a line's end.
const CR: u8 = b'\r';
/// The UTF-8 byte order mark, which an input may begin with.
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// How delimited text is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialect {
    delimiter: u8,
    /// Whether fields may be enclosed in quotes, as CSV's are.
    quoted: bool,
    /// Whether the first record names the fields.
    header: bool,
    /// What a field not enclosed in quotes is written as to be null.
    null: String,
    /// The longest record, in bytes.
    max_record: usize,
}

impl Dialect {
    /// [RFC 4180 §2] CSV: fields split at commas, enclosed in quotes as
    /// they need; no header; an empty field null.
    pub fn csv() -> Dialect {
        Dialect { delimiter: COMMA, quoted: true, header: false, null: String::new(), max_record: DEFAULT_MAX_RECORD }
    }

    /// Tab-separated text: fields split at tabs, never quoted; no header;
    /// an empty field null.
    pub fn tsv() -> Dialect {
        Dialect { delimiter: TAB, ..Dialect::csv() }.unquoted()
    }

    /// The dialect, its fields never enclosed in quotes.
    fn unquoted(self) -> Dialect {
        Dialect { quoted: false, ..self }
    }

    /// The dialect, its fields split at `delimiter`.
    ///
    /// # Errors
    ///
    /// If `delimiter` is not one ASCII character other than a quote, CR or
    /// LF.
    pub fn with_delimiter(self, delimiter: char) -> Result<Dialect, String> {
        let byte = u8::try_from(delimiter).ok().filter(|b| b.is_ascii() && ![QUOTE, CR, LF].contains(b));
        let Some(byte) = byte else {
            return Err(format!("[DESIGN] a delimiter is one ASCII character, not a quote or a line break: {delimiter:?}"));
        };
        Ok(Dialect { delimiter: byte, ..self })
    }

    /// The dialect, its first record naming the fields [RFC 4180 §2 rule
    /// 3].
    #[must_use]
    pub fn with_header(self) -> Dialect {
        Dialect { header: true, ..self }
    }

    /// The dialect, a field written as `null`, not in quotes, null.
    #[must_use]
    pub fn with_null(self, null: &str) -> Dialect {
        Dialect { null: null.to_owned(), ..self }
    }

    /// The dialect, a record at most `bytes` long, its line break counted.
    #[must_use]
    pub fn with_max_record(self, bytes: usize) -> Dialect {
        Dialect { max_record: bytes, ..self }
    }
}

/// Where a scan of a CSV record is: so whether a line break ends it, or
/// is in a quoted field.
#[derive(Debug, Clone, Copy)]
enum Scan {
    /// At a field's beginning.
    FieldStart,
    /// In a field not quoted.
    Plain,
    /// In a quoted field.
    Quoted,
    /// After a quote in a quoted field: its end, or the first of two.
    QuoteInQuoted,
}

impl Scan {
    /// Where the scan is after `b`, fields split at `delimiter`. What is
    /// wrong, as a quote in a field not quoted, is refused as the record
    /// is split.
    fn next(self, b: u8, delimiter: u8) -> Scan {
        let (quote, split) = (b == QUOTE, b == delimiter);
        match self {
            Scan::Quoted => {
                if quote {
                    Scan::QuoteInQuoted
                } else {
                    Scan::Quoted
                }
            }
            Scan::FieldStart | Scan::QuoteInQuoted => {
                if split {
                    Scan::FieldStart
                } else if quote {
                    Scan::Quoted
                } else {
                    Scan::Plain
                }
            }
            Scan::Plain => {
                if split {
                    Scan::FieldStart
                } else {
                    Scan::Plain
                }
            }
        }
    }
}

/// A field as written: its text, and whether it was enclosed in quotes.
struct Field {
    text: String,
    quoted: bool,
}

/// Delimited text read from `R`, a record at a time.
pub struct Delimited<R: BufRead> {
    input: R,
    dialect: Dialect,
    /// The bytes of the record being read.
    bytes: Vec<u8>,
    /// How many lines have been read.
    lines: u64,
    /// How many records of data have been read.
    records: u64,
    /// Whether a header is still to be read.
    header_pending: bool,
    /// Whether anything has been read: a byte order mark begins only the
    /// first record.
    started: bool,
    /// Whether the input has ended, or cannot be read on.
    done: bool,
}

impl<R: BufRead> Delimited<R> {
    /// Delimited text read from `input`, written as `dialect` says.
    pub fn new(input: R, dialect: Dialect) -> Delimited<R> {
        let header_pending = dialect.header;
        Delimited { input, dialect, bytes: Vec::new(), lines: 0, records: 0, header_pending, started: false, done: false }
    }

    /// The next record's fields, and the line it began on; none at the
    /// end of the input. `record` is its number, for an error.
    fn fields(&mut self, record: u64) -> Result<Option<(Vec<Field>, u64)>, SourceError> {
        let line = self.lines.saturating_add(1);
        let position = Position { record, line: Some(line) };
        let read = self.raw().and_then(|more| if more { split(&self.bytes, &self.dialect).map(Some) } else { Ok(None) });
        match read {
            Ok(fields) => {
                self.done = fields.is_none();
                Ok(fields.map(|f| (f, line)))
            }
            Err(message) => {
                self.done = true;
                Err(SourceError { position, message })
            }
        }
    }

    /// Reads the next record's bytes, its line break taken off: false at
    /// the end of the input.
    fn raw(&mut self) -> Result<bool, String> {
        self.bytes.clear();
        let mut at = Scan::FieldStart;
        loop {
            let room = self.dialect.max_record.saturating_sub(self.bytes.len()).saturating_add(1);
            let start = self.bytes.len();
            let mut limited = (&mut self.input).take(u64::try_from(room).unwrap_or(u64::MAX));
            let n = limited.read_until(LF, &mut self.bytes).map_err(|e| format!("[DESIGN] cannot read: {e}"))?;
            if self.bytes.len() > self.dialect.max_record {
                return Err(format!("[DESIGN] the record is longer than {} bytes", self.dialect.max_record));
            }
            if n == 0 {
                return if matches!(at, Scan::Quoted) { Err(unclosed()) } else { Ok(!self.bytes.is_empty()) };
            }
            if self.dialect.quoted {
                let delimiter = self.dialect.delimiter;
                at = self.bytes.get(start..).unwrap_or_default().iter().fold(at, |was, b| was.next(*b, delimiter));
            }
            self.lines = self.lines.saturating_add(1);
            // A line break in a quoted field does not end the record.
            if !matches!(at, Scan::Quoted) {
                self.ended();
                return Ok(true);
            }
        }
    }

    /// Takes the record's line break off its bytes, and a byte order mark
    /// off the first's.
    fn ended(&mut self) {
        if self.bytes.last() == Some(&LF) {
            self.bytes.pop();
            if self.bytes.last() == Some(&CR) {
                self.bytes.pop();
            }
        }
        if !self.started && self.bytes.starts_with(BOM) {
            self.bytes.drain(..BOM.len());
        }
        self.started = true;
    }

    /// Reads the header, if one is still to be read.
    fn header(&mut self) -> Result<Option<Vec<Heading>>, SourceError> {
        if !self.header_pending {
            return Ok(None);
        }
        self.header_pending = false;
        let Some((fields, _)) = self.fields(0)? else {
            return Err(SourceError { position: Position { record: 0, line: Some(1) }, message: "[DESIGN] there is no header".to_owned() });
        };
        Ok(Some(fields.into_iter().map(|f| Heading { name: f.text, data_type: None }).collect()))
    }
}

/// The message of a quoted field not closed by the end of the input.
fn unclosed() -> String {
    "[RFC 4180 §2 rule 5] a quoted field is not closed".to_owned()
}

/// The fields of the record `bytes`, written as `dialect` says.
fn split(bytes: &[u8], dialect: &Dialect) -> Result<Vec<Field>, String> {
    if !dialect.quoted {
        let texts = bytes.split(|b| *b == dialect.delimiter).enumerate();
        return texts.map(|(n, b)| text(b.to_vec(), n).map(|text| Field { text, quoted: false })).collect();
    }
    let mut fields = Vec::new();
    let mut rest = bytes.iter().copied().peekable();
    loop {
        let n = fields.len();
        let quoted = rest.next_if_eq(&QUOTE).is_some();
        let written = if quoted { enclosed(&mut rest, n)? } else { plain(&mut rest, dialect.delimiter, n)? };
        fields.push(Field { text: text(written, n)?, quoted });
        let Some(next) = rest.next() else { return Ok(fields) };
        if next != dialect.delimiter {
            return Err(format!("[RFC 4180 §2 rule 5] field {}: a quoted field's closing quote is followed by more", n.saturating_add(1)));
        }
    }
}

/// The bytes of a field not enclosed in quotes, to the next delimiter;
/// `n` its index, for an error.
fn plain(rest: &mut std::iter::Peekable<impl Iterator<Item = u8>>, delimiter: u8, n: usize) -> Result<Vec<u8>, String> {
    let mut written = Vec::new();
    while let Some(b) = rest.next_if(|b| *b != delimiter) {
        if b == QUOTE {
            return Err(format!("[RFC 4180 §2 rule 5] field {}: a quote in a field not enclosed in quotes", n.saturating_add(1)));
        }
        written.push(b);
    }
    Ok(written)
}

/// The bytes of a field enclosed in quotes, its opening quote read, to its
/// closing quote, a quote written twice in it one [RFC 4180 §2 rule 7];
/// `n` its index, for an error.
fn enclosed(rest: &mut std::iter::Peekable<impl Iterator<Item = u8>>, n: usize) -> Result<Vec<u8>, String> {
    let mut written = Vec::new();
    loop {
        let Some(b) = rest.next() else { return Err(format!("field {}: {}", n.saturating_add(1), unclosed())) };
        if b == QUOTE && rest.next_if_eq(&QUOTE).is_none() {
            return Ok(written);
        }
        written.push(b);
    }
}

/// `bytes` as text, of the field of index `n`.
fn text(bytes: Vec<u8>, n: usize) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| format!("[DESIGN] field {} is not UTF-8", n.saturating_add(1)))
}

impl<R: BufRead> Source for Delimited<R> {
    fn headings(&mut self) -> Result<Option<Vec<Heading>>, SourceError> {
        self.header()
    }

    fn record(&mut self) -> Result<Option<Record>, SourceError> {
        self.header()?;
        if self.done {
            return Ok(None);
        }
        let number = self.records.saturating_add(1);
        let Some((fields, line)) = self.fields(number)? else { return Ok(None) };
        self.records = number;
        let null = &self.dialect.null;
        let values = fields.into_iter().map(|f| if !f.quoted && f.text == *null { Value::Null } else { Value::Char(f.text) }).collect();
        Ok(Some(Record { values, position: Position { record: number, line: Some(line) } }))
    }
}
