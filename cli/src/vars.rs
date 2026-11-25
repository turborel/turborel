//! \[DESIGN\] The client's variables (RULES.md D212), names set and unset
//! by `\set` and `\unset` or `-v NAME=VALUE`, their values text, read back
//! into statements by `:NAME`, `:'NAME'` and `:"NAME"` (`split.rs`)
//! and into commands' arguments. Some names are the client's own; setting
//! them changes how it runs, and it sets others after each statement.

use std::collections::BTreeMap;

/// Each statement its own transaction, when on.
pub const AUTOCOMMIT: &str = "AUTOCOMMIT";
/// A run not typed ends at the first statement that fails, when on.
pub const STOP_ON_ERROR: &str = "STOP_ON_ERROR";
/// What is written as it runs, `none`, `queries` or `errors`.
pub const ECHO: &str = "ECHO";
/// The completions of statements other than queries, `OK` and the like,
/// left unwritten, when on.
pub const QUIET: &str = "QUIET";
/// The rows the last query gave.
pub const ROW_COUNT: &str = "ROW_COUNT";
/// Whether the last statement failed, `true` or `false`.
pub const ERROR: &str = "ERROR";
/// The SQLSTATE the last statement completed with.
pub const SQLSTATE: &str = "SQLSTATE";
/// The message of the last statement that failed.
pub const LAST_ERROR_MESSAGE: &str = "LAST_ERROR_MESSAGE";
/// The SQLSTATE of the last statement that failed.
pub const LAST_ERROR_SQLSTATE: &str = "LAST_ERROR_SQLSTATE";
/// The prompt for a statement (`prompt.rs`).
pub const PROMPT: &str = "PROMPT";
/// The prompt for the rest of a statement begun.
pub const PROMPT_MORE: &str = "PROMPT_MORE";
/// The file typed lines are kept in.
pub const HISTORY_FILE: &str = "HISTORY_FILE";
/// The most entries the history keeps.
pub const HISTORY_SIZE: &str = "HISTORY_SIZE";
/// A query's rows fetched this many at a time through a cursor, if more
/// than 0.
pub const FETCH_COUNT: &str = "FETCH_COUNT";
/// How much of a failure is told, `terse`, `default` or `verbose`.
pub const VERBOSITY: &str = "VERBOSITY";
/// UPDATE and DELETE without WHERE refused before they are sent, when on.
pub const SAFE_UPDATES: &str = "SAFE_UPDATES";
/// The user the session is of, set as it begins.
pub const USER: &str = "USER";
/// The server's product and version, set as the session begins.
pub const SERVER: &str = "SERVER";

/// The values a variable is true by, in any case.
const TRUE_WORDS: [&str; 4] = ["on", "true", "yes", "1"];
/// The values a variable is false by, in any case.
const FALSE_WORDS: [&str; 4] = ["off", "false", "no", "0"];

/// \[DESIGN\] RULES.md D212: the truth `text` names, if it names one,
/// which is on, true, yes or 1, or off, false, no or 0, in any case.
pub fn truth(text: &str) -> Option<bool> {
    let word = text.trim();
    if TRUE_WORDS.iter().any(|w| w.eq_ignore_ascii_case(word)) {
        Some(true)
    } else if FALSE_WORDS.iter().any(|w| w.eq_ignore_ascii_case(word)) {
        Some(false)
    } else {
        None
    }
}

/// Whether `name` may name a variable, being letters, digits and
/// underscores, at least one.
pub fn is_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(is_name_char)
}

/// Whether `c` may be in a variable's name.
pub fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `value` as a `<character string literal>`, each quote doubled
/// [ISO/IEC 9075:1992 §5.3].
pub fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// `value` as a `<delimited identifier>`, each double quote doubled
/// [ISO/IEC 9075:1992 §5.2].
pub fn identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// What begins a reference to whether a variable is set, as in `:{?NAME}`.
const DEFINED_OPEN: &str = ":{?";
/// What ends one.
const DEFINED_CLOSE: char = '}';
/// What a reference to whether a variable is set gives.
const TRUE_TEXT: &str = "true";
/// See [`TRUE_TEXT`].
const FALSE_TEXT: &str = "false";

/// \[DESIGN\] RULES.md D212: the text of the reference to a variable a
/// `:` at `at` in `chars` begins, and where after it; `:NAME` its value,
/// `:'NAME'` its value as a literal, `:"NAME"` as a delimited identifier,
/// `:{?NAME}` `true` if it is set, else `false`. None if the colon begins
/// no reference, or one to a variable not set.
pub fn reference(chars: &[char], at: usize, vars: &Variables) -> Option<(String, usize)> {
    let rest: String = chars.get(at..).unwrap_or_default().iter().collect();
    if let Some(inner) = rest.strip_prefix(DEFINED_OPEN)
        && let Some((name, _)) = inner.split_once(DEFINED_CLOSE)
        && is_name(name)
    {
        let text = if vars.get(name).is_some() { TRUE_TEXT } else { FALSE_TEXT };
        let used = DEFINED_OPEN.len().saturating_add(name.chars().count()).saturating_add(1);
        return Some((text.to_owned(), at.saturating_add(used)));
    }
    let after = at.saturating_add(1);
    let form = chars.get(after).copied().filter(|c| ['\'', '"'].contains(c));
    let start = if form.is_some() { after.saturating_add(1) } else { after };
    let len = chars.get(start..).unwrap_or_default().iter().take_while(|c| is_name_char(**c)).count();
    let end = start.saturating_add(len);
    let name: String = chars.get(start..end).unwrap_or_default().iter().collect();
    let closed = form.is_none_or(|q| chars.get(end) == Some(&q));
    let value = vars.get(&name).filter(|_| closed && len > 0)?;
    Some(match form {
        Some('\'') => (literal(value), end.saturating_add(1)),
        Some(_) => (identifier(value), end.saturating_add(1)),
        None => (value.to_owned(), end),
    })
}

