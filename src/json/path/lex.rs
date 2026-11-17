//! The tokens of an SQL/JSON path expression [ISO/IEC 9075-2:2023 §9.45]:
//! by the lexical rules of ECMAScript 5.1 [§9.45 SR2], with the goal
//! symbol InputElementDiv alone \[SR2b\], no comments \[SR2c\], no reserved
//! words \[SR2d\], `@` a punctuator \[SR2e\], no regular expression literals
//! \[SR2f\] and no semicolons inserted \[SR2g\].
//!
//! A token is an identifier [ECMAScript 5.1 §7.6], a `<SQL/JSON special
//! symbol>` [§9.45 F], a numeric literal [ECMAScript 5.1 §7.8.3] or a
//! string literal [ECMAScript 5.1 §7.8.4]. White space [ECMAScript 5.1
//! §7.2] and line terminators [ECMAScript 5.1 §7.3] separate tokens, and
//! the longest sequence of characters that is a token is taken [ECMAScript
//! 5.1 §7]. \[DESIGN\] A character no token begins with is refused here,
//! though it is an ECMAScript punctuator, since no SQL/JSON path
//! expression holds one.

use super::{beyond, lexical};
use crate::error::Result;
use crate::json::Item;
use crate::json::text::{HEX, HIGH_SURROGATES, approximate, exact, paired};
use crate::regex::general_category;

// [ECMAScript 5.1 §7.2, §7.3, §7.1 Table 1] Characters each class of
// which is named, not found by its Unicode General Category.
/// <VT>, a white space character.
const VT: char = '\u{B}';
/// <FF>, a white space character.
const FF: char = '\u{C}';
/// <NBSP>, a white space character.
const NBSP: char = '\u{A0}';
/// <BOM>, a white space character.
const BOM: char = '\u{FEFF}';
/// <LS>, a line terminator.
const LS: char = '\u{2028}';
/// <PS>, a line terminator.
const PS: char = '\u{2029}';
/// <ZWNJ>, an IdentifierPart.
const ZWNJ: char = '\u{200C}';
/// <ZWJ>, an IdentifierPart.
const ZWJ: char = '\u{200D}';

// [ECMAScript 5.1 §7.6, §7.2] The characters by their Unicode General
// Categories, as `regex::general_category` names them.
/// UnicodeLetter, which begins an identifier.
const LETTERS: [&str; 6] = ["Lu", "Ll", "Lt", "Lm", "Lo", "Nl"];
/// UnicodeCombiningMark, UnicodeDigit and UnicodeConnectorPunctuation,
/// which continue one.
const PARTS: [&str; 4] = ["Mn", "Mc", "Nd", "Pc"];
/// <USP>, white space.
const SPACES: &str = "Zs";

/// [ECMAScript 5.1 §7.8.3] How many characters begin a HexIntegerLiteral:
/// `0x` or `0X`.
const HEX_PREFIX_LENGTH: usize = 2;
/// The radix of decimal digits.
const DECIMAL: u32 = 10;
/// \[DESIGN\] The most decimal digits a HexIntegerLiteral's value is
/// written in as it is read: of more, it is past the range of every
/// decimal floating-point number, whose exponent is at most 6144 and
/// coefficient of 34 digits, so its reading stops, and with it the work of
/// a text of many digits.
const MAX_HEX_VALUE_DIGITS: usize = 6144 + 34;

/// [ECMAScript 5.1 §7.8.4] How many hexadecimal digits a HexEscapeSequence
/// has.
const HEX_ESCAPE_DIGITS: u32 = 2;
/// [ECMAScript 5.1 §7.8.4] How many hexadecimal digits a
/// UnicodeEscapeSequence has.
const UNICODE_ESCAPE_DIGITS: u32 = 4;

/// A `<SQL/JSON special symbol>` [§9.45 F], but the `<dollar sign>`, which
/// begins an identifier [ECMAScript 5.1 §7.6].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Symbol {
    /// `*`.
    Asterisk,
    /// `@`.
    At,
    /// `,`.
    Comma,
    /// `&&`.
    DoubleAmpersand,
    /// `==`.
    DoubleEquals,
    /// `||`.
    DoubleBar,
    /// `!`.
    Exclamation,
    /// `>`.
    Greater,
    /// `>=`.
    GreaterEquals,
    /// `[`.
    LeftBracket,
    /// `(`.
    LeftParen,
    /// `<`.
    Less,
    /// `<=`.
    LessEquals,
    /// `-`.
    Minus,
    /// `<not equals operator>`, `<>`, or `<alternative not equals
    /// operator>`, `!=`: one token, since the Format takes either for the
    /// other.
    NotEquals,
    /// `%`.
    Percent,
    /// `.`.
    Period,
    /// `+`.
    Plus,
    /// `?`.
    Question,
    /// `]`.
    RightBracket,
    /// `)`.
    RightParen,
    /// `/`.
    Solidus,
}

