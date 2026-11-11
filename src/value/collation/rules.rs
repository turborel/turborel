//! \[EXTENSION\] The rules of a tailoring, as CLDR writes them \[UTS #35
//! Part 5 §3\]: `&D<<đ<<<Đ` resets to `D`, then puts `đ` after it at the
//! second level and `Đ` after `đ` at the third.
//!
//! What the tailorings of `cldr38.rules` use is read: resets, `[before n]`,
//! the relations `<`, `<<`, `<<<` and `=`, an extension after `/`, quotes,
//! `\u` escapes, and the options `[caseFirst upper]` and `[backwards 2]`.
//! Anything else is refused, so a rule is never half read.

use crate::error::{Code, Result, SqlError};

/// The most levels a relation names.
pub(super) const LEVELS: usize = 3;
/// The digits of a `\u` and a `\U` escape.
const SHORT_ESCAPE: usize = 4;
const LONG_ESCAPE: usize = 8;
const HEX: u32 = 16;

/// One relation: what it puts after the last, at which level, `0` for
/// equal, and the extension its elements are followed by.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Relation {
    pub(super) level: usize,
    pub(super) text: String,
    pub(super) extension: String,
}

/// A reset and the relations after it.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Rule {
    /// The level a `[before n]` names: the first relation goes before the
    /// reset, not after it.
    pub(super) before: Option<usize>,
    pub(super) reset: String,
    pub(super) relations: Vec<Relation>,
}

/// A tailoring's rules and options.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Rules {
    /// `[caseFirst upper]`: at the third level, upper case first.
    pub(super) upper_first: bool,
    /// `[backwards 2]`: second-level weights compared from the end.
    pub(super) backwards: bool,
    pub(super) list: Vec<Rule>,
}

/// A piece of the rules' text.
#[derive(Debug, PartialEq, Eq)]
enum Token {
    Reset,
    Option(String),
    Relation(usize),
    Extension,
    Text(String),
}

pub(super) fn damaged(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a tailoring's rules are damaged: {why}"))
}

/// `text`'s rules.
///
/// # Errors
///
/// SQLCODE -916 if it holds what this does not read.
pub(super) fn parse(text: &str) -> Result<Rules> {
    let mut out = Rules::default();
    let mut tokens = tokens(text)?.into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            Token::Option(o) if o == "caseFirst upper" => out.upper_first = true,
            Token::Option(o) if o == "backwards 2" => out.backwards = true,
            Token::Reset => {
                let before = match tokens.next_if(|t| matches!(t, Token::Option(_))) {
                    Some(Token::Option(o)) => Some(before_level(&o)?),
                    _ => None,
                };
                let Some(Token::Text(reset)) = tokens.next() else { return Err(damaged("a reset names nothing")) };
                let mut relations = Vec::new();
                while let Some(Token::Relation(level)) = tokens.next_if(|t| matches!(t, Token::Relation(_))) {
                    let Some(Token::Text(named)) = tokens.next() else { return Err(damaged("a relation names nothing")) };
                    let extension = match tokens.next_if_eq(&Token::Extension) {
                        Some(_) => match tokens.next() {
                            Some(Token::Text(e)) => e,
                            _ => return Err(damaged("an extension names nothing")),
                        },
                        None => String::new(),
                    };
                    relations.push(Relation { level, text: named, extension });
                }
                out.list.push(Rule { before, reset, relations });
            }
            Token::Option(_) | Token::Relation(_) | Token::Extension | Token::Text(_) => {
                return Err(damaged(&format!("{token:?} where a reset or option belongs")));
            }
        }
    }
    Ok(out)
}

/// The level of `[before n]`.
fn before_level(option: &str) -> Result<usize> {
    let level = option.strip_prefix("before ").and_then(|n| n.parse::<usize>().ok());
    level.filter(|n| (1..=LEVELS).contains(n)).ok_or_else(|| damaged(&format!("[{option}] after a reset")))
}

