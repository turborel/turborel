//! \[DESIGN\] Statements of text, split at each `;` that ends one: not one
//! in a `<character string literal>` or a `<delimited identifier>`, where
//! a quote is written twice, nor in a `<comment>`, which runs from two
//! minus signs or more to the end of the line [ISO/IEC 9075:1992 §5.2,
//! §5.3]. A statement of nothing but separators is none. A statement is
//! sent without the separators around it, but for the `<newline>` that
//! ends a comment last in it, which a comment needs [X3.135-1986 §5.3];
//! the end of the text ends a comment as a newline would.

/// Where the text read so far stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum State {
    /// Between tokens, or in one that holds no `;`.
    #[default]
    Code,
    /// In a `<character string literal>`.
    Quote,
    /// In a `<delimited identifier>`.
    Ident,
    /// In a `<comment>`.
    Comment,
}

/// What splits text, given in parts, into statements.
#[derive(Debug, Default)]
pub struct Splitter {
    text: String,
    state: State,
    /// Whether the last character, in code, was a minus sign.
    dash: bool,
    /// Whether the statement has a character outside separators.
    significant: bool,
    /// Whether it had one before that minus sign: if a second makes the
    /// two a comment, the first was none.
    before_dash: bool,
    /// Where the `<newline>` ending the last comment ends, in `text`.
    comment_end: usize,
}

impl Splitter {
    /// The statements `chunk` ends, with what came before it.
    pub fn feed(&mut self, chunk: &str) -> Vec<String> {
        let mut done = Vec::new();
        for c in chunk.chars() {
            if let Some(statement) = self.take(c) {
                done.push(statement);
            }
        }
        done
    }

    /// The statement `c` ends, if it ends one.
    fn take(&mut self, c: char) -> Option<String> {
        let was_dash = std::mem::take(&mut self.dash);
        let in_comment = self.state == State::Comment;
        match (self.state, c) {
            (State::Code, ';') => return self.ended(),
            (State::Code, '\'') => self.state = State::Quote,
            (State::Code, '"') => self.state = State::Ident,
            (State::Code, '-') => {
                if was_dash {
                    self.state = State::Comment;
                    self.significant = self.before_dash;
                } else {
                    (self.dash, self.before_dash) = (true, self.significant);
                }
            }
            (State::Quote, '\'') | (State::Ident, '"') | (State::Comment, '\n') => self.state = State::Code,
            (State::Code | State::Quote | State::Ident | State::Comment, _) => {}
        }
        if self.state != State::Comment && !c.is_whitespace() {
            self.significant = true;
        }
        self.text.push(c);
        // The <newline> that ends a comment.
        if in_comment && self.state == State::Code {
            self.comment_end = self.text.len();
        }
        None
    }

    /// The statement read so far, if it is one; the text begun anew.
    fn ended(&mut self) -> Option<String> {
        let text = std::mem::take(&mut self.text);
        let significant = std::mem::take(&mut self.significant);
        let comment_end = std::mem::take(&mut self.comment_end);
        let end = text.trim_end().len().max(comment_end);
        significant.then(|| text.get(..end).unwrap_or_default().trim_start().to_owned())
    }

    /// Whether a statement is begun and not ended.
    pub fn pending(&self) -> bool {
        self.significant || self.state != State::Code
    }

    /// The statement left at the end of the text, ended by it: if it is
    /// one.
    pub fn finish(&mut self) -> Option<String> {
        if self.state == State::Comment {
            self.take('\n');
        }
        self.state = State::Code;
        self.dash = false;
        self.ended()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str) -> Vec<String> {
        let mut splitter = Splitter::default();
        let mut all = splitter.feed(text);
        all.extend(splitter.finish());
        all
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.2, §5.3]; [DESIGN] RULES.md D46: text
    /// splits at each `;` that ends a statement, not one in a string, its
    /// quote doubled, nor in a delimited identifier, nor in a comment; a
    /// statement of separators alone is none; the last needs no `;`.
    #[test]
    fn statements_split_at_their_ends() {
        assert_eq!(split("SELECT 1; SELECT 2"), ["SELECT 1", "SELECT 2"]);
        assert_eq!(split("SELECT 'a;b''c;'; SELECT \"x;\"\"y\" FROM T;"), ["SELECT 'a;b''c;'", "SELECT \"x;\"\"y\" FROM T"]);
        assert_eq!(split("SELECT 1 -- a; comment\n; ;;"), ["SELECT 1 -- a; comment\n"]);
        assert_eq!(split("-- only a note;\n  ;\n"), Vec::<String>::new());
        assert_eq!(split("SELECT 1 - 2;"), ["SELECT 1 - 2"]);
        assert_eq!(split("SELECT 1 --- three\n"), ["SELECT 1 --- three\n"]);
        assert_eq!(split("SELECT 1 -- open"), ["SELECT 1 -- open\n"], "the end of the text ends a comment");
        assert_eq!(split("SELECT 1 \n  ;"), ["SELECT 1"], "no newline kept but a comment's");
        assert_eq!(split("-- one\n-- two\n;"), Vec::<String>::new());
    }

    /// Verifies: [DESIGN] RULES.md D46: text given in parts splits as if
    /// whole; a statement begun, or a string or a comment open, is pending.
    #[test]
    fn text_in_parts_splits_as_whole() {
        let mut splitter = Splitter::default();
        assert_eq!(splitter.feed("SELECT 'a"), Vec::<String>::new());
        assert!(splitter.pending(), "in a string");
        assert_eq!(splitter.feed(";b' FROM T -"), Vec::<String>::new());
        assert_eq!(splitter.feed("- x;\n;"), ["SELECT 'a;b' FROM T -- x;\n"]);
        assert!(!splitter.pending());
        assert_eq!(splitter.feed("  "), Vec::<String>::new());
        assert!(!splitter.pending(), "separators alone");
        assert_eq!(splitter.finish(), None);
    }
}