/// The special symbols but the period, each as written, those of two
/// characters before those of one, so the longest is taken.
pub(super) const SYMBOLS: [(&str, Symbol); 22] = [
    ("!=", Symbol::NotEquals),
    ("<>", Symbol::NotEquals),
    ("<=", Symbol::LessEquals),
    (">=", Symbol::GreaterEquals),
    ("==", Symbol::DoubleEquals),
    ("&&", Symbol::DoubleAmpersand),
    ("||", Symbol::DoubleBar),
    ("!", Symbol::Exclamation),
    ("<", Symbol::Less),
    (">", Symbol::Greater),
    ("@", Symbol::At),
    ("*", Symbol::Asterisk),
    (",", Symbol::Comma),
    ("[", Symbol::LeftBracket),
    ("]", Symbol::RightBracket),
    ("(", Symbol::LeftParen),
    (")", Symbol::RightParen),
    ("-", Symbol::Minus),
    ("+", Symbol::Plus),
    ("%", Symbol::Percent),
    ("?", Symbol::Question),
    ("/", Symbol::Solidus),
];

/// A token of a path expression.
#[derive(Debug, Clone)]
pub(super) enum Token {
    /// A special symbol.
    Symbol(Symbol),
    /// [ECMAScript 5.1 §7.6] An IdentifierName: its characters, each
    /// escape replaced by the character it stands for. A key word, a
    /// variable or a key name, as the parser takes it [§9.45 SR2d, SR4].
    Identifier(String),
    /// [§9.45 GR1b] A `<JSON path string literal>`: its characters, each
    /// escape replaced.
    String(String),
    /// [§9.45 GR1a] A `<JSON path numeric literal>`: the number it is,
    /// and how it is written.
    Number(Item, Numeral),
}

/// How a numeric literal is written [ECMAScript 5.1 §7.8.3].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Numeral {
    /// A DecimalIntegerLiteral alone, its digits all that is written: what
    /// an `<unsigned integer>` may be too, as the precision of a `<JSON
    /// method>` is [§9.46 F].
    Integer,
    /// Any other: a DecimalLiteral of a period or an ExponentPart, or a
    /// HexIntegerLiteral.
    Other,
}

/// A token and where it begins: its first character, counted from 0.
#[derive(Debug, Clone)]
pub(super) struct Lexeme {
    /// The token.
    pub(super) token: Token,
    /// Where it begins.
    pub(super) at: usize,
}

/// What a character may be outside a string literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// IdentifierStart, but its escapes [ECMAScript 5.1 §7.6].
    Start,
    /// IdentifierPart but not IdentifierStart.
    Part,
    /// WhiteSpace [ECMAScript 5.1 §7.2] or LineTerminator [§7.3].
    Separator,
    /// None of them.
    Other,
}

/// A path expression's text being split into tokens.
struct Lexer {
    chars: Vec<char>,
    at: usize,
}

/// The tokens of the path expression `text`.
///
/// # Errors
///
/// SQLCODE -101 if `text` is not of the path language's tokens and
/// separators; SQLCODE -900 for a numeric literal out of every number's
/// range.
pub(super) fn lex(text: &str) -> Result<Vec<Lexeme>> {
    let mut lexer = Lexer { chars: text.chars().collect(), at: 0 };
    let mut lexemes = Vec::new();
    loop {
        lexer.separators()?;
        let at = lexer.at;
        let Some(c) = lexer.peek() else { return Ok(lexemes) };
        let token = lexer.token(c)?;
        lexemes.push(Lexeme { token, at });
    }
}

/// [ECMAScript 5.1 §7.3] Whether `c` is a LineTerminator.
fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | LS | PS)
}

