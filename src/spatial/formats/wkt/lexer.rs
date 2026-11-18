//! The tokens of the well-known text representation, ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §5.1.67: keywords, numbers as ISO/IEC 9075-2 §5.3
//! writes them, double-quoted element tags, parentheses and commas.
//! Spaces separate tokens and are otherwise ignored.

use crate::error::Result;
use crate::spatial::curves::fault::{Fault, fault};

/// A token.
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    /// A keyword, upper-cased.
    Word(String),
    /// A number, with whether it was written as a signed integer.
    Number(f64, bool),
    /// The letters of a double-quoted tag.
    Quoted(String),
    /// `(`.
    Open,
    /// `)`.
    Close,
    /// `,`.
    Comma,
}

/// The clause the text's rules sit in.
pub const CLAUSE: &str = "§5.1.67";

/// A 2FF22 error for `detail`.
pub fn invalid(detail: &str) -> crate::error::SqlError {
    fault(Fault::InvalidText, CLAUSE, detail)
}

/// The tokens of `text`.
///
/// # Errors
///
/// SQLSTATE 2FF22 for a character no token starts with, a malformed
/// number or an unclosed tag.
pub fn tokens(text: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut at = 0_usize;
    while let Some(&c) = chars.get(at) {
        let next = at.saturating_add(1);
        if c.is_whitespace() {
            at = next;
            continue;
        }
        let single = match c {
            '(' => Some(Token::Open),
            ')' => Some(Token::Close),
            ',' => Some(Token::Comma),
            _ => None,
        };
        if let Some(token) = single {
            out.push(token);
            at = next;
        } else if c.is_ascii_alphabetic() {
            let end = scan(&chars, at, |ch| ch.is_ascii_alphanumeric() || ch == '_');
            out.push(Token::Word(chars.get(at..end).unwrap_or_default().iter().collect::<String>().to_ascii_uppercase()));
            at = end;
        } else if c == '"' {
            let end = scan(&chars, next, |ch| ch != '"');
            if chars.get(end).is_none() {
                return Err(invalid("an element tag's double quote is not closed"));
            }
            out.push(Token::Quoted(chars.get(next..end).unwrap_or_default().iter().collect()));
            at = end.saturating_add(1);
        } else {
            let (token, end) = number(&chars, at)?;
            out.push(token);
            at = end;
        }
    }
    Ok(out)
}

/// The index of the first character from `from` that `keep` rejects.
fn scan(chars: &[char], from: usize, keep: impl Fn(char) -> bool) -> usize {
    let mut at = from;
    while chars.get(at).is_some_and(|c| keep(*c)) {
        at = at.saturating_add(1);
    }
    at
}

/// The number starting at `from`: sign, digits, an optional fraction and
/// an optional exponent.
fn number(chars: &[char], from: usize) -> Result<(Token, usize)> {
    let mut at = from;
    if chars.get(at).is_some_and(|c| *c == '-' || *c == '+') {
        at = at.saturating_add(1);
    }
    let whole = scan(chars, at, |c| c.is_ascii_digit());
    let mut end = whole;
    let mut integer = whole > at;
    if chars.get(end) == Some(&'.') {
        end = scan(chars, end.saturating_add(1), |c| c.is_ascii_digit());
        integer = false;
    }
    let digits = end > at.saturating_add(usize::from(chars.get(whole) == Some(&'.')));
    if chars.get(end).is_some_and(|c| *c == 'e' || *c == 'E') {
        let mut exponent = end.saturating_add(1);
        if chars.get(exponent).is_some_and(|c| *c == '-' || *c == '+') {
            exponent = exponent.saturating_add(1);
        }
        let after = scan(chars, exponent, |c| c.is_ascii_digit());
        if after == exponent {
            return Err(invalid("a number's exponent has no digits"));
        }
        end = after;
        integer = false;
    }
    let text: String = chars.get(from..end).unwrap_or_default().iter().collect();
    let value = text.parse::<f64>().ok().filter(|_| digits);
    value.map(|v| (Token::Number(v, integer), end)).ok_or_else(|| invalid(&format!("unexpected text at character {from}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.67] Keywords,
    /// numbers in every form, tags and punctuation come apart.
    #[test]
    fn splits_text() {
        let got = tokens(" point z( -1.5 +2E3,.5 7 \"a b\")").unwrap();
        let want = vec![
            Token::Word("POINT".into()),
            Token::Word("Z".into()),
            Token::Open,
            Token::Number(-1.5, false),
            Token::Number(2000.0, false),
            Token::Comma,
            Token::Number(0.5, false),
            Token::Number(7.0, true),
            Token::Quoted("a b".into()),
            Token::Close,
        ];
        assert_eq!(got, want);
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.67] Malformed
    /// text raises 2FF22.
    #[test]
    fn rejects_bad_text() {
        for text in ["1e", "-", ".", "\"open", "#"] {
            assert_eq!(tokens(text).map_err(|e| e.sqlstate().as_str().to_owned()), Err("2FF22".to_owned()), "{text}");
        }
    }
}
