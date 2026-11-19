//! \[DESIGN\] The tokens of a Snowball program (RULES.md D320): its key
//! words, names, numbers, symbols and literal strings, as the Snowball
//! compiler's tokeniser reads them (snowballstem.org, `compiler/
//! tokeniser.c`). Comments, `stringescapes` and `stringdef` are taken here,
//! as they change how later strings are read, and give no token.
//!
//! A literal string is written between apostrophes. Between the two
//! escape characters `stringescapes` names, a name `stringdef` defined
//! stands for its string, `U+` and hexadecimal digits for one character,
//! an apostrophe or the opening escape character for itself, and white
//! space holding a line end for nothing, so a string goes on across lines.
//! The legacy `hex` and `decimal` forms of `stringdef` are not read.

use std::collections::HashMap;

use super::damaged;
use crate::error::Result;

/// A key word of Snowball.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Key {
    As,
    Do,
    Or,
    And,
    For,
    Get,
    Hex,
    Hop,
    Len,
    Non,
    Not,
    Set,
    Try,
    Fail,
    Goto,
    Loop,
    Next,
    Size,
    Test,
    True,
    Among,
    False,
    Lenof,
    Limit,
    Unset,
    Atmark,
    Attach,
    Cursor,
    Define,
    Delete,
    Gopast,
    Insert,
    Maxint,
    Minint,
    Repeat,
    Sizeof,
    Tomark,
    Atleast,
    Atlimit,
    Decimal,
    Reverse,
    Setmark,
    Strings,
    Tolimit,
    Booleans,
    Integers,
    Routines,
    Setlimit,
    Backwards,
    Externals,
    Groupings,
    Stringdef,
    Substring,
    Backwardmode,
    Stringescapes,
}

/// The key words, by their spelling (`compiler/tokens.h`).
const KEY_WORDS: [(&str, Key); 55] = [
    ("as", Key::As),
    ("do", Key::Do),
    ("or", Key::Or),
    ("and", Key::And),
    ("for", Key::For),
    ("get", Key::Get),
    ("hex", Key::Hex),
    ("hop", Key::Hop),
    ("len", Key::Len),
    ("non", Key::Non),
    ("not", Key::Not),
    ("set", Key::Set),
    ("try", Key::Try),
    ("fail", Key::Fail),
    ("goto", Key::Goto),
    ("loop", Key::Loop),
    ("next", Key::Next),
    ("size", Key::Size),
    ("test", Key::Test),
    ("true", Key::True),
    ("among", Key::Among),
    ("false", Key::False),
    ("lenof", Key::Lenof),
    ("limit", Key::Limit),
    ("unset", Key::Unset),
    ("atmark", Key::Atmark),
    ("attach", Key::Attach),
    ("cursor", Key::Cursor),
    ("define", Key::Define),
    ("delete", Key::Delete),
    ("gopast", Key::Gopast),
    ("insert", Key::Insert),
    ("maxint", Key::Maxint),
    ("minint", Key::Minint),
    ("repeat", Key::Repeat),
    ("sizeof", Key::Sizeof),
    ("tomark", Key::Tomark),
    ("atleast", Key::Atleast),
    ("atlimit", Key::Atlimit),
    ("decimal", Key::Decimal),
    ("reverse", Key::Reverse),
    ("setmark", Key::Setmark),
    ("strings", Key::Strings),
    ("tolimit", Key::Tolimit),
    ("booleans", Key::Booleans),
    ("integers", Key::Integers),
    ("routines", Key::Routines),
    ("setlimit", Key::Setlimit),
    ("backwards", Key::Backwards),
    ("externals", Key::Externals),
    ("groupings", Key::Groupings),
    ("stringdef", Key::Stringdef),
    ("substring", Key::Substring),
    ("backwardmode", Key::Backwardmode),
    ("stringescapes", Key::Stringescapes),
];

/// A symbol of Snowball.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Sym {
    Dollar,
    Bra,
    Ket,
    Multiply,
    Plus,
    Minus,
    Divide,
    Lt,
    Assign,
    Gt,
    Debug,
    LeftSlice,
    RightSlice,
    Ne,
    MultiplyAssign,
    PlusAssign,
    MinusAssign,
    SliceTo,
    DivideAssign,
    InsertSym,
    SliceFrom,
    Le,
    Eq,
    AssignTo,
    Ge,
}

