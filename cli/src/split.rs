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
//!
//! RULES.md D210, D212, D219: a `\` in code begins a backslash command,
//! which runs to the end of its line (`command.rs`); a `:` in code may
//! begin a reference to a variable, its value read in its place and split
//! as if typed; and `\delimiter` may name another text to end a statement
//! with, which ends one wherever it stands in code, as in a routine's body.

use crate::vars::{Variables, reference};

/// What begins a backslash command, in code.
const COMMAND: char = '\\';
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

/// What text splits into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// A statement, ended.
    Statement(String),
    /// A backslash command: the line after its `\`, without its newline.
    Command(String),
}

/// What splits text, given in parts, into statements.
#[derive(Debug, Default)]
pub struct Splitter {
    /// What ends a statement in place of `;`, if `\delimiter` named it.
    delimiter: Option<String>,
    /// How many characters end the text read in code, for a delimiter.
    code_run: usize,
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
    /// The statements `chunk` ends, with what came before it, read as
    /// SQL alone: no command, no variable.
    pub fn feed(&mut self, chunk: &str) -> Vec<String> {
        chunk.chars().filter_map(|c| self.take(c)).collect()
    }

    /// The statements and commands `chunk`, a line, ends, with what came
    /// before it, the variables of `vars` read into it if given.
    pub fn pieces(&mut self, chunk: &str, vars: Option<&Variables>) -> Vec<Piece> {
        let chars: Vec<char> = chunk.chars().collect();
        let mut done = Vec::new();
        let mut at = 0_usize;
        while let Some(&c) = chars.get(at) {
            let next = at.saturating_add(1);
            let code = self.state == State::Code;
            if code && c == COMMAND {
                let rest: String = chars.get(next..).unwrap_or_default().iter().take_while(|ch| **ch != '\n').collect();
                self.dash = false;
                done.push(Piece::Command(rest));
                break;
            }
            // A colon of `::`, a cast, begins no reference.
            let cast = at.checked_sub(1).and_then(|p| chars.get(p)) == Some(&':') || chars.get(next) == Some(&':');
            let read = vars.filter(|_| code && c == ':' && !cast).and_then(|v| reference(&chars, at, v));
            let (text, after) = read.unwrap_or_else(|| (c.to_string(), next));
            done.extend(text.chars().filter_map(|t| self.take(t)).map(Piece::Statement));
            at = after;
        }
        done
    }

    /// What ends a statement from now on: `delimiter`, or `;` for none.
    ///
    /// # Errors
    ///
    /// It is empty, or holds a space, a quote, a minus sign or a `\`.
    pub fn set_delimiter(&mut self, delimiter: Option<String>) -> Result<(), String> {
        let bad = |c: char| c.is_whitespace() || ['\'', '"', '-', COMMAND].contains(&c);
        if let Some(d) = &delimiter
            && (d.is_empty() || d.chars().any(bad))
        {
            return Err(format!("{d:?} cannot end a statement: it holds no spaces, quotes, minus signs or backslashes"));
        }
        self.delimiter = delimiter.filter(|d| d != ";");
        Ok(())
    }

    /// The statement begun, if one is, the text begun anew.
    pub fn take_pending(&mut self) -> Option<String> {
        self.finish()
    }

    /// The text of the statement begun, as read so far.
    pub fn pending_text(&self) -> &str {
        &self.text
    }

    /// The statement begun thrown away.
    pub fn reset(&mut self) {
        drop(self.finish());
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
                if self.depth == 0 && self.delimiter.is_none() {
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
        self.code_run = if self.state == State::Code && !in_comment { self.code_run.saturating_add(1) } else { 0 };
        self.delimited()
    }

    /// The statement a delimiter `\delimiter` named ends, if the text read
    /// in code ends with it: the delimiter cut.
    fn delimited(&mut self) -> Option<String> {
        let delimiter = self.delimiter.as_deref()?;
        if self.code_run < delimiter.chars().count() || !self.text.ends_with(delimiter) {
            return None;
        }
        let cut = self.text.len().saturating_sub(delimiter.len());
        self.text.truncate(cut);
        self.comment_end = self.comment_end.min(cut);
        self.ended()
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
        (self.last, self.depth, self.lead, self.code_run) = (Last::Other, 0, Lead::Start, 0);
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
mod tests;
