//! \[DESIGN\] The client's backslash commands (RULES.md D210). A `\` in
//! code, outside a literal, an identifier or a comment, begins one, which
//! runs to the end of its line (`split.rs`). Its name is the word after
//! the `\`; its arguments are words apart by spaces, a word in single
//! quotes taken as written but for its doubled quotes, one in double
//! quotes kept with them, as an identifier is, and a variable's
//! reference read as in a statement (`vars.rs`).

use std::path::PathBuf;
use std::time::Duration;

use crate::vars::{self, Variables, identifier};

/// The thousandths of a second in a second, for `\watch`'s interval.
const MILLIS_PER_SECOND: u64 = 1000;
/// The digits of a second's fraction `\watch` reads.
const FRACTION_DIGITS: usize = 3;
/// What begins an argument naming a program, not a file.
pub const PIPE: char = '|';

/// Where results go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Where they go already.
    Default,
    /// To the file, written anew.
    File(PathBuf),
    /// To the standard input of the program, its words given.
    Program(Vec<String>),
}

/// How `\x` sets expanded output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expansion {
    /// On if off, else off.
    Toggle,
    /// On or off.
    Set(bool),
    /// On for a result whose rows are wider than the screen.
    Auto,
}

/// A command, its arguments read.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `\q` ends the run.
    Quit,
    /// `\?` lists the commands.
    Commands,
    /// `\h [STATEMENT]` shows a statement's syntax, or lists the statements.
    Syntax(Option<String>),
    /// `\set [NAME [VALUE...]]` lists the variables, or sets one.
    Set(Option<(String, String)>),
    /// `\unset NAME`.
    Unset(String),
    /// `\if VALUE`.
    If(String),
    /// `\elif VALUE`.
    Elif(String),
    /// `\else`.
    Else,
    /// `\endif`.
    Endif,
    /// `\echo TEXT...` writes the text.
    Echo(String),
    /// `\i FILE` and `\ir FILE` run a file's statements, the second's
    /// path from the directory of the file it is in.
    Include {
        /// The file.
        path: PathBuf,
        /// Whether its path is from the directory of the current file.
        relative: bool,
    },
    /// `\o [FILE | |PROGRAM]` sets where results go from now on.
    Output(Target),
    /// `\g [FILE | |PROGRAM]` and `\G` run the statement begun, or the
    /// last, its results sent to the target, expanded for `\G`.
    Go {
        /// Where its results go.
        target: Target,
        /// Whether its results are expanded.
        expanded: bool,
    },
    /// `\gset [PREFIX]` stores a query's one row, each column a variable.
    Gset(String),
    /// `\gexec` runs each value of a query's result as a statement.
    Gexec,
    /// `\watch [SECONDS [COUNT]]` runs the statement again and again.
    Watch {
        /// The interval between runs.
        every: Duration,
        /// How many runs, if limited.
        count: Option<u64>,
    },
    /// `\e [FILE]` edits the statement begun, or a file.
    Edit(Option<PathBuf>),
    /// `\p` writes the statement begun.
    Print,
    /// `\r` throws the statement begun away.
    Reset,
    /// `\x [on | off | auto]`.
    Expanded(Expansion),
    /// `\a` makes output aligned if unaligned, else unaligned.
    Aligned,
    /// `\t [on | off]` writes the rows alone, without header or footer.
    Tuples(Option<bool>),
    /// `\format [OPTION [VALUE]]` sets how results are written.
    Format(Option<(String, Option<String>)>),
    /// A command of the describe family (`describe.rs`), its name and
    /// pattern.
    Describe(String, Option<String>),
    /// `\import TABLE FILE [OPTION...]`.
    Import(Vec<String>),
    /// `\export {TABLE | (QUERY)} FILE [OPTION...]`.
    Export(Vec<String>),
    /// `\conninfo` shows the connection, its server and user.
    ConnInfo,
    /// `\c [SERVER [USER]]` connects anew, the old connection ended.
    Connect(Option<String>, Option<String>),
    /// `\password [USER]` sets a new password, its verifier made here.
    Password(Option<String>),
    /// `\tee FILE` copies everything written to the file too.
    Tee(PathBuf),
    /// `\notee` ends the copy.
    NoTee,
    /// `\delimiter [TEXT]` sets what ends a statement, a semicolon if none.
    Delimiter(Option<String>),
}

