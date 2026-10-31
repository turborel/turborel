//! \[DESIGN\] A configuration file, `turboreld --config FILE`: a source of
//! the database's settings as the program starts, as `--set` is (RULES.md
//! D36, D42). It is no configuration of its own: each key names a
//! setting, each value is the setting's, and the settings it gives are set
//! as `--set` sets them, those read as the database opens before it
//! opens, the others once the server is made, kept with the database. One
//! `--set` gives too is set as `--set` gives it: the command line wins.
//! The program's options (`--dir`, `--log`, `--keys` and the rest) say
//! where the database, its diagnostics and its keys are, not how the
//! database behaves, so they stay on the command line.
//!
//! The file is in a subset of TOML 1.0: a file read here means what TOML
//! means by it; what TOML has beyond the subset is refused, its line and
//! column said. The subset, in the notation of ISO/IEC 9075:1992:
//!
//! ```text
//! <configuration file> ::= [ <line> [ { <newline> <line> }... ] ]
//! <line> ::= [ <blank> ] [ <table header> | <key value pair> ] [ <blank> ] [ <comment> ]
//! <table header> ::= <left bracket> [ <blank> ] <key> [ <blank> ] <right bracket>
//! <key value pair> ::= <key> [ <blank> ] <equals operator> [ <blank> ] <value>
//! <key> ::= <simple key> [ { [ <blank> ] <period> [ <blank> ] <simple key> }... ]
//! <simple key> ::= <bare key> | <basic string> | <literal string>
//! <bare key> ::= { <ASCII letter> | <digit> | <underscore> | <minus sign> }...
//! <value> ::= <basic string> | <literal string> | <integer> | <float> | <boolean> | <array>
//! <basic string> ::= <double quote> [ { <basic character> | <escape> }... ] <double quote>
//! <escape> ::= <reverse solidus>
//!       { b | t | n | f | r | <double quote> | <reverse solidus>
//!       | u <hex digit>4 | U <hex digit>8 }
//! <literal string> ::= <quote> [ <literal character>... ] <quote>
//! <integer> ::= [ <sign> ] <whole digits>
//! <whole digits> ::= 0 | <nonzero digit> [ { [ <underscore> ] <digit> }... ]
//! <float> ::= <integer> { <fraction> [ <exponent> ] | <exponent> }
//! <fraction> ::= <period> <digits>
//! <exponent> ::= { e | E } [ <sign> ] <digits>
//! <digits> ::= <digit> [ { [ <underscore> ] <digit> }... ]
//! <boolean> ::= true | false
//! <array> ::= <left bracket> <array blank>
//!       [ <string> [ { <array blank> <comma> <array blank> <string> }... ]
//!       <array blank> [ <comma> <array blank> ] ] <right bracket>
//! <string> ::= <basic string> | <literal string>
//! <array blank> ::= [ { <space> | <tab> | <newline> | <comment> }... ]
//! <blank> ::= { <space> | <tab> }...
//! <comment> ::= <number sign> [ <comment character>... ]
//! <newline> ::= <line feed> | <carriage return> <line feed>
//! ```
//!
//! A `<basic character>` is any character but a double quote, a reverse
//! solidus and a control character other than tab; a `<literal
//! character>` any but a quote and such a control character; a `<comment
//! character>` any but such a control character and a newline. A string
//! is on one line. A `<hex digit>4` is four hexadecimal digits, of a
//! Unicode scalar value. An integer is of 64 bits, a float finite. A
//! leading byte order mark is passed over.
//!
//! A setting's name is the parts of the last `<table header>`'s key and
//! of the pair's key, joined by periods: `PORT = 7422` under `[SERVER]` is
//! the setting SERVER.PORT, as `SERVER.PORT = 7422` is anywhere before
//! the first header. A name is in any case, as setting names are. Values:
//! a string is a character string; an integer an exact whole number; a
//! float an approximate number; `true` and `false` the words ON and OFF
//! of a switch; an array of strings those strings joined by commas, as a
//! list setting takes them (SERVER.HOST, STORAGE.ENCODINGS), none holding
//! a comma itself. A setting given twice, or a header, is refused. Setting
//! names are data here: each is checked against the settings declared
//! (`given`), so the parser knows of none.
//!
//! Refused, beyond the subset: multi-line strings, integers other than
//! decimal, infinite or not-a-number floats, dates and times, arrays of
//! anything but strings, inline tables, and arrays of tables.

use std::fmt;
use std::path::Path;

use turborel::db::Database;
use turborel::value::Value;

use crate::Config;

/// A byte order mark, passed over at the start of a file (TOML 1.0).
const BYTE_ORDER_MARK: char = '\u{feff}';

/// Where in a file: its line and column, each from 1, a column counted in
/// characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct At {
    /// The line.
    pub line: usize,
    /// The column.
    pub column: usize,
}