/// The class of the ASCII character `c`.
fn ascii_class(c: char) -> Class {
    match c {
        'a'..='z' | 'A'..='Z' | '$' | '_' => Class::Start,
        '0'..='9' => Class::Part,
        '\t' | VT | FF | ' ' | '\n' | '\r' => Class::Separator,
        _ => Class::Other,
    }
}

/// The class of `c`: of ASCII by `ascii_class`, of the characters named
/// above by their names, of any other by its General Category.
///
/// # Errors
///
/// SQLCODE -916 if the table of General Categories is damaged.
fn class_of(c: char) -> Result<Class> {
    if c.is_ascii() {
        return Ok(ascii_class(c));
    }
    match c {
        NBSP | BOM | LS | PS => return Ok(Class::Separator),
        ZWNJ | ZWJ => return Ok(Class::Part),
        _ => {}
    }
    let category = general_category(c)?;
    Ok(if LETTERS.contains(&category) {
        Class::Start
    } else if PARTS.contains(&category) {
        Class::Part
    } else if category == SPACES {
        Class::Separator
    } else {
        Class::Other
    })
}

/// `decimal`, the decimal digits of a number, least significant first,
/// made those of 16 times the number plus `digit`, a hexadecimal digit's
/// value.
fn times_sixteen_plus(decimal: &mut Vec<u32>, digit: u32) {
    let mut carry = digit;
    for place in decimal.iter_mut() {
        // At most 9 × 16 + 15, so no step overflows.
        let n = place.wrapping_mul(HEX).wrapping_add(carry);
        *place = n.checked_rem(DECIMAL).unwrap_or(0);
        carry = n.checked_div(DECIMAL).unwrap_or(0);
    }
    while carry > 0 {
        decimal.push(carry.checked_rem(DECIMAL).unwrap_or(0));
        carry = carry.checked_div(DECIMAL).unwrap_or(0);
    }
}

