//! \[EXTENSION\] The rules of a tailoring, as CLDR writes them \[UTS #35
//! Part 5 §3\]: `&D<<đ<<<Đ` resets to `D`, then puts `đ` after it at the
//! second level and `Đ` after `đ` at the third.
//!
//! Everything the collations of `cldr38.rules` use is read. That is resets
//! to text or to the positions `[last regular]` and `[last … ignorable]`,
//! `[before n]`, the relations `<` to `<<<<` and `=`, their lists `<*` and
//! `=*` with ranges, a prefix before `|`, an extension after `/`, quotes,
//! `\u` escapes, and the options `[caseFirst]`, `[backwards 2]`,
//! `[strength n]`, `[alternate shifted]`, `[reorder]`,
//! `[suppressContractions]` and `[optimize]`, the last of which changes no
//! order. Anything else is refused, so a rule is never half read.

mod tokens;

use crate::error::{Code, Result, SqlError};
use tokens::{Piece, Token, tokens};

/// The levels of an element's weights.
pub(super) const LEVELS: usize = 3;
/// The level of `<<<<`, and the most levels a key holds.
pub(super) const QUATERNARY: usize = 4;
/// What a list's range is written with \[UTS #35 Part 5 §3.6\].
const RANGE: char = '-';
/// The options read \[UTS #35 Part 5 §3.4, §3.12, §3.13\], by name and
/// value; those whose value is read alone take any.
const CASE_FIRST: &str = "caseFirst";
const UPPER: &str = "upper";
const OFF: &str = "off";
const BACKWARDS: &str = "backwards";
const SECOND: &str = "2";
const STRENGTH: &str = "strength";
const ALTERNATE: &str = "alternate";
const SHIFTED: &str = "shifted";
const NORMALIZATION: &str = "normalization";
const ON: &str = "on";
const OPTIMIZE: &str = "optimize";
const REORDER: &str = "reorder";
const SUPPRESS: &str = "suppressContractions";
/// The option after a reset naming the level a relation goes before.
const BEFORE: &str = "before ";
/// The positions a reset may name \[UTS #35 Part 5 §3.11\].
const LAST_REGULAR: &str = "last regular";
const LAST_PRIMARY_IGNORABLE: &str = "last primary ignorable";
const LAST_SECONDARY_IGNORABLE: &str = "last secondary ignorable";
const LAST_TERTIARY_IGNORABLE: &str = "last tertiary ignorable";

/// A position `[last …]` a reset may name instead of text \[UTS #35 Part
/// 5 §3.11\].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Last {
    /// The last primary before the unified ideographs.
    Regular,
    /// The greatest element whose first level is ignorable.
    PrimaryIgnorable,
    /// The greatest element whose first two levels are ignorable.
    SecondaryIgnorable,
    /// The element ignorable at every level.
    TertiaryIgnorable,
}

/// What a reset names.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Reset {
    Text(String),
    Last(Last),
}

/// One relation: what it puts after the last, at which level, `0` for
/// equal; the text it is weighed after, if any; and the extension its
/// elements are followed by.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Relation {
    pub(super) level: usize,
    pub(super) prefix: String,
    pub(super) text: String,
    pub(super) extension: String,
}

/// A reset and the relations after it.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Rule {
    /// The level a `[before n]` names: the first relation goes before the
    /// reset, not after it.
    pub(super) before: Option<usize>,
    pub(super) reset: Reset,
    pub(super) relations: Vec<Relation>,
}

/// A tailoring's rules and options.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Rules {
    /// `[caseFirst upper]`: at the third level, upper case first.
    pub(super) upper_first: bool,
    /// `[backwards 2]`: second-level weights compared from the end.
    pub(super) backwards: bool,
    /// `[strength n]`: the levels a key holds.
    pub(super) strength: usize,
    /// `[reorder]`: the script groups put first, in order.
    pub(super) reorder: Vec<String>,
    /// `[suppressContractions]`: the characters UNICODE's contractions
    /// beginning with which are not made.
    pub(super) suppressed: Vec<char>,
    pub(super) list: Vec<Rule>,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules { upper_first: false, backwards: false, strength: QUATERNARY, reorder: Vec::new(), suppressed: Vec::new(), list: Vec::new() }
    }
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
            Token::Option(o) => option(&o, &mut out)?,
            Token::Reset => {
                let mut before = None;
                if let Some(Token::Option(o)) = tokens.next_if(|t| matches!(t, Token::Option(o) if o.starts_with(BEFORE))) {
                    before = Some(before_level(&o)?);
                }
                let reset = match tokens.next() {
                    Some(Token::Text(pieces)) => Reset::Text(plain(&pieces)),
                    Some(Token::Option(o)) => Reset::Last(position(&o)?),
                    _ => return Err(damaged("a reset names nothing")),
                };
                let mut relations = Vec::new();
                while let Some(Token::Relation { level, star }) = tokens.next_if(|t| matches!(t, Token::Relation { .. })) {
                    let Some(Token::Text(named)) = tokens.next() else { return Err(damaged("a relation names nothing")) };
                    if star {
                        relations.extend(listed(&named)?.into_iter().map(|c| Relation {
                            level,
                            prefix: String::new(),
                            text: c.into(),
                            extension: String::new(),
                        }));
                    } else {
                        relations.push(relation(level, plain(&named), &mut tokens)?);
                    }
                }
                out.list.push(Rule { before, reset, relations });
            }
            Token::Relation { .. } | Token::Extension | Token::Prefix | Token::Text(_) => {
                return Err(damaged(&format!("{token:?} where a reset or option belongs")));
            }
        }
    }
    Ok(out)
}