/// A setting a file sets: its name, the parts of its header's key and its
/// own key joined by periods, as written; its value; where its key begins.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The setting's name.
    pub name: String,
    /// Its value.
    pub value: Value,
    /// Where its key begins.
    pub at: At,
}

/// What is wrong with a file, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// Where.
    pub at: At,
    /// What, for the operator.
    pub message: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.at.line, self.at.column, self.message)
    }
}

/// The settings the configuration file `text` sets, in the order it sets
/// them.
///
/// # Errors
///
/// What is not of the subset, where it begins: a key, a value, a setting
/// or header given twice, or anything after a line's pair or header but a
/// comment.
pub fn read(text: &str) -> Result<Vec<Entry>, Refusal> {
    let mut reader = Reader::new(text);
    let mut file = File::default();
    while reader.peek().is_some() {
        reader.line(&mut file)?;
    }
    Ok(file.entries)
}

/// A setting to set as the program starts: its name and value, and, if a
/// file gave it, where (`FILE:LINE:COLUMN`).
#[derive(Debug, Clone)]
pub(crate) struct Given {
    /// The setting's name.
    pub(crate) name: String,
    /// Its value.
    pub(crate) value: Value,
    /// Where a file gave it; none for `--set`.
    pub(crate) from: Option<String>,
}

impl Given {
    /// Sets it as the database's value of its setting in `db`.
    ///
    /// # Errors
    ///
    /// It cannot be set, said with where it was given.
    pub(crate) fn set(&self, db: &Database) -> Result<(), String> {
        let name = &self.name;
        db.set_setting(name, &self.value).map_err(|e| match &self.from {
            Some(from) => format!("{from}: cannot set {name}: {e}"),
            None => format!("cannot set {name}: {e}"),
        })
    }
}

/// The settings of `config`'s file, `--config`, if any, read and checked
/// as its grammar says (`read`).
///
/// # Errors
///
/// The file cannot be read, or is not of the subset, said with its path
/// and where.
pub(crate) fn entries(config: &Config) -> Result<Vec<Entry>, String> {
    let Some(path) = &config.file else { return Ok(Vec::new()) };
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    read(&text).map_err(|refusal| format!("{}:{refusal}", path.display()))
}

/// The settings `config` gives to set as the program starts: those of its
/// file, its `entries`, each of the settings `db` declares or a server
/// declares, but those `--set` gives too; then those `--set` gives, so the
/// command line wins.
///
/// # Errors
///
/// An entry names no setting, said with the file's path and the line.
pub(crate) fn given(db: &Database, config: &Config, entries: Vec<Entry>) -> Result<Vec<Given>, String> {
    let path = config.file.as_deref().unwrap_or_else(|| Path::new(""));
    let set = &config.settings;
    let known: Vec<String> = db
        .settings()
        .unwrap_or_default()
        .into_iter()
        .map(|l| l.definition.name)
        .chain(turborel_server::settings::definitions().into_iter().map(|d| d.name))
        .collect();
    let mut given = Vec::new();
    for Entry { name, value, at } in entries {
        let from = format!("{}:{}:{}", path.display(), at.line, at.column);
        if !known.iter().any(|k| k.eq_ignore_ascii_case(&name)) {
            return Err(format!("{from}: no setting is named {name}"));
        }
        if !set.iter().any(|(n, _)| n.trim().eq_ignore_ascii_case(&name)) {
            given.push(Given { name, value, from: Some(from) });
        }
    }
    given.extend(set.iter().map(|(name, value)| Given { name: name.clone(), value: value.clone(), from: None }));
    Ok(given)
}

/// The settings and headers a file has given so far, and the header now.
#[derive(Default)]
struct File {
    /// The parts of the last header's key; none before the first.
    table: Vec<String>,
    /// Each header's name, and where it is.
    headers: Vec<(String, At)>,
    /// The settings set.
    entries: Vec<Entry>,
}

impl File {
    /// The table `parts` names begins, its header at `at`.
    fn header(&mut self, parts: Vec<String>, at: At) -> Result<(), Refusal> {
        let name = parts.join(".");
        if let Some((_, first)) = self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            return Err(Refusal { at, message: format!("the table [{name}] is given twice, first at line {}", first.line) });
        }
        self.headers.push((name, at));
        self.table = parts;
        Ok(())
    }

    /// The setting `parts` names in the table now is set to `value`, its
    /// key at `at`.
    fn entry(&mut self, parts: &[String], value: Value, at: At) -> Result<(), Refusal> {
        let name = self.table.iter().chain(parts).cloned().collect::<Vec<_>>().join(".");
        if let Some(first) = self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(&name)) {
            return Err(Refusal { at, message: format!("{name} is given twice, first at line {}", first.at.line) });
        }
        self.entries.push(Entry { name, value, at });
        Ok(())
    }
}

/// A file's characters, read one at a time, where the next is known.
struct Reader {
    /// The characters.
    chars: Vec<char>,
    /// The next one's place among them.
    next: usize,
    /// Where the next one is.
    at: At,
}

