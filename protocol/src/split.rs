//! \[DESIGN\] Statements of text, split at each `;` that ends one: not one
//! in a `<character string literal>` or a `<delimited identifier>`, where
//! a quote is written twice, nor in a `<comment>`, which runs from two
//! minus signs or more to the end of the line [ISO/IEC 9075:1992 §5.2,
//! §5.3]. A statement of nothing but separators is none. A statement is
//! sent without the separators around it, but for the `<newline>` that
//! ends a comment last in it, which a comment needs [X3.135-1986 §5.3];
//! the end of the text ends a comment as a newline would.
//!
//! A `;` in a `<compound triggered SQL statement>` [ISO/IEC 9075-2:2023
//! §11.49], or a `<compound statement>` or another SQL control statement
//! [ISO/IEC 9075-4:2006 draft §14.1 to §14.13], ends none either. In code,
//! the word BEGIN, a key word nothing else begins with, opens a block. In
//! one, BEGIN, CASE, IF, LOOP and REPEAT open a level, as DO does, which
//! begins the statements of WHILE and FOR; END closes one, and the word
//! after END, as in END IF, opens none. The block ends with the END of its
//! BEGIN. A statement whose first word, after a label and its colon if one
//! is written, is IF, CASE, LOOP, REPEAT, WHILE or FOR is a control
//! statement run alone, whose openers open levels as a block's do. MODULE
//! after CREATE opens a level, which END MODULE closes, so an
//! `<SQL-server module definition>` [ISO/IEC 9075-4:2006 draft §9.21] is
//! one statement with its routines. Words are matched in any case, never
//! in a string, an identifier or a comment.

/// BEGIN, which opens a block, or a level in one.
const BEGIN: &str = "BEGIN";
/// The words that open a level in a block, but after END.
const OPENERS: [&str; 5] = ["CASE", "IF", "LOOP", "REPEAT", "DO"];
/// END, which closes a block or a level in one.
const END: &str = "END";
/// CREATE, which, before MODULE, begins an SQL-server module definition.
const CREATE: &str = "CREATE";
/// MODULE, which opens a level after CREATE.
const MODULE: &str = "MODULE";
/// The first words of a control statement run alone, but BEGIN.
const CONTROL: [&str; 6] = ["IF", "CASE", "LOOP", "REPEAT", "WHILE", "FOR"];

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

/// What came last in code, for END IF and the like.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Last {
    /// The word END, with only separators after it.
    End,
    /// The word CREATE, with only separators after it, which MODULE after
    /// makes open a level.
    Create,
    /// Anything else, or nothing.
    #[default]
    Other,
}

