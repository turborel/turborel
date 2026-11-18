//! The lexical items of a search pattern [ISO/IEC 13249-2:2002 (CD, SC32
//! N0640) §5.3.1 Description 3]: key words and language names, quoted
//! words, phrases and names, unsigned integers and the symbols `(`, `)`,
//! `,`, `|` and `&`. White space separates items and is otherwise not
//! significant [§4.2.1 NOTE 1].

use crate::error::{Code, Result, SqlError, SqlState};

/// The `<double quote>` that opens and closes a word, a phrase, an escape
/// character or a thesaurus name.
pub(super) const DOUBLE_QUOTE: char = '"';

// The key words of a pattern [§5.3.2].
/// `IS ABOUT`.
pub(super) const ABOUT: &str = "ABOUT";
/// Joins the arguments of a context condition.
pub(super) const AND: &str = "AND";
/// `ANY ORDER`.
pub(super) const ANY: &str = "ANY";
/// `IN SAME ... AS`.
pub(super) const AS: &str = "AS";
/// `EXPAND BROADER TERM`.
pub(super) const BROADER: &str = "BROADER";
/// A unit.
pub(super) const CHARACTERS: &str = "CHARACTERS";
/// An `<escape specification>`.
pub(super) const ESCAPE: &str = "ESCAPE";
/// Listed, and in no rule.
const FROM: &str = "FROM";
/// Listed, and in no rule.
const PROXIMITY: &str = "PROXIMITY";
/// A thesaurus expansion.
pub(super) const EXPAND: &str = "EXPAND";
/// `FOR n LEVELS`.
pub(super) const FOR: &str = "FOR";
/// `FORM OF`.
pub(super) const FORM: &str = "FORM";
/// `FUZZY FORM OF`.
pub(super) const FUZZY: &str = "FUZZY";
/// `IN ORDER`, `IN SAME`.
pub(super) const IN: &str = "IN";
/// `IS ABOUT`.
pub(super) const IS: &str = "IS";
/// `FOR 1 LEVEL`.
pub(super) const LEVEL: &str = "LEVEL";
/// `FOR n LEVELS`.
pub(super) const LEVELS: &str = "LEVELS";
/// `SOUNDS LIKE`.
pub(super) const LIKE: &str = "LIKE";
/// `EXPAND NARROWER TERM`.
pub(super) const NARROWER: &str = "NARROWER";
/// A proximity.
pub(super) const NEAR: &str = "NEAR";
/// Negation.
pub(super) const NOT: &str = "NOT";
/// `FORM OF`, `TERM OF`.
pub(super) const OF: &str = "OF";
/// `IN ORDER`, `ANY ORDER`.
pub(super) const ORDER: &str = "ORDER";
/// A unit.
pub(super) const PARAGRAPH: &str = "PARAGRAPH";
/// A unit.
pub(super) const PARAGRAPHS: &str = "PARAGRAPHS";
/// `EXPAND PREFERRED TERM`.
pub(super) const PREFERRED: &str = "PREFERRED";
/// `EXPAND RELATED TERM`.
pub(super) const RELATED: &str = "RELATED";
/// `IN SAME`.
pub(super) const SAME: &str = "SAME";
/// A unit.
pub(super) const SENTENCE: &str = "SENTENCE";
/// A unit.
pub(super) const SENTENCES: &str = "SENTENCES";
/// `SOUNDS LIKE`.
pub(super) const SOUNDS: &str = "SOUNDS";
/// `STEMMED FORM OF`.
pub(super) const STEMMED: &str = "STEMMED";
/// `EXPAND SYNONYM TERM`.
pub(super) const SYNONYM: &str = "SYNONYM";
/// `TERM OF`.
pub(super) const TERM: &str = "TERM";
/// A thesaurus expansion.
pub(super) const THESAURUS: &str = "THESAURUS";
/// `EXPAND TOP TERM`.
pub(super) const TOP: &str = "TOP";
/// A proximity's distance.
pub(super) const WITHIN: &str = "WITHIN";
/// A unit.
pub(super) const WORDS: &str = "WORDS";

/// Every key word [§5.3.2], none of which names a language. FROM and
/// PROXIMITY are listed though no rule uses them.
pub(super) const KEY_WORDS: [&str; 38] = [
    ABOUT, AND, ANY, AS, BROADER, CHARACTERS, ESCAPE, EXPAND, FOR, FORM, FROM, FUZZY, IN, IS, LEVEL, LEVELS, LIKE, NARROWER, NEAR, NOT, OF,
    ORDER, PARAGRAPH, PARAGRAPHS, PREFERRED, PROXIMITY, RELATED, SAME, SENTENCE, SENTENCES, SOUNDS, STEMMED, SYNONYM, TERM, THESAURUS, TOP,
    WITHIN, WORDS,
];