/// The name of the command in `text`, the line after its `\`, and the
/// rest of the line.
pub fn split_name(text: &str) -> (&str, &str) {
    let text = text.trim_end_matches(['\r', '\n']);
    text.split_once(char::is_whitespace).map_or((text, ""), |(name, rest)| (name, rest.trim_start()))
}

/// The arguments of `rest`, variables read from `vars`.
///
/// # Errors
///
/// A quote never closed.
pub fn arguments(rest: &str, vars: &Variables) -> Result<Vec<String>, String> {
    let chars: Vec<char> = rest.chars().collect();
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut at = 0_usize;
    while let Some(&c) = chars.get(at) {
        if c.is_whitespace() {
            words.extend(word.take());
            at = at.saturating_add(1);
            continue;
        }
        let (text, next) = piece(&chars, at, vars)?;
        word.get_or_insert_with(String::new).push_str(&text);
        at = next;
    }
    words.extend(word);
    Ok(words)
}

/// The text of the piece of a word at `at` in `chars`, and where the next
/// begins.
fn piece(chars: &[char], at: usize, vars: &Variables) -> Result<(String, usize), String> {
    let after = at.saturating_add(1);
    match chars.get(at).copied() {
        Some('\'') => quoted(chars, after, '\''),
        Some('"') => quoted(chars, after, '"').map(|(body, next)| (identifier(&body), next)),
        Some(':') => Ok(vars::reference(chars, at, vars).unwrap_or_else(|| (":".to_owned(), after))),
        Some(c) => Ok((c.to_string(), after)),
        None => Ok((String::new(), at)),
    }
}

/// The body of a quoted piece beginning at `from`, its quote `quote`
/// doubled in it, and where after its closing quote.
fn quoted(chars: &[char], from: usize, quote: char) -> Result<(String, usize), String> {
    let mut body = String::new();
    let mut at = from;
    loop {
        let Some(c) = chars.get(at).copied() else { return Err(format!("a {quote} is not closed")) };
        at = at.saturating_add(1);
        if c != quote {
            body.push(c);
            continue;
        }
        if chars.get(at) != Some(&quote) {
            return Ok((body, at));
        }
        body.push(quote);
        at = at.saturating_add(1);
    }
}

/// The target `args` name, which is none, a file, or `|` and a program's
/// words.
pub fn target(args: &[String]) -> Target {
    let joined = args.join(" ");
    if let Some(program) = joined.strip_prefix(PIPE) {
        return Target::Program(program.split_whitespace().map(str::to_owned).collect());
    }
    args.first().map_or(Target::Default, |file| Target::File(PathBuf::from(file)))
}

/// The interval `text` names in seconds, with at most three digits of
/// fraction.
///
/// # Errors
///
/// It names none.
pub fn seconds(text: &str) -> Result<Duration, String> {
    let wrong = || format!("{text} is no number of seconds");
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > FRACTION_DIGITS || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return Err(wrong());
    }
    let secs: u64 = if whole.is_empty() { 0 } else { whole.parse().map_err(|_| wrong())? };
    let padded = format!("{fraction:0<FRACTION_DIGITS$}");
    let millis: u64 = padded.parse().map_err(|_| wrong())?;
    Ok(Duration::from_secs(secs).saturating_add(Duration::from_millis(millis.min(MILLIS_PER_SECOND))))
}

#[cfg(test)]
mod tests;

mod parse;
pub use parse::{AUTO, parse};
