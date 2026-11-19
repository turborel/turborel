//! \[DESIGN\] Tab completion (RULES.md D211): the word before the cursor
//! completed by what its context allows. After `\`, a backslash command;
//! after FROM, JOIN, INTO, UPDATE, TABLE, a describe command and the like,
//! a table or view; after SCHEMA, a schema; after USER, ROLE,
//! AUTHORIZATION and TO, a user or role; after SELECT, WHERE, BY, SET, ON,
//! a comparison or a comma, a column or key word; else a key word of the
//! lexer's. A word typed in lower case is completed in lower case.
//!
//! The names are read from the Information Schema once, the first time a
//! context needs them, and kept until a statement that may change them
//! (`words::SCHEMA_CHANGES`) or `\c` throws them away.

use turborel::lexer::KEY_WORDS;
use turborel::storage::Row;

use crate::format::cell;
use crate::run::{Completion, Statements};

/// What completes tables, read from the Information Schema.
const TABLES: &str = "SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.TABLES";
/// What completes columns.
const COLUMNS: &str = "SELECT DISTINCT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA <> 'INFORMATION_SCHEMA'";
/// What completes schemas.
const SCHEMAS: &str = "SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA";
/// What completes users.
const USERS: &str = "SELECT USER_NAME FROM INFORMATION_SCHEMA.USERS";
/// What completes roles.
const ROLES: &str = "SELECT ROLE_NAME FROM INFORMATION_SCHEMA.ENABLED_ROLES";
/// The words after which a table is named.
const TABLE_WORDS: [&str; 9] = ["FROM", "JOIN", "INTO", "UPDATE", "TABLE", "VIEW", "INDEX", "ON", "DESCRIBE"];
/// The words after which a column or an expression comes.
const COLUMN_WORDS: [&str; 10] = ["SELECT", "WHERE", "BY", "SET", "AND", "OR", "NOT", "HAVING", "WHEN", "RETURNING"];
/// The words after which a user or role is named.
const USER_WORDS: [&str; 4] = ["USER", "ROLE", "AUTHORIZATION", "TO"];
/// The backslash commands completed after `\`.
pub const COMMAND_NAMES: [&str; 49] = [
    "?",
    "a",
    "c",
    "conninfo",
    "d",
    "dD",
    "dG",
    "dT",
    "df",
    "di",
    "dn",
    "dp",
    "ds",
    "dt",
    "du",
    "dv",
    "dx",
    "delimiter",
    "e",
    "echo",
    "elif",
    "else",
    "endif",
    "export",
    "format",
    "g",
    "gexec",
    "gset",
    "gx",
    "G",
    "h",
    "i",
    "if",
    "import",
    "ir",
    "l",
    "notee",
    "o",
    "p",
    "password",
    "q",
    "r",
    "set",
    "t",
    "tee",
    "timing",
    "unset",
    "watch",
    "x",
];

/// The names completion offers, as read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Names {
    /// Each table and view, by its name and by its schema's and name.
    pub tables: Vec<String>,
    /// Each column's name.
    pub columns: Vec<String>,
    /// Each schema.
    pub schemas: Vec<String>,
    /// Each user and role.
    pub users: Vec<String>,
}

/// What the word being completed may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// A backslash command.
    Command,
    /// A table or view.
    Table,
    /// A column, or a key word.
    Column,
    /// A schema.
    Schema,
    /// A user or role.
    User,
    /// A key word.
    KeyWord,
}

/// The texts of the first column of `rows`.
fn texts(rows: &[Row]) -> Vec<String> {
    rows.iter().filter_map(|r| r.first()).map(|v| cell(v, "")).collect()
}

impl Names {
    /// Reads the names over `connection`; a query that fails reads none.
    pub fn read<S: Statements + ?Sized>(connection: &mut S) -> Names {
        let mut query = |sql: &str| {
            let mut kept = crate::run::Kept::default();
            connection.query_into(sql, &mut kept).map_or_else(|_| Vec::new(), |_| kept.rows)
        };
        let mut tables = Vec::new();
        for row in query(TABLES) {
            let [schema, name] = [0, 1].map(|at| row.get(at).map(|v| cell(v, "")).unwrap_or_default());
            tables.push(format!("{schema}.{name}"));
            tables.push(name);
        }
        let mut users = texts(&query(USERS));
        users.extend(texts(&query(ROLES)));
        let mut names = Names { tables, columns: texts(&query(COLUMNS)), schemas: texts(&query(SCHEMAS)), users };
        for list in [&mut names.tables, &mut names.columns, &mut names.schemas, &mut names.users] {
            list.sort();
            list.dedup();
        }
        names
    }
}

/// Where the word before the cursor begins in `before`.
fn word_start(before: &str) -> usize {
    before.rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '"' || c == '+')).map_or(0, |at| at.saturating_add(1))
}

