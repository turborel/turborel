//! \[DESIGN\] Delimited text (RULES.md D195, D975): a record for each row,
//! its fields split by a delimiter, as CSV [RFC 4180] writes them or as
//! tab-separated text. A null is written as the null text, unquoted. In
//! CSV a field is quoted where it holds the delimiter, a quote, a line
//! break or spaces at either end, is empty or is the null text, a quote
//! in it written twice, so it reads back as written. Text cannot hold the
//! delimiter or a line break, nor the null text, and refuses them.
//!
//! A character large object is written as its text. A binary string, a
//! binary large object among them, is written as its octets in hexits, two
//! to each, upper case, as a `<binary string literal>` writes them between
//! its quotes [ISO/IEC 9075-2:2023 §5.3], which an import reads back. A
//! large object sent by its locator is written a chunk at a time as it is
//! read, in quotes in CSV whatever it holds, as it cannot be known ahead.

use std::borrow::Cow;
use std::io::{self, Write};

use turborel::storage::Row;
use turborel::value::Value;
use turborel::value::lob::{Lob, LobKind, char_cut};
use turborel_protocol::Column;

use super::{Layout, Lobs, holds_located, located, read_whole};

/// [RFC 4180 §2 rule 5] What encloses a field in CSV.
const QUOTE: char = '"';
/// [RFC 4180 §2 rule 7] A quote within a field, as it is written.
const QUOTE_TWICE: &str = "\"\"";
/// [RFC 4180 §2 rule 1] What ends a CSV record.
const CRLF: &str = "\r\n";
/// What ends a line of text.
const LF: &str = "\n";
/// [RFC 4180 §2 rule 4] CSV's delimiter.
const COMMA: char = ',';
/// Text's delimiter.
const TAB: char = '\t';
/// [ISO/IEC 9075-2:2023 §5.3] The hexits of a `<binary string literal>`,
/// by their values.
const HEXITS: &[u8; 16] = b"0123456789ABCDEF";

/// How a delimited layout writes a field that holds what would end it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quoting {
    /// [RFC 4180 §2 rules 5-7] In quotes, a quote in it written twice: CSV.
    Quoted,
    /// Never quoted, so a field that holds the delimiter, a line break or
    /// the null text is refused: tab-separated text.
    Refused,
}

/// \[DESIGN\] RULES.md D975: rows as delimited text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delimited {
    /// How a field that holds the delimiter is written.
    pub quoting: Quoting,
    /// Whether a header of the columns' names comes first.
    pub header: bool,
    /// The character between fields.
    pub delimiter: char,
    /// What a null is written as.
    pub null: String,
    /// What ends a record.
    pub line_end: String,
}

impl Delimited {
    /// CSV [RFC 4180]: a header, commas, a null empty, records ended by CR
    /// LF.
    pub fn csv() -> Delimited {
        Delimited { quoting: Quoting::Quoted, header: true, delimiter: COMMA, null: String::new(), line_end: CRLF.to_owned() }
    }

    /// Tab-separated text: no header, tabs, a null empty, lines ended by
    /// LF.
    pub fn text() -> Delimited {
        Delimited { quoting: Quoting::Refused, header: false, delimiter: TAB, null: String::new(), line_end: LF.to_owned() }
    }