/// The symbols, the two-character ones first so they are found before
/// their first character alone.
const SYMBOLS: [(&str, Sym); 25] = [
    ("!=", Sym::Ne),
    ("*=", Sym::MultiplyAssign),
    ("+=", Sym::PlusAssign),
    ("-=", Sym::MinusAssign),
    ("->", Sym::SliceTo),
    ("/=", Sym::DivideAssign),
    ("<-", Sym::SliceFrom),
    ("<=", Sym::Le),
    ("<+", Sym::InsertSym),
    ("==", Sym::Eq),
    ("=>", Sym::AssignTo),
    (">=", Sym::Ge),
    ("$", Sym::Dollar),
    ("(", Sym::Bra),
    (")", Sym::Ket),
    ("*", Sym::Multiply),
    ("+", Sym::Plus),
    ("-", Sym::Minus),
    ("/", Sym::Divide),
    ("<", Sym::Lt),
    ("=", Sym::Assign),
    (">", Sym::Gt),
    ("?", Sym::Debug),
    ("[", Sym::LeftSlice),
    ("]", Sym::RightSlice),
];

/// The opening of a line comment.
const LINE_COMMENT: &str = "//";
/// The opening of a block comment.
const BLOCK_COMMENT: &str = "/*";
/// The close of a block comment.
const BLOCK_END: &str = "*/";
/// The opening of a literal string, and its close.
const QUOTE: char = '\'';
/// What a `U+` character escape begins with.
const CODE_POINT: &str = "U+";
/// The radix of a `U+` character escape.
const HEX: u32 = 16;

/// A token of the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Token {
    Key(Key),
    Name(String),
    Number(i64),
    Text(Vec<char>),
    Sym(Sym),
}

/// A token and the line it is on, from 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Lexed {
    pub token: Token,
    pub line: u32,
}

/// The tokenizer's state.
struct Lexer {
    chars: Vec<char>,
    at: usize,
    line: u32,
    /// The escape characters `stringescapes` named, if it has.
    escapes: Option<(char, char)>,
    /// The strings `stringdef` defined, by name.
    defined: HashMap<String, Vec<char>>,
    out: Vec<Lexed>,
}

/// The tokens of the Snowball program `source`.
///
/// # Errors
///
/// SQLCODE -916 if it is not made of Snowball's tokens.
pub(super) fn lex(source: &str) -> Result<Vec<Lexed>> {
    let mut lexer = Lexer { chars: source.chars().collect(), at: 0, line: 1, escapes: None, defined: HashMap::new(), out: Vec::new() };
    while let Some(c) = lexer.peek() {
        lexer.step(c)?;
    }
    Ok(lexer.out)
}

impl Lexer {
    /// The character at the cursor.
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    /// The character at the cursor, taken, lines counted.
    fn take(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at = self.at.saturating_add(1);
        if c == '\n' {
            self.line = self.line.saturating_add(1);
        }
        Some(c)
    }

    /// Whether `text` is next.
    fn looking_at(&self, text: &str) -> bool {
        text.chars().enumerate().all(|(i, c)| self.chars.get(self.at.saturating_add(i)) == Some(&c))
    }

    /// `token` added at the current line.
    fn push(&mut self, token: Token) {
        self.out.push(Lexed { token, line: self.line });
    }

    /// The token, comment or directive beginning with `c`.
    fn step(&mut self, c: char) -> Result<()> {
        if c.is_whitespace() {
            self.take();
            return Ok(());
        }
        if c.is_ascii_alphabetic() {
            return self.word();
        }
        if c.is_ascii_digit() {
            return self.number();
        }
        if c == QUOTE {
            self.take();
            let text = self.literal()?;
            self.push(Token::Text(text));
            return Ok(());
        }
        if self.looking_at(LINE_COMMENT) {
            while self.peek().is_some_and(|c| c != '\n') {
                self.take();
            }
            return Ok(());
        }
        if self.looking_at(BLOCK_COMMENT) {
            return self.block_comment();
        }
        self.symbol()
    }

    /// A block comment, to its close.
    fn block_comment(&mut self) -> Result<()> {
        let line = self.line;
        while !self.looking_at(BLOCK_END) {
            if self.take().is_none() {
                return Err(damaged(line, "a comment is not closed"));
            }
        }
        self.take();
        self.take();
        Ok(())
    }

