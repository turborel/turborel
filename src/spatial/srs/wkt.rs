//! The well-known text representation of a spatial reference system, the
//! \<spatial reference system\> of ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §14.1.9: reading it into a [`CoordinateSystem`] and writing one back
//! in canonical form, the pair the ST_WellKnownText transform group of
//! §14.1.8 is made of.
//!
//! Reading goes in two steps. The text is first read as a tree of items,
//! each a keyword and its delimited, comma-separated arguments, so the
//! delimiter pairing rule of §14.1.9 Description 1a holds for every
//! item. The tree is then checked against the grammar's productions and
//! built into the model (`build`). Writing is in `write`.

mod build;
mod write;

use super::fault::{Fault, Outcome};
use super::model::{CoordinateSystem, Direction};
use super::number;

pub use write::write;

// The key words of §14.1.9 Description 1c, then the \[EXTENSION\] items.

/// PROJCS, a \<projected cs\>.
pub const PROJCS: &str = "PROJCS";
/// GEOGCS, a \<geographic cs\>.
pub const GEOGCS: &str = "GEOGCS";
/// GEOCCS, a \<geocentric cs\>.
pub const GEOCCS: &str = "GEOCCS";
/// DATUM, a \<datum\>.
pub const DATUM: &str = "DATUM";
/// SPHEROID, a \<spheroid\>.
pub const SPHEROID: &str = "SPHEROID";
/// PRIMEM, a \<prime meridian\>.
pub const PRIMEM: &str = "PRIMEM";
/// PROJECTION, a \<projection\>.
pub const PROJECTION: &str = "PROJECTION";
/// PARAMETER, a \<parameter\>.
pub const PARAMETER: &str = "PARAMETER";
/// UNIT, a \<unit\>.
pub const UNIT: &str = "UNIT";
/// AUTHORITY, an organization's code for an item.
pub const AUTHORITY: &str = "AUTHORITY";
/// TOWGS84, the datum's Helmert parameters to WGS 84.
pub const TOWGS84: &str = "TOWGS84";
/// AXIS, a coordinate axis.
pub const AXIS: &str = "AXIS";

/// The axis direction key words of an AXIS item.
pub(crate) const DIRECTIONS: [(&str, Direction); 7] = [
    ("NORTH", Direction::North),
    ("SOUTH", Direction::South),
    ("EAST", Direction::East),
    ("WEST", Direction::West),
    ("UP", Direction::Up),
    ("DOWN", Direction::Down),
    ("OTHER", Direction::Other),
];

/// The deepest nesting of items the grammar produces is PROJCS, GEOGCS,
/// DATUM, SPHEROID, AUTHORITY; a deeper text is refused before any
/// recursion can exhaust the stack (RULES.md R-CF-6).
const MAX_DEPTH: usize = 6;

/// The \<spatial reference system\> `text`, as a coordinate system.
/// Key words are read without regard to case.
///
/// # Errors
///
/// F22 invalid well-known text representation if `text` is not
/// producible in the BNF of §14.1.9 with its extensions, or breaks a rule
/// of §14.1.9 Description 1 (a delimiter pair, a semi-major axis or a
/// conversion factor not above zero). §14.1.2 Description 2 lets an
/// implementation raise it; this one always does.
pub fn read(text: &str) -> Outcome<CoordinateSystem> {
    let mut reader = Reader { tokens: tokens(text)?.into_iter().peekable() };
    let root = reader.node(0)?;
    if reader.tokens.next().is_some() {
        return Err(Fault::text("[ISO/IEC 13249-3:2016 §14.1.9] text follows the <spatial reference system>"));
    }
    build::system(root)
}

/// A <left delimiter> or <right delimiter>.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delimiter {
    /// <left paren> and <right paren>.
    Paren,
    /// <left bracket> and <right bracket>.
    Bracket,
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Word(String),
    Text(String),
    Number(f64),
    Open(Delimiter),
    Close(Delimiter),
    Comma,
}

/// An item: its key word, upper case, and its arguments.
#[derive(Debug)]
struct Node {
    keyword: String,
    args: Vec<Arg>,
}

/// One argument of an item.
#[derive(Debug)]
enum Arg {
    /// A double-quoted name.
    Text(String),
    /// A <number>.
    Number(f64),
    /// A bare word, as an AXIS direction.
    Word(String),
    /// A nested item.
    Node(Node),
}