/// A relation of `level` naming `named`, and the text after it of a
/// prefix and an extension.
fn relation(level: usize, named: String, tokens: &mut std::iter::Peekable<std::vec::IntoIter<Token>>) -> Result<Relation> {
    let mut after = |marker: Token| match tokens.next_if_eq(&marker) {
        Some(_) => match tokens.next() {
            Some(Token::Text(t)) => Ok(Some(plain(&t))),
            _ => Err(damaged("a prefix or extension names nothing")),
        },
        None => Ok(None),
    };
    let (prefix, text) = match after(Token::Prefix)? {
        Some(text) => (named, text),
        None => (String::new(), named),
    };
    let extension = after(Token::Extension)?.unwrap_or_default();
    Ok(Relation { level, prefix, text, extension })
}

/// An option, read into `out`.
fn option(option: &str, out: &mut Rules) -> Result<()> {
    let (name, value) = option.split_once(' ').unwrap_or((option, ""));
    match (name, value) {
        (CASE_FIRST, UPPER) => out.upper_first = true,
        (CASE_FIRST, OFF) => out.upper_first = false,
        (BACKWARDS, SECOND) => out.backwards = true,
        (STRENGTH, n) => {
            out.strength = n.parse().ok().filter(|n| (1..=QUATERNARY).contains(n)).ok_or_else(|| damaged(&format!("[{option}]")))?;
        }
        (ALTERNATE, SHIFTED) | (NORMALIZATION, ON) | (OPTIMIZE, _) => {}
        (REORDER, codes) => out.reorder = codes.split_whitespace().map(str::to_owned).collect(),
        (SUPPRESS, set) => out.suppressed.extend(listed(&tokens::set(set)?)?),
        _ => return Err(damaged(&format!("the option [{option}]"))),
    }
    Ok(())
}

/// The level of `[before n]`.
fn before_level(option: &str) -> Result<usize> {
    let level = option.strip_prefix(BEFORE).and_then(|n| n.parse::<usize>().ok());
    level.filter(|n| (1..=LEVELS).contains(n)).ok_or_else(|| damaged(&format!("[{option}] after a reset")))
}

/// The position a reset's option names.
fn position(option: &str) -> Result<Last> {
    match option {
        LAST_REGULAR => Ok(Last::Regular),
        LAST_PRIMARY_IGNORABLE => Ok(Last::PrimaryIgnorable),
        LAST_SECONDARY_IGNORABLE => Ok(Last::SecondaryIgnorable),
        LAST_TERTIARY_IGNORABLE => Ok(Last::TertiaryIgnorable),
        _ => Err(damaged(&format!("a reset to [{option}]"))),
    }
}

/// The characters of `pieces`.
fn plain(pieces: &[Piece]) -> String {
    pieces.iter().map(|p| p.c).collect()
}

/// The characters a list names, a `-` neither quoted nor escaped between
/// two standing for those from the one to the other.
fn listed(pieces: &[Piece]) -> Result<Vec<char>> {
    let mut out: Vec<char> = Vec::new();
    let mut at = 0;
    while let Some(piece) = pieces.get(at) {
        at = at.saturating_add(1);
        if piece.plain && piece.c == RANGE && !out.is_empty() {
            let (Some(&from), Some(to)) = (out.last(), pieces.get(at)) else { return Err(damaged("a range with no end")) };
            at = at.saturating_add(1);
            let range =
                u32::from(from).checked_add(1).filter(|f| *f <= u32::from(to.c)).ok_or_else(|| damaged("a range that runs backwards"))?;
            out.extend((range..=u32::from(to.c)).filter_map(char::from_u32));
        } else {
            out.push(piece.c);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
