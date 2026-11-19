//! XQuery's lexical structure [XQuery Appendix A.2]: the characters of a
//! query, its line ends normalized [Appendix A.2.3], read as tokens on
//! the parser's demand.
//!
//! The grammar is context-sensitive [Appendix A.2]: keywords are not
//! reserved, so a name is a token of its own and the parser decides by
//! where it is whether it is a keyword; and the direct constructors are
//! read by explicit whitespace rules, where no comment is recognized
//! [Appendix A.2.4.2]. So the lexer has two states. In the default state
//! (`Lexer::token`) it skips whitespace and comments [Appendix A.2.4.1]
//! and reads the longest token valid there [Appendix A.2]; the parser
//! switches it, at a `<` of a constructor, to reading characters
//! (`Lexer::peek_char` and the reads of `lexer/scan.rs`) for the
//! constructor's tags, attribute values and content, and back to tokens
//! at each enclosed expression and after the constructor ends.

mod scan;

use super::ast::{Pragma, QName};
use super::error::{ErrorCode, Place, xquery_error};
use crate::error::{Result, SqlError};
use crate::xml::{Version, is_char, is_space, normalized};

/// A token of the default state [XQuery Appendix A.2.1, A.2.2].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Tok {
    /// A `QName` or `NCName` \[154\], \[155\], keywords among them.
    Name(QName),
    /// `NCName:*` [Appendix A.1 \[80\]], read whole as whitespace may not
    /// break it.
    PrefixWildcard(String),
    /// `*:NCName`.
    LocalWildcard(String),
    /// `IntegerLiteral` \[141\], as written.
    Integer(String),
    /// `DecimalLiteral` \[142\].
    Decimal(String),
    /// `DoubleLiteral` \[143\].
    Double(String),
    /// `StringLiteral` \[144\]: its value.
    String(String),
    /// `Pragma` [Appendix A.1 \[66\]], read whole as its whitespace is
    /// explicit.
    Pragma(Pragma),
    /// A delimiting terminal [Appendix A.2.2].
    Sym(Sym),
    /// The end of the text.
    End,
}

/// A delimiting terminal of the default state [XQuery Appendix A.2.2].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Sym {
    /// `=`.
    Eq,
    /// `!=`.
    Ne,
    /// `<`, an operator or a direct element constructor's start.
    Lt,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
    /// `<<`.
    Precedes,
    /// `>>`.
    Follows,
    /// `:=`.
    Assign,
    /// `;`.
    Semicolon,
    /// `,`.
    Comma,
    /// `$`.
    Dollar,
    /// `(`.
    LParen,
    /// `)`.
    RParen,
    /// `[`.
    LBracket,
    /// `]`.
    RBracket,
    /// `{`.
    LBrace,
    /// `}`.
    RBrace,
    /// `::`.
    Axis,
    /// `@`.
    At,
    /// `.`.
    Dot,
    /// `..`.
    DotDot,
    /// `*`.
    Star,
    /// `?`.
    Question,
    /// `+`.
    Plus,
    /// `-`.
    Minus,
    /// `/`.
    Slash,
    /// `//`.
    SlashSlash,
    /// `|`.
    Bar,
    /// `<!--`, a direct comment constructor's start.
    CommentStart,
    /// `<?`, a direct processing instruction constructor's start.
    PiStart,
}

/// [XQuery Appendix A.2.2] The delimiting terminals of the default state,
/// each longer one before those it begins with, so the first that matches
/// is the longest [Appendix A.2].
const SYMBOLS: [(&str, Sym); 31] = [
    ("<!--", Sym::CommentStart),
    ("::", Sym::Axis),
    (":=", Sym::Assign),
    ("!=", Sym::Ne),
    ("<=", Sym::Le),
    ("<<", Sym::Precedes),
    ("<?", Sym::PiStart),
    (">=", Sym::Ge),
    (">>", Sym::Follows),
    ("..", Sym::DotDot),
    ("//", Sym::SlashSlash),
    ("=", Sym::Eq),
    ("<", Sym::Lt),
    (">", Sym::Gt),
    (";", Sym::Semicolon),
    (",", Sym::Comma),
    ("$", Sym::Dollar),
    ("(", Sym::LParen),
    (")", Sym::RParen),
    ("[", Sym::LBracket),
    ("]", Sym::RBracket),
    ("{", Sym::LBrace),
    ("}", Sym::RBrace),
    ("@", Sym::At),
    (".", Sym::Dot),
    ("*", Sym::Star),
    ("?", Sym::Question),
    ("+", Sym::Plus),
    ("-", Sym::Minus),
    ("/", Sym::Slash),
    ("|", Sym::Bar),
];