impl Reader {
    /// A reader of `text`, from its start, past a byte order mark.
    fn new(text: &str) -> Reader {
        let chars = text.strip_prefix(BYTE_ORDER_MARK).unwrap_or(text).chars().collect();
        Reader { chars, next: 0, at: At { line: 1, column: 1 } }
    }

    /// The next character, if any, not taken.
    fn peek(&self) -> Option<char> {
        self.chars.get(self.next).copied()
    }

    /// The character `ahead` past the next, if any, not taken.
    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.next.saturating_add(ahead)).copied()
    }

    /// The next character, if any, taken.
    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.next = self.next.saturating_add(1);
        if c == '\n' {
            self.at = At { line: self.at.line.saturating_add(1), column: 1 };
        } else {
            self.at.column = self.at.column.saturating_add(1);
        }
        Some(c)
    }

    /// Spaces and tabs, taken.
    fn blanks(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.bump();
        }
    }

    /// `expected`, taken, or else `message` refusing what is there.
    fn expect(&mut self, expected: char, message: &str) -> Result<(), Refusal> {
        if self.peek() != Some(expected) {
            return refuse(self.at, message);
        }
        self.bump();
        Ok(())
    }

    /// A `<line>`, through its newline, if it has one.
    fn line(&mut self, file: &mut File) -> Result<(), Refusal> {
        self.blanks();
        match self.peek() {
            Some('[') => self.header(file)?,
            Some('#' | '\n' | '\r') | None => {}
            Some(_) => self.pair(file)?,
        }
        self.end_of_line()
    }

    /// The rest of a line: blanks, a comment, and its newline or the end.
    fn end_of_line(&mut self) -> Result<(), Refusal> {
        self.blanks();
        if self.peek() == Some('#') {
            self.comment()?;
        }
        let at = self.at;
        match self.bump() {
            None | Some('\n') => Ok(()),
            Some('\r') => self.expect('\n', "a carriage return is followed by a line feed"),
            Some(c) => refuse(at, &format!("{c} follows where the line ends: only a comment, from #, may")),
        }
    }

    /// A `<comment>`, up to its newline.
    fn comment(&mut self) -> Result<(), Refusal> {
        self.bump();
        while let Some(c) = self.peek() {
            if matches!(c, '\n' | '\r') {
                break;
            }
            character(c, self.at)?;
            self.bump();
        }
        Ok(())
    }

    /// A `<table header>`.
    fn header(&mut self, file: &mut File) -> Result<(), Refusal> {
        let at = self.at;
        self.bump();
        if self.peek() == Some('[') {
            return refuse(at, "arrays of tables, [[...]], are not taken");
        }
        self.blanks();
        let parts = self.key()?;
        self.blanks();
        self.expect(']', "a table's header is a key in brackets, [GROUP]")?;
        file.header(parts, at)
    }

    /// A `<key value pair>`.
    fn pair(&mut self, file: &mut File) -> Result<(), Refusal> {
        let at = self.at;
        let parts = self.key()?;
        self.blanks();
        self.expect('=', "a key is followed by = and its value")?;
        self.blanks();
        let value = self.value()?;
        file.entry(&parts, value, at)
    }

    /// A `<key>`: its parts, the blanks after it taken.
    fn key(&mut self) -> Result<Vec<String>, Refusal> {
        let mut parts = vec![self.simple_key()?];
        loop {
            self.blanks();
            if self.peek() != Some('.') {
                return Ok(parts);
            }
            self.bump();
            self.blanks();
            parts.push(self.simple_key()?);
        }
    }

    /// A `<simple key>`, not empty, as no part of a setting's name is.
    fn simple_key(&mut self) -> Result<String, Refusal> {
        let at = self.at;
        let part = match self.peek() {
            Some('"') => self.basic_string()?,
            Some('\'') => self.literal_string()?,
            Some(_) | None => self.bare_key(),
        };
        if part.is_empty() {
            return refuse(at, "a key is expected: letters, digits, _ and -, or a quoted string, not empty");
        }
        Ok(part)
    }

    /// A `<bare key>`, empty if there is none.
    fn bare_key(&mut self) -> String {
        let mut part = String::new();
        while let Some(c) = self.peek() {
            if !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) {
                break;
            }
            part.push(c);
            self.bump();
        }
        part
    }
}

/// `c`, unless it is a control character other than tab, which a string
/// or a comment does not hold as it is (TOML 1.0).
fn character(c: char, at: At) -> Result<char, Refusal> {
    if c.is_ascii_control() && c != '\t' {
        return refuse(at, &format!("U+{:04X} is a control character, not taken as it is: in a basic string, escape it", u32::from(c)));
    }
    Ok(c)
}

/// A refusal of what is `at`, saying `message`.
fn refuse<T>(at: At, message: &str) -> Result<T, Refusal> {
    Err(Refusal { at, message: message.to_owned() })
}

mod value;

// Unit tests: the subset read, and refused where it is not (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