    /// Writes `text`, a field's, to `buf`, quoted as it must be.
    ///
    /// # Errors
    ///
    /// `InvalidData` if text cannot hold it.
    pub fn text_field(&self, buf: &mut Vec<u8>, text: &str) -> io::Result<()> {
        let breaks = text.contains([self.delimiter, '\r', '\n']);
        match self.quoting {
            Quoting::Quoted => {
                let bare = text.contains(QUOTE) || text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace);
                if text.is_empty() || text == self.null || breaks || bare {
                    buf.push(b'"');
                    buf.extend_from_slice(text.replace(QUOTE, QUOTE_TWICE).as_bytes());
                    buf.push(b'"');
                } else {
                    buf.extend_from_slice(text.as_bytes());
                }
                Ok(())
            }
            Quoting::Refused => {
                if breaks || text == self.null {
                    return Err(unheld(text));
                }
                buf.extend_from_slice(text.as_bytes());
                Ok(())
            }
        }
    }

    /// Writes `v`, a field's value, to `buf`, or, a large object sent by its
    /// locator, to `out` as it is read, what `buf` holds written first.
    fn field(&self, (out, buf): (&mut dyn Write, &mut Vec<u8>), v: &Value, lobs: &mut dyn Lobs) -> io::Result<()> {
        if let Value::Lob(l) = v
            && located(l)
        {
            out.write_all(buf)?;
            buf.clear();
            return self.streamed(out, l, lobs);
        }
        if v.is_null() {
            buf.extend_from_slice(self.null.as_bytes());
            return Ok(());
        }
        let whole = if holds_located(v) { Cow::Owned(read_whole(v, lobs)?) } else { Cow::Borrowed(v) };
        let text = if let Value::Char(s) = &*whole { Cow::Borrowed(&**s) } else { Cow::Owned(whole.to_string()) };
        self.text_field(buf, &text)
    }

    /// Writes `lob`, sent by its locator, to `out` a chunk at a time as
    /// `lobs` reads it: in hexits if binary; else its text, in quotes in
    /// CSV, each chunk cut where a character ends, the rest of one kept for
    /// the next.
    fn streamed(&self, out: &mut dyn Write, lob: &Lob, lobs: &mut dyn Lobs) -> io::Result<()> {
        if lob.kind() == LobKind::Binary {
            return lobs.chunks(lob, &mut |chunk| out.write_all(&hexits(chunk)));
        }
        let quoted = self.quoting == Quoting::Quoted;
        if quoted {
            out.write_all(b"\"")?;
        }
        let mut carried = Vec::new();
        lobs.chunks(lob, &mut |chunk| {
            carried.extend_from_slice(chunk);
            let cut = char_cut(&carried, carried.len());
            let rest = carried.split_off(cut);
            let whole = std::mem::replace(&mut carried, rest);
            let text = std::str::from_utf8(&whole).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            if !quoted && text.contains([self.delimiter, '\r', '\n']) {
                return Err(unheld(text));
            }
            out.write_all(if quoted { Cow::Owned(text.replace(QUOTE, QUOTE_TWICE)) } else { Cow::Borrowed(text) }.as_bytes())
        })?;
        if !carried.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a character large object ends within a character"));
        }
        if quoted { out.write_all(b"\"") } else { Ok(()) }
    }

    /// Writes `fields`' texts as a record to `buf`.
    fn record<'t>(&self, buf: &mut Vec<u8>, fields: impl Iterator<Item = &'t str>) -> io::Result<()> {
        for (at, text) in fields.enumerate() {
            if at > 0 {
                buf.extend_from_slice(self.delimiter.encode_utf8(&mut [0; 4]).as_bytes());
            }
            self.text_field(buf, text)?;
        }
        buf.extend_from_slice(self.line_end.as_bytes());
        Ok(())
    }
}

/// The error of text that cannot hold `text`.
fn unheld(text: &str) -> io::Error {
    let shown: String = text.chars().take(SHOWN_CHARS).collect();
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{shown:?} holds the delimiter or a line break, or is the null text: text cannot hold it; export as csv"),
    )
}

/// \[DESIGN\] The most characters of a field an error shows.
const SHOWN_CHARS: usize = 64;

/// `bytes` in hexits, two to each, upper case.
pub(crate) fn hexits(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        out.push(HEXITS.get(usize::from(b >> 4_u8)).copied().unwrap_or(b'0'));
        out.push(HEXITS.get(usize::from(b & 0x0F)).copied().unwrap_or(b'0'));
    }
    out
}

impl Layout for Delimited {
    fn columns(&mut self, out: &mut dyn Write, columns: &[Column]) -> io::Result<()> {
        if !self.header {
            return Ok(());
        }
        let mut buf = Vec::new();
        self.record(&mut buf, columns.iter().map(|c| c.name.as_deref().unwrap_or_default()))?;
        out.write_all(&buf)
    }

    fn rows(&mut self, out: &mut dyn Write, rows: &[Row], lobs: &mut dyn Lobs) -> io::Result<()> {
        let mut buf = Vec::new();
        let delimiter = self.delimiter.encode_utf8(&mut [0; 4]).as_bytes().to_vec();
        for row in rows {
            for (at, v) in row.iter().enumerate() {
                if at > 0 {
                    buf.extend_from_slice(&delimiter);
                }
                self.field((out, &mut buf), v, lobs)?;
            }
            buf.extend_from_slice(self.line_end.as_bytes());
        }
        out.write_all(&buf)
    }

    fn end(&mut self, _: &mut dyn Write) -> io::Result<()> {
        Ok(())
    }
}
