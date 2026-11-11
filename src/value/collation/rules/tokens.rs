//! The tokens of a tailoring's rules: syntax characters, options in
//! brackets, which may hold a set in brackets of their own, and text, its
//! quotes and escapes read.

use super::{QUATERNARY, damaged};
use crate::error::Result;

/// The digits of a `\u` and a `\U` escape.
const SHORT_ESCAPE: usize = 4;
const LONG_ESCAPE: usize = 8;
const HEX: u32 = 16;
/// What follows a relation to make it a list.
const STAR: char = '*';

/// A character of text, and whether it was written plainly, neither
/// quoted nor escaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Piece {
    pub(super) c: char,
    pub(super) plain: bool,
}

/// A piece of the rules' text.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Token {
    Reset,
    Option(String),
    Relation { level: usize, star: bool },
    Extension,
    Prefix,
    Text(Vec<Piece>),
}

/// Whether `c` ends a run of text.
fn syntax(c: char) -> bool {
    matches!(c, '&' | '[' | ']' | '<' | '=' | '/' | '|' | '#') || blank(c)
}

/// Whether `c` is white space between tokens: Unicode's
/// `Pattern_White_Space`, the direction marks among it \[UAX #31 §4\].
fn blank(c: char) -> bool {
    matches!(c, '\t'..='\r' | ' ' | '\u{85}' | '\u{200E}' | '\u{200F}' | '\u{2028}' | '\u{2029}')
}

/// `text`'s tokens; text broken by white space is one.
pub(super) fn tokens(text: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(&c) = chars.get(at) {
        at = at.saturating_add(1);
        let token = match c {
            space if blank(space) => continue,
            '#' => {
                at = chars.iter().skip(at).position(|n| *n == '\n').map_or(chars.len(), |n| at.saturating_add(n));
                continue;
            }
            '&' => Token::Reset,
            ']' => return Err(damaged("a bracket closes none")),
            '/' => Token::Extension,
            '|' => Token::Prefix,
            '[' => {
                let close = closing(&chars, at)?;
                let option: String = chars.iter().skip(at).take(close.saturating_sub(at)).collect();
                at = close.saturating_add(1);
                Token::Option(option.trim().to_owned())
            }
            '<' | '=' => {
                let more = if c == '<' { chars.iter().skip(at).take_while(|n| **n == '<').count() } else { 0 };
                at = at.saturating_add(more);
                let star = chars.get(at) == Some(&STAR);
                at = at.saturating_add(usize::from(star));
                let level = if c == '<' { more.saturating_add(1) } else { 0 };
                if level > QUATERNARY {
                    return Err(damaged("a relation past the fourth level"));
                }
                Token::Relation { level, star }
            }
            _ => {
                let mut run = Vec::new();
                at = literal(&chars, at.saturating_sub(1), &mut run)?;
                match out.last_mut() {
                    Some(Token::Text(before)) => {
                        before.extend(run);
                        continue;
                    }
                    _ => Token::Text(run),
                }
            }
        };
        out.push(token);
    }
    Ok(out)
}

/// Where the bracket that closes the one before `at` is, past any set in
/// brackets inside it. No option CLDR writes quotes a bracket.
fn closing(chars: &[char], mut at: usize) -> Result<usize> {
    let mut depth = 0_usize;
    while let Some(&c) = chars.get(at) {
        match c {
            '[' => depth = depth.saturating_add(1),
            ']' if depth == 0 => return Ok(at),
            ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        at = at.saturating_add(1);
    }
    Err(damaged("an option is not closed"))
}

/// The pieces of a set, `[…]`: its characters and plain `-`, white space
/// left out.
pub(super) fn set(text: &str) -> Result<Vec<Piece>> {
    let inside = text.trim().strip_prefix('[').and_then(|t| t.strip_suffix(']')).ok_or_else(|| damaged("a set not in brackets"))?;
    let chars: Vec<char> = inside.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(&c) = chars.get(at) {
        at = at.saturating_add(1);
        match c {
            '\\' => {
                let (escaped, next) = escape(&chars, at)?;
                out.push(Piece { c: escaped, plain: false });
                at = next;
            }
            '[' | ']' | '\'' => return Err(damaged("a set of a form not read")),
            space if blank(space) => {}
            other => out.push(Piece { c: other, plain: true }),
        }
    }
    Ok(out)
}

/// The text from `at` to the next syntax character, its quotes and escapes
/// read, added to `out`; where it ends.
fn literal(chars: &[char], mut at: usize, out: &mut Vec<Piece>) -> Result<usize> {
    let mut quoted = false;
    while let Some(&c) = chars.get(at) {
        at = at.saturating_add(1);
        match c {
            '\'' if chars.get(at) == Some(&'\'') => {
                out.push(Piece { c: '\'', plain: false });
                at = at.saturating_add(1);
            }
            '\'' => quoted = !quoted,
            '\\' => {
                let (escaped, next) = escape(chars, at)?;
                out.push(Piece { c: escaped, plain: false });
                at = next;
            }
            other if quoted || !syntax(other) => out.push(Piece { c: other, plain: !quoted }),
            _ => return Ok(at.saturating_sub(1)),
        }
    }
    if quoted { Err(damaged("a quote is not closed")) } else { Ok(at) }
}

/// The character an escape after `at` stands for, and where it ends.
fn escape(chars: &[char], at: usize) -> Result<(char, usize)> {
    let digits = match chars.get(at) {
        Some('u') => SHORT_ESCAPE,
        Some('U') => LONG_ESCAPE,
        Some(&other) => return Ok((other, at.saturating_add(1))),
        None => return Err(damaged("an escape ends the rules")),
    };
    let from = at.saturating_add(1);
    let hex: String = chars.iter().skip(from).take(digits).collect();
    let c = u32::from_str_radix(&hex, HEX).ok().and_then(char::from_u32).ok_or_else(|| damaged(&format!("the escape \\u{hex}")))?;
    Ok((c, from.saturating_add(digits)))
}
