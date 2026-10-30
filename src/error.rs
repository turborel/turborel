//! Errors as SQLCODE values. Every negative value is implementor-defined
//! [X3.135-1986 §3.3, §7.3 GR3b ii]; 100 means "no data" and is not an error
//! [X3.135-1986 §7.3 GR3a]; 0 is success. Each also has an SQLSTATE
//! (`state.rs`).

mod state;
mod subject;

use std::fmt;

pub use state::SqlState;
pub use subject::Subject;

/// The kinds of error, each with its negative SQLCODE as its discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// Input is not made of valid `<token>`s and `<separator>`s ([X3.135-1986 §5.1-§5.3]).
    Lexical = -101,
    /// Input violates a Format (the BNF).
    Syntax = -102,
    /// Input matches the BNF but violates a numbered Syntax Rule.
    SyntaxRule = -103,
    /// The applicable `<privileges>` do not include the needed action [X3.135-1986 §6.6].
    Privilege = -104,
    /// An integrity constraint would be violated [X3.135-1986 §4.5]: NOT NULL,
    /// UNIQUE or PRIMARY KEY, CHECK, FOREIGN KEY, a domain's or an assertion
    /// [ISO/IEC 9075:1992 §4.10]; or a row would leave a view WITH CHECK
    /// OPTION [X3.135-1986 §6.5 GR3a].
    Constraint = -201,
    /// A General Rule "shall" failed at run time (overflow, division by zero).
    Data = -301,
    /// The cursor is not in the state or position the statement requires
    /// [X3.135-1986 §8.1 GR1, §8.4 GR1, §8.6 GR1, §8.8 GR1, §8.11 GR1].
    Cursor = -501,
    /// A single-row SELECT found more than one row [X3.135-1986 §8.10 GR2],
    /// or a subquery that may give at most one row gave more [X3.135-1986 §5.11 GR1]
    /// [ISO/IEC 9075:1992 §7.11 GR1].
    Cardinality = -811,
    /// \[DESIGN\] The transaction conflicted with another: a lock it could not
    /// have or wait for, or a write or commit its engine refused (RULES.md
    /// D28, D29). The statement, or for a refused commit the transaction, is undone.
    Conflict = -911,
    /// \[DESIGN\] The transaction policy forbids using an engine this way.
    Policy = -912,
    /// [ISO/IEC 9075:1992 §14.1 GR1, §13.6-§13.10 GR1, §12.3 GR6c v] The
    /// statement is not allowed in the transaction's state: SET
    /// TRANSACTION, SET SESSION AUTHORIZATION [§16.4 GR1] or DISCONNECT
    /// [§15.3 GR6] while one is active, a change of data or schemas in a
    /// READ ONLY one; \[EXTENSION\] or a statement that runs outside a
    /// transaction while one is active.
    TransactionState = -917,
    /// \[DESIGN\] The memory budget cannot grant what the statement needs
    /// (RULES.md D38, `memory.rs`), or a buffer pool has every frame held.
    Resources = -918,
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §22.1, Table 23] A connection
    /// exception: a server refused a connection or its credentials, or the
    /// connection failed, its messages malformed or the server going away.
    Connection = -919,
    /// \[DESIGN\] The statement was cancelled, from another thread or by a
    /// server's client (RULES.md D44); it is undone, its transaction goes
    /// on.
    Cancelled = -920,
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §17, §22.1] A dynamic SQL error: a
    /// prepared statement given values not one for each of its parameters,
    /// or executed or opened as it cannot be (SQLSTATE class 07).
    Dynamic = -921,
    /// \[DESIGN\] A participant could not promise to commit, so COMMIT rolled back instead.
    CommitFailed = -913,
    /// \[DESIGN\] A change could not be undone, so its table is damaged: the
    /// table no longer shows a state the transactions produced, and every
    /// later statement on it is refused (RULES.md D19).
    Damaged = -914,
    /// \[DESIGN\] The operating system or a file-system plug-in reported an
    /// error reading, writing or syncing a file (RULES.md D30).
    Io = -915,
    /// \[DESIGN\] Stored data failed its checksum or is not in the format
    /// expected: it was corrupted, or never completely written (RULES.md D30).
    Corrupt = -916,
    /// Valid SQL that this implementation does not process: not built yet,
    /// or beyond one of its limits (RULES.md D18).
    Unsupported = -900,
    /// Misuse of the host (Rust) API, which lies outside the standard.
    Api = -910,
}