/// A lexical item of a pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Lexeme {
    /// A key word or a language name, as written.
    Name(String),
    /// What stands between two `<double quote>`s, each `<doublequote
    /// symbol>` (`""`) in it taken for one `"` [§5.3.1 Description 3a].
    Quoted(String),
    /// An `<unsigned integer>`.
    Number(u64),
    /// `(`.
    LeftParen,
    /// `)`.
    RightParen,
    /// `,`.
    Comma,
    /// `|`.
    Bar,
    /// `&`.
    Ampersand,
}

/// A lexical item and the position of its first character, from 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Item {
    /// The item.
    pub lexeme: Lexeme,
    /// Its first character's position in the pattern, from 1.
    pub at: usize,
}

/// The error of a pattern not of the form of a `<search expression>`,
/// `what` naming the rule broken, at character `at`.
pub(super) fn invalid(at: usize, what: &str) -> SqlError {
    SqlError::new(Code::Data, format!("{what}, at character {at} of the search pattern")).with_state(SqlState::INVALID_SEARCH_EXPRESSION)
}

/// The items of `text`.
///
/// # Errors
///
/// SQLSTATE 2FF01 for a character that begins no item, an unclosed
/// quote, or an integer too large.
pub(super) fn lex(text: &str) -> Result<Vec<Item>> {
    let mut chars = text.chars().zip(1_usize..).peekable();
    let mut items = Vec::new();
    while let Some((c, at)) = chars.next() {
        let lexeme = if let Some(symbol) = symbol(c) {
            symbol
        } else if c == DOUBLE_QUOTE {
            Lexeme::Quoted(quoted(&mut chars, at)?)
        } else if c.is_whitespace() {
            continue;
        } else if c.is_ascii_digit() {
            Lexeme::Number(number(c, &mut chars, at)?)
        } else if c.is_alphabetic() {
            Lexeme::Name(name(c, &mut chars))
        } else {
            return Err(invalid(at, &format!("[ISO/IEC 13249-2:2002 §5.3.1 Description 3] {c} begins no part of a search expression")));
        };
        items.push(Item { lexeme, at });
    }
    Ok(items)
}

/// The symbol `c` is, if it is one.
fn symbol(c: char) -> Option<Lexeme> {
    match c {
        '(' => Some(Lexeme::LeftParen),
        ')' => Some(Lexeme::RightParen),
        ',' => Some(Lexeme::Comma),
        '|' => Some(Lexeme::Bar),
        '&' => Some(Lexeme::Ampersand),
        _ => None,
    }
}

/// The characters of the pattern still to read, each with its position.
type Chars<'t> = std::iter::Peekable<std::iter::Zip<std::str::Chars<'t>, std::ops::RangeFrom<usize>>>;

/// What stands up to the closing quote of a quote opened at `at`, `""`
/// read as one `"`.
fn quoted(chars: &mut Chars<'_>, at: usize) -> Result<String> {
    let mut out = String::new();
    while let Some((c, _)) = chars.next() {
        if c != DOUBLE_QUOTE {
            out.push(c);
        } else if chars.next_if(|(next, _)| *next == DOUBLE_QUOTE).is_some() {
            out.push(DOUBLE_QUOTE);
        } else {
            return Ok(out);
        }
    }
    Err(invalid(at, "[ISO/IEC 13249-2:2002 §5.3.1 Description 3] a double quote is never closed"))
}

/// The unsigned integer whose first digit is `first`.
fn number(first: char, chars: &mut Chars<'_>, at: usize) -> Result<u64> {
    let mut digits = String::from(first);
    while let Some((c, _)) = chars.next_if(|(c, _)| c.is_ascii_digit()) {
        digits.push(c);
    }
    digits.parse().map_err(|_| invalid(at, &format!("[ISO/IEC 13249-2:2002 §5.3.1 Description 3] {digits} is too large a count")))
}

/// The key word or name whose first letter is `first`: letters, digits
/// and `_`.
fn name(first: char, chars: &mut Chars<'_>) -> String {
    let mut out = String::from(first);
    while let Some((c, _)) = chars.next_if(|(c, _)| c.is_alphanumeric() || *c == '_') {
        out.push(c);
    }
    out
}