impl Lexer {
    /// The character next, if one is.
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    /// The character `n` past the one next, if one is.
    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.at.checked_add(n)?).copied()
    }

    /// Past the character next.
    fn bump(&mut self) {
        self.at = self.at.saturating_add(1);
    }

    /// Whether `c` is next, taken if it is.
    fn accept(&mut self, c: char) -> bool {
        let found = self.peek() == Some(c);
        if found {
            self.bump();
        }
        found
    }

    /// Past the white space and line terminators next.
    fn separators(&mut self) -> Result<()> {
        loop {
            let Some(c) = self.peek() else { return Ok(()) };
            if class_of(c)? != Class::Separator {
                return Ok(());
            }
            self.bump();
        }
    }

    /// The token that begins with `c`, the character next, taken.
    fn token(&mut self, c: char) -> Result<Token> {
        match c {
            '"' | '\'' => self.string(c).map(Token::String),
            '0'..='9' => self.number(),
            '.' => {
                if self.peek_at(1).is_some_and(|d| d.is_ascii_digit()) {
                    return self.number();
                }
                self.bump();
                Ok(Token::Symbol(Symbol::Period))
            }
            _ => self.symbol().map_or_else(|| self.identifier(), |symbol| Ok(Token::Symbol(symbol))),
        }
    }

    /// The special symbol next, taken; none if none is next.
    fn symbol(&mut self) -> Option<Symbol> {
        let (written, symbol) = SYMBOLS.iter().find(|(written, _)| self.is_next(written))?;
        self.at = self.at.saturating_add(written.chars().count());
        Some(*symbol)
    }

    /// Whether the characters `written` are next.
    fn is_next(&self, written: &str) -> bool {
        written.chars().enumerate().all(|(n, c)| self.peek_at(n) == Some(c))
    }

    /// [ECMAScript 5.1 §7.6] An IdentifierName, its escapes replaced.
    fn identifier(&mut self) -> Result<Token> {
        let mut name = String::new();
        loop {
            let at = self.at;
            let Some(c) = self.peek() else { break };
            let escaped = c == '\\';
            let character = if escaped { self.identifier_escape()? } else { c };
            let class = class_of(character)?;
            if class == Class::Start || (class == Class::Part && !name.is_empty()) {
                if !escaped {
                    self.bump();
                }
                name.push(character);
                continue;
            }
            if escaped {
                return Err(lexical(at, "an escape in an identifier stands for a character the identifier cannot have there"));
            }
            if name.is_empty() {
                return Err(lexical(at, &format!("no token begins with {c:?}")));
            }
            break;
        }
        Ok(Token::Identifier(name))
    }

    /// [ECMAScript 5.1 §7.6] The character of the `\ UnicodeEscapeSequence`
    /// next in an identifier, taken. A surrogate is no character, so none
    /// of an identifier.
    fn identifier_escape(&mut self) -> Result<char> {
        let at = self.at;
        self.bump();
        if !self.accept('u') {
            return Err(lexical(at, "a '\\' in an identifier begins no \\u escape"));
        }
        let unit = self.hex(UNICODE_ESCAPE_DIGITS, at)?;
        char::from_u32(unit).ok_or_else(|| lexical(at, "an escape in an identifier stands for a surrogate"))
    }

    /// The value of `count` hexadecimal digits next, taken, of the escape
    /// at character `at`.
    fn hex(&mut self, count: u32, at: usize) -> Result<u32> {
        let mut value: u32 = 0;
        for _ in 0..count {
            let Some(digit) = self.peek().and_then(|c| c.to_digit(HEX)) else {
                return Err(lexical(at, &format!("{count} hexadecimal digits follow the escape's letter")));
            };
            self.bump();
            value = value.wrapping_mul(HEX).wrapping_add(digit);
        }
        Ok(value)
    }

    /// [ECMAScript 5.1 §7.8.3] A NumericLiteral, the first character of
    /// which is next: its value [§9.45 GR1a] as the `<signed numeric
    /// literal>` of its characters, exact without an ExponentPart and
    /// approximate with one; of a HexIntegerLiteral, its MV, exact.
    fn number(&mut self) -> Result<Token> {
        let start = self.at;
        if self.is_hex() {
            let item = self.hex_integer()?;
            return self.followed(Token::Number(item, Numeral::Other));
        }
        let whole = if self.accept('0') { String::from('0') } else { self.digits() };
        let point = self.accept('.');
        let fraction = if point { self.digits() } else { String::new() };
        let token = if matches!(self.peek(), Some('e' | 'E')) {
            self.exponent()?;
            let text: String = self.chars.get(start..self.at).map_or_else(String::new, |read| read.iter().collect());
            Token::Number(approximate(&text)?, Numeral::Other)
        } else {
            Token::Number(exact(false, &whole, &fraction)?, if point { Numeral::Other } else { Numeral::Integer })
        };
        self.followed(token)
    }

    /// `token`, the numeric literal just read, if what follows may follow
    /// it: "The source character immediately following a NumericLiteral
    /// must not be an IdentifierStart or DecimalDigit."
    fn followed(&mut self, token: Token) -> Result<Token> {
        let after = self.at;
        let Some(c) = self.peek() else { return Ok(token) };
        let class = class_of(c)?;
        if c.is_ascii_digit() || c == '\\' || class == Class::Start {
            return Err(lexical(after, "a numeric literal is followed at once by a digit or an identifier"));
        }
        Ok(token)
    }

    /// Whether a HexIntegerLiteral is next: `0x` or `0X`, and a hexadecimal
    /// digit.
    fn is_hex(&self) -> bool {
        self.peek() == Some('0') && matches!(self.peek_at(1), Some('x' | 'X')) && self.peek_at(2).is_some_and(|c| c.is_ascii_hexdigit())
    }

    /// [ECMAScript 5.1 §7.8.3] The HexIntegerLiteral next, taken: `0x` or
    /// `0X` and its HexDigits. Its MV, "(the MV of HexIntegerLiteral times
    /// 16) plus the MV of HexDigit" digit by digit, is an exact number, as
    /// the `exact` of its decimal digits is: of more than 18 digits a
    /// DECFLOAT, rounded to 34.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if it is out of the range of a decimal floating-point
    /// number.
    fn hex_integer(&mut self) -> Result<Item> {
        self.at = self.at.saturating_add(HEX_PREFIX_LENGTH);
        let mut decimal: Vec<u32> = vec![0];
        while let Some(digit) = self.peek().and_then(|c| c.to_digit(HEX)) {
            self.bump();
            times_sixteen_plus(&mut decimal, digit);
            if decimal.len() > MAX_HEX_VALUE_DIGITS {
                return Err(beyond("a number is out of the range of a decimal floating-point number"));
            }
        }
        let digits: String = decimal.iter().rev().filter_map(|d| char::from_digit(*d, DECIMAL)).collect();
        exact(false, &digits, "")
    }

    /// DecimalDigits: the decimal digits next, taken, none perhaps.
    fn digits(&mut self) -> String {
        let mut digits = String::new();
        while let Some(c) = self.peek().filter(char::is_ascii_digit) {
            digits.push(c);
            self.bump();
        }
        digits
    }

    /// ExponentPart, its ExponentIndicator next: past it.
    fn exponent(&mut self) -> Result<()> {
        let at = self.at;
        self.bump();
        if !self.accept('+') {
            let _minus = self.accept('-');
        }
        if self.digits().is_empty() {
            return Err(lexical(at, "a numeric literal's exponent has no digits"));
        }
        Ok(())
    }

    /// [ECMAScript 5.1 §7.8.4] A StringLiteral, its quotation mark `quote`
    /// next: its characters, each escape replaced [§9.45 GR1b].
    fn string(&mut self, quote: char) -> Result<String> {
        let start = self.at;
        self.bump();
        let mut string = String::new();
        loop {
            let at = self.at;
            let Some(c) = self.peek() else { return Err(lexical(start, "a string literal is not closed")) };
            self.bump();
            if c == quote {
                return Ok(string);
            }
            if is_line_terminator(c) {
                return Err(lexical(at, "a line terminator is in a string literal, not escaped"));
            }
            if c == '\\' {
                self.escape(start, &mut string)?;
            } else {
                string.push(c);
            }
        }
    }

    /// [ECMAScript 5.1 §7.8.4] Past the EscapeSequence or LineContinuation
    /// whose `\` is read, in the string literal that begins at character
    /// `start`: what it stands for pushed to `string`.
    fn escape(&mut self, start: usize, string: &mut String) -> Result<()> {
        let at = self.at;
        let Some(c) = self.peek() else { return Err(lexical(start, "a string literal is not closed")) };
        self.bump();
        let character = match c {
            'b' => '\u{8}',
            't' => '\t',
            'n' => '\n',
            'v' => VT,
            'f' => FF,
            'r' => '\r',
            '0' => self.nul(at)?,
            '1'..='9' => return Err(lexical(at, "an escape of a digit is an octal escape, which ECMAScript 5.1 does not have")),
            'x' => self.hex_escape(at)?,
            'u' => self.unicode_escape(at)?,
            '\r' => {
                // [§7.3] A LineContinuation: <CR> <LF> is one LineTerminatorSequence.
                let _lf = self.accept('\n');
                return Ok(());
            }
            '\n' | LS | PS => return Ok(()),
            _ => c,
        };
        string.push(character);
        Ok(())
    }

    /// The <NUL> of the escape `\0`, at character `at`: one no decimal
    /// digit follows.
    fn nul(&self, at: usize) -> Result<char> {
        if self.peek().is_some_and(|c| c.is_ascii_digit()) {
            return Err(lexical(at, "an escape of a digit is an octal escape, which ECMAScript 5.1 does not have"));
        }
        Ok('\0')
    }

    /// The character of a HexEscapeSequence, at character `at`, whose `x`
    /// is read.
    fn hex_escape(&mut self, at: usize) -> Result<char> {
        let unit = self.hex(HEX_ESCAPE_DIGITS, at)?;
        // Two hexadecimal digits are at most 0xFF, always a character.
        Ok(char::from_u32(unit).unwrap_or(char::REPLACEMENT_CHARACTER))
    }

    /// The character a UnicodeEscapeSequence at character `at`, whose `u`
    /// is read, stands for: of its code unit, or of it and the next
    /// escape's, a UTF-16 surrogate pair. \[DESIGN\] A surrogate not of a
    /// pair is no Unicode character, so none of an SQL character string
    /// [§9.45 GR1b], and is refused.
    fn unicode_escape(&mut self, at: usize) -> Result<char> {
        let first = self.hex(UNICODE_ESCAPE_DIGITS, at)?;
        if !HIGH_SURROGATES.contains(&first) {
            return char::from_u32(first).ok_or_else(|| lexical(at, "an escape of a low surrogate follows no high surrogate"));
        }
        let unpaired = || lexical(at, "no escape of a low surrogate follows an escape of a high surrogate");
        if !self.accept('\\') {
            return Err(unpaired());
        }
        if !self.accept('u') {
            return Err(unpaired());
        }
        let second = self.hex(UNICODE_ESCAPE_DIGITS, at)?;
        paired(first, second).ok_or_else(unpaired)
    }
}

#[cfg(test)]
mod tests;