/// An error: its kind, which sets the SQLCODE, what happened, and the rule
/// of a standard it breaks, if it breaks one (RULES.md R-ERR-2).
///
/// Code writes a message as its source cites it, `"[X3.135-1986 §5.7 SR3b]
/// column ID is ambiguous"`, and the error keeps the citation apart from
/// the text: `rule` is `X3.135-1986 §5.7 SR3b`, `msg` is `column ID is
/// ambiguous`. The tags `[DESIGN]` and `[EXTENSION]`, which mark in the
/// source what no standard governs, are not kept: they tell a reader of the
/// error nothing.
#[derive(Debug, Clone)]
pub struct SqlError {
    /// The kind of error.
    pub code: Code,
    /// What happened, in plain words.
    pub msg: String,
    /// The rule of a standard broken, as the standard prints it, if one is:
    /// `X3.135-1986 §8.3 SR6`, `ISO/IEC 9075:1992 §11.8 GR7`.
    pub rule: Option<String>,
    /// Character offset into the statement, when known: where the token, or
    /// the construct, it is about begins. For an OPEN, a position is in the
    /// cursor's specification, as declared.
    pub pos: Option<usize>,
    /// \[EXTENSION\] Its SQLSTATE, where closer than its code's
    /// (`SqlState::of`): a subclass, or another class.
    pub state: Option<SqlState>,
    /// \[EXTENSION\] What it is about, where known: the constraint, table,
    /// column or cursor GET DIAGNOSTICS gives [ISO/IEC 9075:1992 §18.1].
    pub subject: Option<Box<Subject>>,
}

impl SqlError {
    /// An error with no position in the statement.
    pub fn new(code: Code, msg: impl Into<String>) -> Self {
        let (rule, msg) = cited(&msg.into());
        SqlError { code, msg, rule, pos: None, state: None, subject: None }
    }
    /// An error at character `pos` of the statement.
    pub fn at(code: Code, pos: usize, msg: impl Into<String>) -> Self {
        SqlError { pos: Some(pos), ..SqlError::new(code, msg) }
    }

    /// Its message, with the rule it breaks after it if it breaks one: for
    /// an error that reports this one inside its own message.
    pub fn described(&self) -> String {
        match &self.rule {
            Some(rule) => format!("{} [{rule}]", self.msg),
            None => self.msg.clone(),
        }
    }
    /// This error, at character `pos` of the statement unless it has a
    /// position already: an error takes the position of the innermost
    /// construct that has one, the binder placing each construct's errors
    /// as it binds it.
    #[must_use]
    pub fn or_at(mut self, pos: usize) -> Self {
        self.pos.get_or_insert(pos);
        self
    }

    /// This error, with the SQLSTATE `state` [ISO/IEC 9075:1992 §22.1].
    #[must_use]
    pub fn with_state(mut self, state: SqlState) -> Self {
        self.state = Some(state);
        self
    }

    /// This error, about `subject` [ISO/IEC 9075:1992 §18.1 GR3].
    #[must_use]
    pub fn about(mut self, subject: Subject) -> Self {
        self.subject = Some(Box::new(subject));
        self
    }

    /// \[EXTENSION\] The SQLSTATE: the one it was raised with, or its
    /// code's [ISO/IEC 9075:1992 §22.1].
    pub fn sqlstate(&self) -> SqlState {
        self.state.unwrap_or_else(|| SqlState::of(self.code))
    }