/// How `sym` is written.
pub(super) fn symbol_text(sym: Sym) -> &'static str {
    SYMBOLS.iter().find(|&&(_, s)| s == sym).map_or("", |&(text, _)| text)
}

/// [XQuery Appendix A.2.1 \[151\]] What opens a comment.
const COMMENT_OPEN: &str = "(:";
/// What closes one.
const COMMENT_CLOSE: &str = ":)";
/// [XQuery Appendix A.1 \[66\]] What opens a pragma.
const PRAGMA_OPEN: &str = "(#";
/// What closes one.
const PRAGMA_CLOSE: &str = "#)";

/// A token, and where in the text it is.
#[derive(Debug, Clone)]
pub(super) struct Token {
    /// The token, or the error reading it raised, which the parser raises
    /// only if it reads that far: a parser that switches the lexer to
    /// reading characters drops the tokens it had looked ahead to.
    pub(super) tok: std::result::Result<Tok, Box<SqlError>>,
    /// Where its first character is.
    pub(super) at: usize,
    /// Where the character after it is.
    pub(super) end: usize,
}

/// A query's text being read.
#[derive(Debug)]
pub(super) struct Lexer {
    /// The text, its line ends normalized.
    chars: Vec<char>,
    /// Where the next character to read is.
    at: usize,
    /// The XML version whose lexical rules apply [XQuery Appendix A.2].
    version: Version,
}

impl Lexer {
    /// A lexer of `text` by XML `version`'s rules.
    ///
    /// # Errors
    ///
    /// err:XPST0003 if `text` holds a character that is no `Char` of
    /// `version` [XQuery Appendix A.2.1 \[157\]].
    pub(super) fn new(text: &str, version: Version) -> Result<Lexer> {
        let raw: Vec<char> = text.chars().collect();
        let chars = normalized(version, &raw);
        let lexer = Lexer { chars, at: 0, version };
        if let Some(bad) = lexer.chars.iter().position(|&c| !is_char(version, c)) {
            let shown = lexer.chars.get(bad).map_or(0, |&c| u32::from(c));
            return Err(lexer
                .error(bad, &format!("the character #x{shown:X} is no Char of XML {} [XQuery Appendix A.2.1 [157]]", version.number())));
        }
        Ok(lexer)
    }

    /// The version whose lexical rules apply.
    pub(super) fn version(&self) -> Version {
        self.version
    }

    /// Where the next character to read is.
    pub(super) fn position(&self) -> usize {
        self.at
    }

    /// Moves to `at`, where the parser goes on reading in another state.
    pub(super) fn seek(&mut self, at: usize) {
        self.at = at;
    }

    /// Where the character at `at` is, by line and column.
    pub(super) fn place(&self, at: usize) -> Place {
        Place::after(self.chars.get(..at).unwrap_or_default())
    }

    /// The syntax error err:XPST0003 at `at`, `why` naming the production
    /// broken [XQuery Appendix F].
    pub(super) fn error(&self, at: usize, why: &str) -> SqlError {
        xquery_error(ErrorCode::XPST0003, self.place(at), why)
    }