/// Whether `c` ends a run of text.
fn syntax(c: char) -> bool {
    matches!(c, '&' | '[' | '<' | '=' | '/' | '|' | '#') || c.is_whitespace()
}

/// `text`'s tokens; text broken by white space is one.
fn tokens(text: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(&c) = chars.get(at) {
        at = at.saturating_add(1);
        let token = match c {
            space if space.is_whitespace() => continue,
            '#' => {
                at = chars.iter().skip(at).position(|n| *n == '\n').map_or(chars.len(), |n| at.saturating_add(n));
                continue;
            }
            '&' => Token::Reset,
            '/' => Token::Extension,
            '=' => Token::Relation(0),
            '[' => {
                let close = chars.iter().skip(at).position(|n| *n == ']').ok_or_else(|| damaged("an option is not closed"))?;
                let option: String = chars.iter().skip(at).take(close).collect();
                at = at.saturating_add(close).saturating_add(1);
                Token::Option(option.trim().to_owned())
            }
            '<' => {
                let more = chars.iter().skip(at).take_while(|n| **n == '<').count();
                at = at.saturating_add(more);
                let level = more.saturating_add(1);
                if level > LEVELS || chars.get(at) == Some(&'*') {
                    return Err(damaged("a relation of a level or form not read"));
                }
                Token::Relation(level)
            }
            '|' => return Err(damaged("a prefix")),
            _ => {
                let mut run = String::new();
                at = literal(&chars, at.saturating_sub(1), &mut run)?;
                match out.last_mut() {
                    Some(Token::Text(before)) => {
                        before.push_str(&run);
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

/// The text from `at` to the next syntax character, its quotes and escapes
/// read, added to `out`; where it ends.
fn literal(chars: &[char], mut at: usize, out: &mut String) -> Result<usize> {
    let mut quoted = false;
    while let Some(&c) = chars.get(at) {
        at = at.saturating_add(1);
        match c {
            '\'' if chars.get(at) == Some(&'\'') => {
                out.push('\'');
                at = at.saturating_add(1);
            }
            '\'' => quoted = !quoted,
            '\\' => {
                let (escaped, next) = escape(chars, at)?;
                out.push(escaped);
                at = next;
            }
            other if quoted || !syntax(other) => out.push(other),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn relation(level: usize, text: &str, extension: &str) -> Relation {
        Relation { level, text: text.to_owned(), extension: extension.to_owned() }
    }

    /// Verifies: [EXTENSION] RULES.md D60: a tailoring's rules are read as
    /// CLDR writes them, levels, `[before n]`, extensions, quotes, escapes
    /// and options among them.
    #[test]
    fn rules_are_read_as_cldr_writes_them() {
        let read = parse("[caseFirst upper]\n&[before 1]ǀ<å<<<Å # note\n&t<<<þ/h\n&C<c''h=c\\u2019h\n&D\\u0335 << đ").unwrap();
        assert!(read.upper_first && !read.backwards);
        assert_eq!(
            read.list[0],
            Rule { before: Some(1), reset: "ǀ".to_owned(), relations: vec![relation(1, "å", ""), relation(3, "Å", "")] }
        );
        assert_eq!(read.list[1].relations, vec![relation(3, "þ", "h")]);
        assert_eq!(read.list[2].relations, vec![relation(1, "c'h", ""), relation(0, "c\u{2019}h", "")]);
        assert_eq!(read.list[3].reset, "D\u{335}");
        assert_eq!(read.list[3].relations, vec![relation(2, "đ", "")]);
        assert_eq!(parse("&'<'<b").unwrap().list[0].reset, "<");
    }

    /// Verifies: [EXTENSION] RULES.md D60: what is not read is refused,
    /// not half read.
    #[test]
    fn rules_not_read_are_refused() {
        for text in ["&a<<<<b", "&a<*bc", "&a|b<c", "[reorder Grek]", "<a", "&[before 4]a<b", "&a<'b", "&a<\\uZZZZ", "&a</"] {
            assert_eq!(parse(text).map_err(|e| e.code), Err(Code::Corrupt), "{text}");
        }
    }
}