/// How much of a failure is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    /// Its SQLCODE, SQLSTATE and message.
    Terse,
    /// Those, the rule it cites and where in the statement.
    Default,
    /// Those, and the statement's line with a mark under where.
    Verbose,
}

/// What is written as statements run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Echo {
    /// Nothing.
    None,
    /// Each statement, to the output, before it runs.
    Queries,
    /// Each statement that fails, to the errors, before its failure.
    Errors,
}

/// The variables of a session.
#[derive(Debug, Clone, Default)]
pub struct Variables {
    values: BTreeMap<String, String>,
}

impl Variables {
    /// The value of `name`, if it is set.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// Sets `name` to `value`.
    pub fn set(&mut self, name: &str, value: &str) {
        self.values.insert(name.to_owned(), value.to_owned());
    }

    /// Unsets `name`.
    pub fn unset(&mut self, name: &str) {
        self.values.remove(name);
    }

    /// Every variable set, by name.
    pub fn all(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }

    /// Whether `name` is set and true; false if unset or not a truth.
    pub fn on(&self, name: &str) -> bool {
        self.get(name).and_then(truth).unwrap_or(false)
    }

    /// The whole number `name` holds, if it holds one.
    pub fn number(&self, name: &str) -> Option<u32> {
        self.get(name).and_then(|v| v.trim().parse().ok())
    }

    /// How much of a failure is told, as VERBOSITY says, by default its
    /// rule and where.
    pub fn verbosity(&self) -> Verbosity {
        match self.get(VERBOSITY).map(str::to_ascii_lowercase).as_deref() {
            Some("terse") => Verbosity::Terse,
            Some("verbose") => Verbosity::Verbose,
            Some(_) | None => Verbosity::Default,
        }
    }

    /// What is written as statements run, as ECHO says, by default
    /// nothing.
    pub fn echo(&self) -> Echo {
        match self.get(ECHO).map(str::to_ascii_lowercase).as_deref() {
            Some("queries") => Echo::Queries,
            Some("errors") => Echo::Errors,
            Some(_) | None => Echo::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D212: a variable is set, read and
    /// unset; a truth is on, true, yes or 1, or off, false, no or 0, in
    /// any case; anything else is none, and an unset name is false.
    #[test]
    fn variables_are_set_and_read_as_truths() {
        let mut vars = Variables::default();
        assert_eq!(vars.get("A"), None);
        vars.set("A", "On");
        assert!(vars.on("A"));
        vars.set("A", "nope");
        assert!(!vars.on("A"));
        assert_eq!(truth(" YES "), Some(true));
        assert_eq!(truth("0"), Some(false));
        assert_eq!(truth(""), None);
        vars.unset("A");
        assert_eq!(vars.all().count(), 0);
        vars.set(FETCH_COUNT, " 25");
        assert_eq!(vars.number(FETCH_COUNT), Some(25));
        assert!(is_name("ROW_1"));
        assert!(!is_name("A-B"));
        assert!(!is_name(""));
    }

    /// Verifies: [DESIGN] RULES.md D212: VERBOSITY and ECHO are read in
    /// any case, an unknown value taken as the default.
    #[test]
    fn the_clients_own_variables_are_read() {
        let mut vars = Variables::default();
        assert_eq!(vars.verbosity(), Verbosity::Default);
        vars.set(VERBOSITY, "VERBOSE");
        assert_eq!(vars.verbosity(), Verbosity::Verbose);
        vars.set(VERBOSITY, "terse");
        assert_eq!(vars.verbosity(), Verbosity::Terse);
        assert_eq!(vars.echo(), Echo::None);
        vars.set(ECHO, "queries");
        assert_eq!(vars.echo(), Echo::Queries);
        vars.set(ECHO, "Errors");
        assert_eq!(vars.echo(), Echo::Errors);
    }

    /// Verifies: [DESIGN] RULES.md D212; [ISO/IEC 9075:1992 §5.2, §5.3] a
    /// value is quoted as a literal, its quotes doubled, or as a delimited
    /// identifier, its double quotes doubled.
    #[test]
    fn values_are_quoted_as_literals_and_identifiers() {
        assert_eq!(literal("it's"), "'it''s'");
        assert_eq!(identifier("a\"b"), "\"a\"\"b\"");
    }
}