    /// The SQLCODE: a negative number whose value is implementor-defined
    /// [X3.135-1986 §7.3 GR3b ii].
    #[expect(clippy::as_conversions, reason = "R-NUM-2 deviation: a field-less enum's discriminant is its SQLCODE, and the cast is exact")]
    pub fn sqlcode(&self) -> i32 {
        self.code as i32
    }
}

impl fmt::Display for SqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SQLCODE {} (SQLSTATE {}): {}", self.sqlcode(), self.sqlstate(), self.described())?;
        if let Some(p) = self.pos {
            write!(f, " (at character {p})")?;
        }
        Ok(())
    }
}

/// The tags that mark, in the source, what no standard governs.
const TAGS: [&str; 2] = ["[DESIGN]", "[EXTENSION]"];
/// The standards a citation may name.
const STANDARDS: [&str; 2] = ["X3.135-1986", "ISO/IEC 9075:1992"];

/// The rule a message as the source writes it cites first, and its text:
/// its leading tags dropped, and a leading citation of a standard taken
/// out. A message that begins otherwise is its own text.
fn cited(message: &str) -> (Option<String>, String) {
    let mut text = message.trim_start();
    while let Some(rest) = TAGS.iter().find_map(|t| text.strip_prefix(t)) {
        text = rest.trim_start();
    }
    let citation =
        text.strip_prefix('[').and_then(|rest| rest.split_once(']')).filter(|(rule, _)| STANDARDS.iter().any(|s| rule.starts_with(s)));
    match citation {
        Some((rule, rest)) => (Some(rule.to_owned()), rest.trim_start().to_owned()),
        None => (None, text.to_owned()),
    }
}

/// The result of anything that can fail with an `SqlError`.
pub type Result<T> = std::result::Result<T, SqlError>;

/// `SqlError::or_at` for a result's error.
pub trait At {
    /// The result, its error at character `pos` unless it has a position.
    #[must_use]
    fn or_at(self, pos: usize) -> Self;
}

impl<T> At for Result<T> {
    fn or_at(self, pos: usize) -> Self {
        self.map_err(|e| e.or_at(pos))
    }
}

/// A new `SqlError`, as an `Err`.
///
/// # Errors
///
/// Always: the error is the result.
pub fn fail<T>(code: Code, msg: impl Into<String>) -> Result<T> {
    Err(SqlError::new(code, msg))
}

/// A new `SqlError` with the SQLSTATE `state`, as an `Err`.
///
/// # Errors
///
/// Always: the error is the result.
pub fn fail_with<T>(code: Code, state: SqlState, msg: impl Into<String>) -> Result<T> {
    Err(SqlError::new(code, msg).with_state(state))
}