    /// A key word or name, or the directive a key word begins.
    fn word(&mut self) -> Result<()> {
        let mut word = String::new();
        while let Some(c) = self.peek().filter(|c| c.is_ascii_alphanumeric() || *c == '_') {
            word.push(c);
            self.take();
        }
        match KEY_WORDS.iter().find(|(spelling, _)| *spelling == word).map(|(_, key)| *key) {
            Some(Key::Stringescapes) => self.stringescapes(),
            Some(Key::Stringdef) => self.stringdef(),
            Some(key) => {
                self.push(Token::Key(key));
                Ok(())
            }
            None => {
                self.push(Token::Name(word));
                Ok(())
            }
        }
    }

    /// A number.
    fn number(&mut self) -> Result<()> {
        let mut value: i64 = 0;
        while let Some(digit) = self.peek().and_then(|c| c.to_digit(10)) {
            value = value.checked_mul(10).and_then(|v| v.checked_add(i64::from(digit))).ok_or_else(|| damaged(self.line, "a number is too large"))?;
            self.take();
        }
        self.push(Token::Number(value));
        Ok(())
    }

    /// The next character that is not white space.
    fn next_real(&mut self) -> Option<char> {
        while self.peek().is_some_and(char::is_whitespace) {
            self.take();
        }
        self.take()
    }

    /// `stringescapes`, its two escape characters.
    fn stringescapes(&mut self) -> Result<()> {
        let (Some(open), Some(close)) = (self.next_real(), self.next_real()) else {
            return Err(damaged(self.line, "stringescapes names two characters"));
        };
        self.escapes = Some((open, close));
        Ok(())
    }

    /// `stringdef`, a name, a run of characters other than white space,
    /// and its string.
    fn stringdef(&mut self) -> Result<()> {
        let mut name = String::new();
        if let Some(first) = self.next_real() {
            name.push(first);
        }
        while let Some(c) = self.peek().filter(|c| !c.is_whitespace()) {
            name.push(c);
            self.take();
        }
        if self.next_real() != Some(QUOTE) || name.is_empty() {
            return Err(damaged(self.line, "stringdef names a string, written between apostrophes"));
        }
        let text = self.literal()?;
        self.defined.insert(name, text);
        Ok(())
    }

    /// A literal string's characters, its opening apostrophe taken, to
    /// its close.
    fn literal(&mut self) -> Result<Vec<char>> {
        let mut text = Vec::new();
        loop {
            let c = self.take().filter(|c| *c != '\n').ok_or_else(|| damaged(self.line, "a string is not closed"))?;
            if let Some((open, close)) = self.escapes.filter(|(open, _)| *open == c) {
                self.escape(open, close, &mut text)?;
            } else if c == QUOTE {
                return Ok(text);
            } else {
                text.push(c);
            }
        }
    }

    /// What is between the escape characters `open` and `close`, the first
    /// taken, added to `text`.
    fn escape(&mut self, open: char, close: char, text: &mut Vec<char>) -> Result<()> {
        let mut inside = String::new();
        loop {
            let c = self.take().ok_or_else(|| damaged(self.line, "a string is not closed"))?;
            if c == close {
                break;
            }
            inside.push(c);
        }
        if inside.contains('\n') && inside.trim().is_empty() {
            return Ok(());
        }
        if let Some(defined) = self.defined.get(&inside) {
            text.extend_from_slice(defined);
            return Ok(());
        }
        let single = if inside.chars().count() == 1 { inside.chars().next() } else { None };
        if let Some(c) = single.filter(|c| *c == QUOTE || *c == open) {
            text.push(c);
            return Ok(());
        }
        let code = inside.strip_prefix(CODE_POINT).and_then(|hex| u32::from_str_radix(hex, HEX).ok()).and_then(char::from_u32);
        text.push(code.ok_or_else(|| damaged(self.line, &format!("no string is defined as {inside}")))?);
        Ok(())
    }

    /// A symbol.
    fn symbol(&mut self) -> Result<()> {
        let Some((spelling, sym)) = SYMBOLS.iter().find(|(spelling, _)| self.looking_at(spelling)).copied() else {
            return Err(damaged(self.line, &format!("{} is no symbol", self.peek().unwrap_or_default())));
        };
        for _ in spelling.chars() {
            self.take();
        }
        self.push(Token::Sym(sym));
        Ok(())
    }
}