    /// The character `ahead` characters after the next.
    pub(super) fn char_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.at.saturating_add(ahead)).copied()
    }

    /// The character at `index` of the text.
    pub(super) fn char_at_index(&self, index: usize) -> Option<char> {
        self.chars.get(index).copied()
    }

    /// The next character.
    pub(super) fn peek_char(&self) -> Option<char> {
        self.char_at(0)
    }

    /// Whether the text from the next character starts with `text`.
    pub(super) fn looking_at(&self, text: &str) -> bool {
        text.chars().zip(self.at..).all(|(c, at)| self.chars.get(at) == Some(&c))
    }

    /// Reads past `text` if the next characters are it.
    pub(super) fn eat(&mut self, text: &str) -> bool {
        let matched = self.looking_at(text);
        if matched {
            self.at = self.at.saturating_add(text.chars().count());
        }
        matched
    }

    /// Reads past `c` if it is next.
    pub(super) fn eat_char(&mut self, c: char) -> bool {
        let matched = self.peek_char() == Some(c);
        if matched {
            self.at = self.at.saturating_add(1);
        }
        matched
    }

    /// Reads the next character.
    pub(super) fn bump(&mut self) -> Option<char> {
        let c = self.peek_char();
        if c.is_some() {
            self.at = self.at.saturating_add(1);
        }
        c
    }

    /// Reads whitespace, `S` [XQuery Appendix A.2.1 \[156\]], and says
    /// whether there was any.
    pub(super) fn space(&mut self) -> bool {
        let from = self.at;
        while self.peek_char().is_some_and(is_space) {
            self.at = self.at.saturating_add(1);
        }
        self.at != from
    }

    /// The next token of the default state, after the whitespace and
    /// comments before it [XQuery Appendix A.2.4.1].
    pub(super) fn token(&mut self) -> Token {
        let read = self.separators().and_then(|()| {
            let at = self.at;
            self.next_tok().map(|tok| (at, tok))
        });
        match read {
            Ok((at, tok)) => Token { tok: Ok(tok), at, end: self.at },
            Err(error) => Token { tok: Err(Box::new(error)), at: self.at, end: self.at },
        }
    }

    /// Reads whitespace and comments, comments nesting [XQuery Appendix
    /// A.1.3, grammar-note: comments].
    fn separators(&mut self) -> Result<()> {
        loop {
            self.space();
            if !self.looking_at(COMMENT_OPEN) {
                return Ok(());
            }
            self.comment()?;
        }
    }

    /// Reads a comment [XQuery Appendix A.2.1 \[151\]]: to the `:)` that
    /// balances its `(:`, however many open within it.
    fn comment(&mut self) -> Result<()> {
        let start = self.at;
        let mut open = 0_usize;
        loop {
            if self.eat(COMMENT_OPEN) {
                open = open.saturating_add(1);
            } else if self.eat(COMMENT_CLOSE) {
                open = open.saturating_sub(1);
                if open == 0 {
                    return Ok(());
                }
            } else if self.bump().is_none() {
                return Err(self.error(start, "a comment is not closed by ':)' [XQuery Appendix A.2.1 [151]]"));
            }
        }
    }

    /// The token at the next character, which is no whitespace.
    fn next_tok(&mut self) -> Result<Tok> {
        let Some(c) = self.peek_char() else {
            return Ok(Tok::End);
        };
        if c.is_ascii_digit() || (c == '.' && self.char_at(1).is_some_and(|d| d.is_ascii_digit())) {
            return self.numeric();
        }
        if c == '"' || c == '\'' {
            return self.string_literal().map(Tok::String);
        }
        if self.looking_at(PRAGMA_OPEN) {
            return self.pragma().map(Tok::Pragma);
        }
        if self.is_name_start(c) {
            return Ok(self.name_token());
        }
        if c == '*' && self.char_at(1) == Some(':') && self.char_at(2).is_some_and(|n| self.is_name_start(n)) {
            self.at = self.at.saturating_add(2);
            return Ok(Tok::LocalWildcard(self.ncname().unwrap_or_default()));
        }
        self.symbol().map(Tok::Sym)
    }

    /// The delimiting terminal at the next character.
    fn symbol(&mut self) -> Result<Sym> {
        if let Some(&(text, sym)) = SYMBOLS.iter().find(|(text, _)| self.looking_at(text)) {
            self.at = self.at.saturating_add(text.chars().count());
            return Ok(sym);
        }
        let c = self.peek_char().unwrap_or_default();
        Err(self.error(self.at, &format!("'{c}' begins no token of XQuery [XQuery Appendix A.2.2]")))
    }

    /// The name token at the next character, a name start character: a
    /// QName [XQuery Appendix A.2.1 \[154\]], whose colon no whitespace or
    /// comment may surround [Appendix A.1.2, xml-version], or a wildcard
    /// `NCName:*`.
    fn name_token(&mut self) -> Tok {
        let first = self.ncname().unwrap_or_default();
        if self.peek_char() == Some(':') {
            if self.char_at(1) == Some('*') {
                self.at = self.at.saturating_add(2);
                return Tok::PrefixWildcard(first);
            }
            if self.char_at(1).is_some_and(|c| self.is_name_start(c)) {
                self.at = self.at.saturating_add(1);
                let local = self.ncname().unwrap_or_default();
                return Tok::Name(QName { prefix: Some(first), local });
            }
        }
        Tok::Name(QName { prefix: None, local: first })
    }

    /// Reads a pragma [XQuery Appendix A.1 \[66\]], its whitespace explicit:
    /// `(#`, whitespace, its QName, then its contents to the first `#)`.
    fn pragma(&mut self) -> Result<Pragma> {
        let start = self.at;
        self.eat(PRAGMA_OPEN);
        self.space();
        let name =
            self.qname().ok_or_else(|| self.error(self.at, "a pragma names itself by a QName after '(#' [XQuery Appendix A.1 [66]]"))?;
        let from = self.at;
        while !self.looking_at(PRAGMA_CLOSE) {
            if self.bump().is_none() {
                return Err(self.error(start, "a pragma is not closed by '#)' [XQuery Appendix A.1 [66]]"));
            }
        }
        let contents = self.chars.get(from..self.at).unwrap_or_default().iter().collect();
        self.eat(PRAGMA_CLOSE);
        Ok(Pragma { name, contents })
    }
}

#[cfg(test)]
mod tests;