/// The tokens of `text`. Names run from a <double quote> to the next one;
/// any character but a double quote is accepted in them, as real names
/// hold `/` and `,`, beyond the <letters> of §14.1.9.
fn tokens(text: &str) -> Outcome<Vec<Token>> {
    let mut out = Vec::new();
    let mut rest = text.trim_start();
    while let Some(first) = rest.chars().next() {
        let (token, length) = token(first, rest)?;
        out.push(token);
        rest = rest.get(length..).unwrap_or_default().trim_start();
    }
    Ok(out)
}

/// The token at the start of `rest`, whose first character is `first`,
/// and its length in bytes.
fn token(first: char, rest: &str) -> Outcome<(Token, usize)> {
    let simple = match first {
        '(' => Some(Token::Open(Delimiter::Paren)),
        '[' => Some(Token::Open(Delimiter::Bracket)),
        ')' => Some(Token::Close(Delimiter::Paren)),
        ']' => Some(Token::Close(Delimiter::Bracket)),
        ',' => Some(Token::Comma),
        _ => None,
    };
    if let Some(token) = simple {
        return Ok((token, 1));
    }
    if first == '"' {
        let body = rest.get(1..).unwrap_or_default();
        let end = body.find('"').ok_or_else(|| Fault::text("[ISO/IEC 13249-3:2016 §14.1.9] a name lacks its closing <double quote>"))?;
        return Ok((Token::Text(body.get(..end).unwrap_or_default().to_owned()), end.saturating_add(2)));
    }
    if first.is_ascii_alphabetic() {
        let length = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
        return Ok((Token::Word(rest.get(..length).unwrap_or_default().to_ascii_uppercase()), length));
    }
    let length = number::literal_length(rest).ok_or_else(|| unexpected(rest))?;
    let value = rest.get(..length).and_then(number::value).ok_or_else(|| unexpected(rest))?;
    Ok((Token::Number(value), length))
}

fn unexpected(rest: &str) -> Fault {
    let near: String = rest.chars().take(16).collect();
    Fault::text(format!("[ISO/IEC 13249-3:2016 §14.1.9] unexpected text at '{near}'"))
}

struct Reader {
    tokens: std::iter::Peekable<std::vec::IntoIter<Token>>,
}

impl Reader {
    /// An item: a key word, a <left delimiter>, arguments separated by
    /// <comma>s and the matching <right delimiter>.
    fn node(&mut self, depth: usize) -> Outcome<Node> {
        let Some(Token::Word(keyword)) = self.tokens.next() else {
            return Err(Fault::text("[ISO/IEC 13249-3:2016 §14.1.9] a key word was expected"));
        };
        self.items(keyword, depth)
    }

    fn items(&mut self, keyword: String, depth: usize) -> Outcome<Node> {
        if depth >= MAX_DEPTH {
            return Err(Fault::text("[ISO/IEC 13249-3:2016 §14.1.9] items are nested too deeply"));
        }
        let Some(Token::Open(open)) = self.tokens.next() else {
            return Err(Fault::text(format!("[ISO/IEC 13249-3:2016 §14.1.9] {keyword} lacks its <left delimiter>")));
        };
        let mut args = Vec::new();
        loop {
            args.push(self.arg(depth)?);
            match self.tokens.next() {
                Some(Token::Comma) => {}
                Some(Token::Close(close)) => {
                    if close != open {
                        return Err(Fault::text(format!("[ISO/IEC 13249-3:2016 §14.1.9 Description 1a] {keyword}'s delimiters differ")));
                    }
                    return Ok(Node { keyword, args });
                }
                Some(Token::Word(_) | Token::Text(_) | Token::Number(_) | Token::Open(_)) | None => {
                    return Err(Fault::text(format!("[ISO/IEC 13249-3:2016 §14.1.9] {keyword} lacks a <comma> or <right delimiter>")));
                }
            }
        }
    }

    fn arg(&mut self, depth: usize) -> Outcome<Arg> {
        match self.tokens.next() {
            Some(Token::Text(text)) => Ok(Arg::Text(text)),
            Some(Token::Number(value)) => Ok(Arg::Number(value)),
            Some(Token::Word(word)) => {
                if matches!(self.tokens.peek(), Some(Token::Open(_))) {
                    Ok(Arg::Node(self.items(word, depth.saturating_add(1))?))
                } else {
                    Ok(Arg::Word(word))
                }
            }
            Some(Token::Open(_) | Token::Close(_) | Token::Comma) | None => {
                Err(Fault::text("[ISO/IEC 13249-3:2016 §14.1.9] a name, number or item was expected"))
            }
        }
    }
}