/// What the first word of a statement makes of it, its label's not
/// counted.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Lead {
    /// No word yet, or only a label and its colon.
    #[default]
    Start,
    /// One word, which a colon makes a label.
    One,
    /// A control statement run alone.
    Control,
    /// Any other statement.
    Other,
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
    /// The word of code being read, if one is.
    word: String,
    /// What came last in code, as far as opening a level goes.
    last: Last,
    /// The levels of blocks and of the statements in them open, 0 out of
    /// any block.
    depth: usize,
    /// What its first word, its label's not counted, makes of it.
    lead: Lead,
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
        if self.state == State::Code {
            self.read_word(c);
        }
        let was_dash = std::mem::take(&mut self.dash);
        let in_comment = self.state == State::Comment;
        match (self.state, c) {
            (State::Code, ';') => {
                if self.depth == 0 {
                    return self.ended();
                }
            }
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

    /// Reads `c`, of code, into the word being read, or ends that word.
    /// A character neither a separator nor a minus sign, which may begin a
    /// comment, parts END from the word after it.
    fn read_word(&mut self, c: char) {
        if c.is_alphanumeric() || c == '_' {
            self.word.push(c);
            return;
        }
        if !self.word.is_empty() {
            self.ended_word();
        }
        // The colon after a label, so the word after it is the first.
        if c == ':' && self.lead == Lead::One {
            self.lead = Lead::Start;
        }
        if !c.is_whitespace() && c != '-' {
            self.last = Last::Other;
        }
    }

    /// Ends the word read, which may open or close a block or a level in
    /// one.
    fn ended_word(&mut self) {
        let word = std::mem::take(&mut self.word);
        let is = |keyword: &str| word.eq_ignore_ascii_case(keyword);
        self.lead = match self.lead {
            Lead::Start if CONTROL.iter().any(|w| is(w)) => Lead::Control,
            Lead::Start => Lead::One,
            Lead::One | Lead::Other => Lead::Other,
            Lead::Control => Lead::Control,
        };
        let module = self.last == Last::Create && is(MODULE);
        let opens = is(BEGIN)
            || module
            || ((self.depth > 0 || self.lead == Lead::Control) && self.last != Last::End && OPENERS.iter().any(|w| is(w)));
        if opens {
            self.depth = self.depth.saturating_add(1);
        } else if is(END) {
            self.depth = self.depth.saturating_sub(1);
        }
        self.last = if is(END) {
            Last::End
        } else if is(CREATE) {
            Last::Create
        } else {
            Last::Other
        };
    }

    /// The statement read so far, if it is one; the text begun anew.
    fn ended(&mut self) -> Option<String> {
        self.word.clear();
        (self.last, self.depth, self.lead) = (Last::Other, 0, Lead::Start);
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

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49] a `;` in BEGIN
    /// ATOMIC ... END ends no statement, a CASE ... END in the block
    /// closing no block, words matched in any case; the `;` after the
    /// block's END ends the statement; ATOMIC alone, or a word BEGIN only
    /// begins, opens none.
    #[test]
    fn semicolons_in_begin_atomic_end_no_statement() {
        let trigger = "CREATE TRIGGER TR AFTER INSERT ON T begin  atomic \
                       INSERT INTO U VALUES (CASE WHEN 1 = 1 THEN 1 ELSE 2 END); DELETE FROM U; End";
        assert_eq!(split(&format!("{trigger}; SELECT 1;")), [trigger, "SELECT 1"]);
        assert_eq!(split("SELECT ATOMIC FROM T; SELECT 2"), ["SELECT ATOMIC FROM T", "SELECT 2"]);
        assert_eq!(split("SELECT CASE WHEN A THEN 1 END FROM T; SELECT 2"), ["SELECT CASE WHEN A THEN 1 END FROM T", "SELECT 2"]);
        assert_eq!(split("SELECT BEGIN_ATOMIC; SELECT 2"), ["SELECT BEGIN_ATOMIC", "SELECT 2"]);
        assert_eq!(split("X BEGIN -- note\n ATOMIC A; B END; C"), ["X BEGIN -- note\n ATOMIC A; B END", "C"]);
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49]; [ISO/IEC
    /// 9075:1992 §5.2, §5.3] BEGIN ATOMIC in a string, a delimited
    /// identifier or a comment opens no block.
    #[test]
    fn begin_atomic_quoted_or_commented_opens_no_block() {
        assert_eq!(split("SELECT 'BEGIN ATOMIC'; SELECT 2"), ["SELECT 'BEGIN ATOMIC'", "SELECT 2"]);
        assert_eq!(split("SELECT \"BEGIN ATOMIC\"; SELECT 2"), ["SELECT \"BEGIN ATOMIC\"", "SELECT 2"]);
        assert_eq!(split("SELECT 1 -- BEGIN ATOMIC\n; SELECT 2"), ["SELECT 1 -- BEGIN ATOMIC\n", "SELECT 2"]);
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.1 to §14.13] a `;` in a
    /// compound statement, plain or ATOMIC, ends no statement, nor one in
    /// IF, CASE, LOOP, REPEAT, or WHILE or FOR, whose DO opens a level;
    /// END IF and the like close one level, the word after END opening
    /// none; out of a block IF, LOOP, REPEAT and DO open none. MC/DC of
    /// `opens`: BEGIN out of a block opens, an opener in one opens, an
    /// opener after END does not, an opener out of a block does not.
    #[test]
    fn semicolons_in_control_statements_end_no_statement() {
        let body = "CREATE PROCEDURE P () BEGIN DECLARE I INTEGER DEFAULT 0; \
                    L: LOOP SET I = I + 1; IF I > 3 THEN LEAVE L; END IF; END LOOP L; \
                    WHILE I > 0 DO SET I = I - 1; END WHILE; REPEAT SET I = I + 1; UNTIL I > 2 END REPEAT; \
                    FOR R AS SELECT K FROM T DO CASE WHEN R.K = 1 THEN SET I = 0; ELSE SET I = 1; END CASE; END FOR; \
                    B: BEGIN ATOMIC SET I = 2; END B; END";
        assert_eq!(split(&format!("{body}; SELECT 1;")), [body, "SELECT 1"]);
        assert_eq!(split("begin insert into t values (1); end; select 2"), ["begin insert into t values (1); end", "select 2"]);
        assert_eq!(
            split("DROP TABLE IF; SELECT LOOP, REPEAT, DO FROM T; SELECT 2"),
            ["DROP TABLE IF", "SELECT LOOP, REPEAT, DO FROM T", "SELECT 2"]
        );
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §9.21] a `;` in a
    /// CREATE MODULE, after a routine of it or in a body, ends no
    /// statement; END MODULE closes it, a module in a schema definition
    /// too; MODULE alone, as in MODULE.T, opens nothing.
    #[test]
    fn semicolons_in_a_module_end_no_statement() {
        let module = "create module M PROCEDURE P () BEGIN SET X = 1; END; \
                      FUNCTION F () RETURNS INTEGER RETURN CASE WHEN 1 = 1 THEN 1 END; END MODULE";
        assert_eq!(split(&format!("{module}; SELECT 2")), [module, "SELECT 2"]);
        let schema = "CREATE SCHEMA S CREATE TABLE T (A INT) CREATE MODULE M PROCEDURE P () DELETE FROM T; END MODULE";
        assert_eq!(split(&format!("{schema}; SELECT 2")), [schema, "SELECT 2"]);
        assert_eq!(split("SELECT * FROM MODULE.T; SELECT 2"), ["SELECT * FROM MODULE.T", "SELECT 2"]);
        assert_eq!(
            split("SELECT CREATE_ FROM T; DROP MODULE M CASCADE; SELECT 2"),
            ["SELECT CREATE_ FROM T", "DROP MODULE M CASCADE", "SELECT 2"]
        );
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §4.10.7, §14.6 to §14.13] a
    /// control statement run alone, labelled or not, keeps its `;`s: IF,
    /// CASE, LOOP, REPEAT, and WHILE and FOR by their DO. A query whose
    /// first word is none of them, or whose label-like first word is not
    /// one, splits as before.
    #[test]
    fn control_statements_run_alone_keep_their_semicolons() {
        for alone in [
            "IF X > 1 THEN SIGNAL SQLSTATE '75001'; ELSE SIGNAL SQLSTATE '75002'; END IF",
            "case x when 1 then signal sqlstate '75001'; end case",
            "L: LOOP LEAVE L; END LOOP L",
            "REPEAT SIGNAL SQLSTATE '01001'; UNTIL TRUE END REPEAT",
            "W: WHILE FALSE DO SIGNAL SQLSTATE '75001'; END WHILE W",
            "FOR R AS SELECT K FROM T DO IF R.K = 1 THEN SIGNAL SQLSTATE '75001'; END IF; END FOR",
        ] {
            assert_eq!(split(&format!("{alone}; SELECT 1")), [alone, "SELECT 1"], "{alone}");
        }
        assert_eq!(split("SELECT IF FROM T; SELECT 2"), ["SELECT IF FROM T", "SELECT 2"]);
        assert_eq!(split("X: SELECT 1; SELECT 2"), ["X: SELECT 1", "SELECT 2"]);
    }
}
