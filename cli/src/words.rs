//! \[DESIGN\] The words of a statement as the client reads them for
//! itself (RULES.md D210, D219): its `<regular identifier>`s and key
//! words in upper case, outside literals, delimited identifiers and
//! comments [ISO/IEC 9075:1992 §5.2, §5.3], so it can tell an UPDATE
//! without WHERE, a statement that changes the schema, or the context a
//! word is completed in, without parsing it.

/// UPDATE, which SAFE_UPDATES refuses without WHERE.
pub const UPDATE: &str = "UPDATE";
/// DELETE, which SAFE_UPDATES refuses without WHERE.
pub const DELETE: &str = "DELETE";
/// WHERE.
pub const WHERE: &str = "WHERE";
/// The first words of statements that may change the names the schema
/// holds, or undo such a change: tab completion reads them again after.
pub const SCHEMA_CHANGES: [&str; 5] = ["CREATE", "ALTER", "DROP", "ROLLBACK", "IMPORT"];
/// The first words of statements that end or mark a transaction, or
/// change the connection, which take no savepoint of their own.
pub const TRANSACTION_CONTROL: [&str; 8] = ["COMMIT", "ROLLBACK", "SAVEPOINT", "RELEASE", "START", "SET", "CONNECT", "DISCONNECT"];

/// What is being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum In {
    Code,
    Quote,
    Ident,
    Comment,
}

/// The words of `sql`, in upper case, outside its literals, delimited
/// identifiers and comments, each with where it begins.
pub fn words(sql: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut state = In::Code;
    let mut word: Option<(usize, String)> = None;
    let mut chars = sql.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let in_word = state == In::Code && (c.is_alphanumeric() || c == '_');
        if in_word {
            word.get_or_insert_with(|| (at, String::new())).1.extend(c.to_uppercase());
            continue;
        }
        found.extend(word.take());
        state = match (state, c) {
            (In::Code, '\'') => In::Quote,
            (In::Code, '"') => In::Ident,
            (In::Code, '-') => {
                if chars.peek().is_some_and(|(_, n)| *n == '-') {
                    In::Comment
                } else {
                    In::Code
                }
            }
            (In::Quote, '\'') | (In::Ident, '"') | (In::Comment, '\n') => In::Code,
            (In::Code | In::Quote | In::Ident | In::Comment, _) => state,
        };
    }
    found.extend(word);
    found
}

/// The first word of `sql`, in upper case, if it has one.
pub fn first(sql: &str) -> Option<String> {
    words(sql).into_iter().next().map(|(_, w)| w)
}

/// \[DESIGN\] RULES.md D219: whether `sql` is an UPDATE or DELETE without
/// WHERE, which changes every row of its table.
pub fn unbounded_change(sql: &str) -> bool {
    let all = words(sql);
    let changes = all.first().is_some_and(|(_, w)| w == UPDATE || w == DELETE);
    changes && !all.iter().any(|(_, w)| w == WHERE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D219; [ISO/IEC 9075:1992 §5.2, §5.3] a
    /// statement's words are read in upper case outside literals,
    /// delimited identifiers and comments.
    #[test]
    fn words_are_read_outside_quotes_and_comments() {
        let found: Vec<String> = words("select a_1, 'where' \"WHERE\" -- where\nfrom t").into_iter().map(|(_, w)| w).collect();
        assert_eq!(found, ["SELECT", "A_1", "FROM", "T"]);
        assert_eq!(words("x y")[1], (2, "Y".to_owned()));
        assert_eq!(first("  drop table t"), Some("DROP".to_owned()));
        assert_eq!(first("-- nothing"), None);
        assert_eq!(first("a-b"), Some("A".to_owned()));
    }

    /// Verifies: [DESIGN] RULES.md D219: UPDATE and DELETE without WHERE
    /// change every row; with WHERE, or another statement, do not; a WHERE
    /// in a literal is none. MC/DC of `changes && !where`: UPDATE without
    /// WHERE, UPDATE with it, a SELECT without it.
    #[test]
    fn changes_of_every_row_are_found() {
        assert!(unbounded_change("UPDATE T SET A = 1"));
        assert!(unbounded_change("delete from t"));
        assert!(unbounded_change("DELETE FROM T -- WHERE A = 1"));
        assert!(!unbounded_change("UPDATE T SET A = 1 WHERE B = 2"));
        assert!(!unbounded_change("DELETE FROM T WHERE CURRENT OF C"));
        assert!(!unbounded_change("SELECT * FROM T"));
        assert!(unbounded_change("UPDATE T SET A = 'WHERE'"));
    }
}