/// The context of a word begun at `start` in `before`.
pub fn context(before: &str, start: usize) -> Context {
    let head = before.get(..start).unwrap_or_default();
    let line = head.rsplit('\n').next().unwrap_or_default();
    if head.ends_with('\\') {
        return Context::Command;
    }
    let trimmed = head.trim_end();
    if line.trim_start().starts_with("\\d") && line.contains(' ') {
        return Context::Table;
    }
    if trimmed.ends_with([',', '=', '<', '>', '(']) {
        return Context::Column;
    }
    let Some(previous) = crate::words::words(trimmed).pop().map(|(_, w)| w) else { return Context::KeyWord };
    if TABLE_WORDS.contains(&previous.as_str()) {
        Context::Table
    } else if previous == "SCHEMA" {
        Context::Schema
    } else if USER_WORDS.contains(&previous.as_str()) {
        Context::User
    } else if COLUMN_WORDS.contains(&previous.as_str()) {
        Context::Column
    } else {
        Context::KeyWord
    }
}

/// The candidates of `context` that begin `word`, in its case.
pub fn candidates(context: Context, word: &str, names: Option<&Names>) -> Vec<String> {
    let lower = word.chars().any(char::is_lowercase) && !word.starts_with('"');
    let upper = word.to_uppercase();
    let key_words = KEY_WORDS.iter().map(|k| (*k).to_owned());
    let pool: Vec<String> = match (context, names) {
        (Context::Command, _) => return COMMAND_NAMES.iter().filter(|c| c.starts_with(word)).map(|c| (*c).to_owned()).collect(),
        (Context::Table, Some(n)) => n.tables.clone(),
        (Context::Schema, Some(n)) => n.schemas.clone(),
        (Context::User, Some(n)) => n.users.clone(),
        (Context::Column, Some(n)) => n.columns.iter().cloned().chain(key_words).collect(),
        (Context::Column | Context::KeyWord, None) | (Context::KeyWord, Some(_)) => key_words.collect(),
        (Context::Table | Context::Schema | Context::User, None) => Vec::new(),
    };
    let mut found: Vec<String> =
        pool.into_iter().filter(|c| c.starts_with(&upper)).map(|c| if lower { c.to_lowercase() } else { c }).collect();
    found.sort();
    found.dedup();
    found
}

/// The completions of the text `before` the cursor: the names they need
/// read over `connection` into `names` if not yet read.
pub fn complete<S: Statements + ?Sized>(before: &str, names: &mut Option<Names>, connection: &mut S) -> Completion {
    let start = word_start(before);
    let found = context(before, start);
    let needs = matches!(found, Context::Table | Context::Column | Context::Schema | Context::User);
    if needs && names.is_none() {
        *names = Some(Names::read(connection));
    }
    let word = before.get(start..).unwrap_or_default();
    Completion { start, candidates: candidates(found, word, names.as_ref()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(before: &str) -> Context {
        context(before, word_start(before))
    }

    /// Verifies: [DESIGN] RULES.md D211: the context of a word is read
    /// from what comes before it: a backslash, a table word, SCHEMA, a
    /// user word, a column word or comparison, a describe command, else a
    /// key word.
    #[test]
    fn contexts_are_read_from_the_words_before() {
        assert_eq!(at("\\ti"), Context::Command);
        assert_eq!(at("SELECT * FROM st"), Context::Table);
        assert_eq!(at("insert into "), Context::Table);
        assert_eq!(at("SET SCHEMA "), Context::Schema);
        assert_eq!(at("CREATE SCHEMA s"), Context::Schema);
        assert_eq!(at("GRANT SELECT ON T TO a"), Context::User);
        assert_eq!(at("SELECT a, b"), Context::Column);
        assert_eq!(at("SELECT * FROM T WHERE x = y"), Context::Column);
        assert_eq!(at("SELECT * FROM T WHERE "), Context::Column);
        assert_eq!(at("\\d st"), Context::Table);
        assert_eq!(at("sel"), Context::KeyWord);
        assert_eq!(at("SELECT 'FROM x"), Context::Column);
    }

    /// Verifies: [DESIGN] RULES.md D211: candidates are those of the
    /// context that begin the word, in its case; names not read offer
    /// none; columns come with key words.
    #[test]
    fn candidates_begin_the_word_in_its_case() {
        let names = Names {
            tables: vec!["S.STAFF".into(), "STAFF".into()],
            columns: vec!["SALARY".into()],
            schemas: vec!["S".into()],
            users: vec!["ANN".into()],
        };
        assert_eq!(candidates(Context::Table, "st", Some(&names)), ["staff"]);
        assert_eq!(candidates(Context::Table, "S.", Some(&names)), ["S.STAFF"]);
        assert_eq!(candidates(Context::Table, "S", None), Vec::<String>::new());
        assert!(candidates(Context::Column, "SAL", Some(&names)).contains(&"SALARY".to_owned()));
        assert!(candidates(Context::KeyWord, "SELE", None).contains(&"SELECT".to_owned()));
        assert_eq!(candidates(Context::Command, "ti", None), ["timing"]);
        assert_eq!(candidates(Context::User, "A", Some(&names)), ["ANN"]);
        assert_eq!(candidates(Context::Schema, "", Some(&names)), ["S"]);
    }
}