// Unit tests: the SQLCODE of each code, as listed in CONFORMANCE.md (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// The documented number of each code. The match is exhaustive, so a new
    /// code cannot be added without giving it a number here.
    fn documented(code: Code) -> i32 {
        match code {
            Code::Lexical => -101,
            Code::Syntax => -102,
            Code::SyntaxRule => -103,
            Code::Privilege => -104,
            Code::Constraint => -201,
            Code::Data => -301,
            Code::Cursor => -501,
            Code::Cardinality => -811,
            Code::Conflict => -911,
            Code::Policy => -912,
            Code::TransactionState => -917,
            Code::Resources => -918,
            Code::Connection => -919,
            Code::Cancelled => -920,
            Code::Dynamic => -921,
            Code::CommitFailed => -913,
            Code::Damaged => -914,
            Code::Io => -915,
            Code::Corrupt => -916,
            Code::Unsupported => -900,
            Code::Api => -910,
        }
    }

    /// Verifies: [DESIGN] RULES.md R-ERR-2: an error keeps the rule of a
    /// standard its source cites apart from its message, which says what
    /// happened in plain words; the source's tags for what no standard
    /// governs are dropped; a message citing nothing is its own text; the
    /// error shows its message, then its rule, then its position.
    #[test]
    fn errors_keep_their_rule_apart_from_their_message() {
        let e = SqlError::at(Code::SyntaxRule, 12, "[X3.135-1986 §5.7 SR3b] column ID is ambiguous; qualify it");
        assert_eq!((e.rule.as_deref(), e.msg.as_str()), (Some("X3.135-1986 §5.7 SR3b"), "column ID is ambiguous; qualify it"));
        assert_eq!(
            e.to_string(),
            "SQLCODE -103 (SQLSTATE 2A000): column ID is ambiguous; qualify it [X3.135-1986 §5.7 SR3b] (at character 12)"
        );
        let tagged = SqlError::new(Code::Api, "[EXTENSION] [ISO/IEC 9075:1992 §21.1] only SELECT is granted");
        assert_eq!((tagged.rule.as_deref(), tagged.msg.as_str()), (Some("ISO/IEC 9075:1992 §21.1"), "only SELECT is granted"));
        let design = SqlError::new(Code::Api, "[DESIGN] no index T_X");
        assert_eq!((design.rule, design.msg.as_str()), (None, "no index T_X"));
        let plain = SqlError::new(Code::Io, "[see below] the disk is full");
        assert_eq!((plain.rule, plain.msg.as_str()), (None, "[see below] the disk is full"));
        assert_eq!(e.described(), "column ID is ambiguous; qualify it [X3.135-1986 §5.7 SR3b]");
    }

    /// Verifies: [X3.135-1986 §3.3, §7.3 GR3b ii] every error sets a negative
    /// SQLCODE; [DESIGN] its implementor-defined value is the documented one.
    #[test]
    fn each_code_has_its_documented_sqlcode() {
        let all = [
            Code::Lexical,
            Code::Syntax,
            Code::SyntaxRule,
            Code::Privilege,
            Code::Constraint,
            Code::Data,
            Code::Cursor,
            Code::Cardinality,
            Code::Conflict,
            Code::Policy,
            Code::TransactionState,
            Code::Resources,
            Code::Connection,
            Code::Cancelled,
            Code::Dynamic,
            Code::CommitFailed,
            Code::Damaged,
            Code::Io,
            Code::Corrupt,
            Code::Unsupported,
            Code::Api,
        ];
        for code in all {
            let sqlcode = SqlError::new(code, "").sqlcode();
            assert_eq!(sqlcode, documented(code), "{code:?}");
            assert!(sqlcode < 0_i32, "{code:?}");
            let state = SqlError::new(code, "").sqlstate();
            assert_eq!(state, SqlState::of(code));
            assert!(state.as_str().len() == 5 && state.as_str().bytes().all(|b| b.is_ascii_digit() || b.is_ascii_uppercase()), "{state}");
            // [ISO/IEC 9075:1992 §22.1] a class this implementation defines
            // begins with 5 to 9 or I to Z; its subclass is the SQLCODE's digits.
            if state.class() == "TB" {
                assert_eq!(state.as_str().get(2..), Some(sqlcode.unsigned_abs().to_string().as_str()), "{code:?}");
            }
        }
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §22.1] an error raised with
    /// a closer SQLSTATE keeps it; one raised without has its code's; the
    /// message shows both values.
    #[test]
    fn an_error_has_its_sqlstate() {
        let plain = SqlError::new(Code::Data, "x");
        assert_eq!(plain.sqlstate(), SqlState::DATA_EXCEPTION);
        let closer = plain.with_state(SqlState::DIVISION_BY_ZERO);
        assert_eq!((closer.sqlstate().as_str(), closer.sqlstate().class()), ("22012", "22"));
        assert_eq!(closer.to_string(), "SQLCODE -301 (SQLSTATE 22012): x");
    }
}
