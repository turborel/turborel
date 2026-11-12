//! XQuery regular expressions [ISO/IEC 9075-2:2023 §9.28; XQuery and XPath
//! Functions and Operators 3.1 (F&O 3.1) §5.6; XML Schema 1.1 Part 2 (XSD
//! 1.1 Part 2) Appendix G], which LIKE_REGEX matches, by a matcher of our
//! own (RULES.md D3, D61).
//!
//! A pattern is read under its flags (`regex/flags.rs`, `regex/parse.rs`,
//! `regex/class_expr.rs`) and compiled to a program (`regex/compile.rs`).
//! A program with no back-reference runs on `regex/pike.rs`, in time
//! linear in the string; one with a back-reference on `regex/backtrack.rs`,
//! which counts its steps. Character classes come from Unicode 13.0's
//! General Categories, blocks and case mappings (`regex/unicode.rs`).
//!
//! [§9.28 GR9a] The standard's changes to F&O are kept: `^`, `$` and `.`
//! know UTS #18 RL1.6's newlines, LF, VT, FF, CR, NEL, LINE SEPARATOR,
//! PARAGRAPH SEPARATOR and CRLF; `\s` matches a space, a tab or a newline
//! sequence, and `\S` any one character `\s` does not. Matching takes no
//! account of collations [F&O 3.1 §5.6.3].

mod backtrack;
mod class;
mod class_expr;
mod compile;
mod flags;
mod parse;
mod pike;
mod replace;
mod unicode;

use compile::{Program, compile};
use flags::{Flag, Flags};
use parse::parse;
use unicode::Table;

pub(crate) use replace::zero_length;

use crate::error::{Code, Result, SqlError, SqlState};

/// The error of a pattern that is no XQuery regular expression, `why`
/// [§9.28 GR4].
fn invalid(why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-2:2023 §9.28 GR4] the pattern is not an XQuery regular expression: {why}"))
        .with_state(SqlState::INVALID_XQUERY_REGULAR_EXPRESSION)
}

/// The error of a pattern beyond turborel's limits, `why` (RULES.md D18).
fn beyond(why: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] the regular expression is beyond turborel's limits: {why}"))
}

/// A regular expression compiled.
#[derive(Debug, Clone)]
pub struct Regex {
    program: Program,
    groups: usize,
    back_references: bool,
    caseless: bool,
    literal: bool,
    table: Table,
}

/// A match: where it, and each group of it, begins and ends, characters
/// from 0, its end excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captures {
    spans: Vec<Option<(usize, usize)>>,
}

impl Captures {
    /// Where group `group` begins and ends, the whole match group 0; none
    /// if it matched nothing in this match [§9.28 GR10].
    pub fn get(&self, group: usize) -> Option<(usize, usize)> {
        self.spans.get(group).copied().flatten()
    }
}

impl Regex {
    /// `pattern` compiled under the `<XQuery option flag>` `flag`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2201S if `pattern` is no XQuery regular expression
    /// \[GR4\], else 2201T if `flag` is no XQuery option flag \[GR5\];
    /// SQLCODE -900 past `MAX_INSTRUCTIONS`, or groups nested too deep.
    pub fn new(pattern: &str, flag: &str) -> Result<Regex> {
        let table = Table::read()?;
        let flags = match Flags::of(flag) {
            Ok(flags) => flags,
            Err(e) => {
                // [GR4] before [GR5]: a pattern no expression is that first.
                parse(pattern, Flags::default(), &table)?;
                return Err(e);
            }
        };
        let parsed = parse(pattern, flags, &table)?;
        let program = compile(&parsed, flags)?;
        Ok(Regex {
            program,
            groups: parsed.groups,
            back_references: parsed.back_references,
            caseless: flags.has(Flag::Caseless),
            literal: flags.has(Flag::Literal),
            table,
        })
    }

    /// How many groups capture [§9.28 GR8].
    pub fn groups(&self) -> usize {
        self.groups
    }

    /// Whether `text`, or some substring of it, matches [F&O 3.1 §5.6.3].
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if a search with back-references passes `MAX_STEPS`.
    pub fn is_match(&self, text: &str) -> Result<bool> {
        let chars: Vec<char> = text.chars().collect();
        Ok(self.search(&chars, 0, false)?.is_some())
    }

    /// The first match in `text` beginning at character `from` or after,
    /// characters from 0, `^` still the string's start.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if a search with back-references passes `MAX_STEPS`.
    pub fn find_at(&self, text: &[char], from: usize) -> Result<Option<Captures>> {
        let Some(slots) = self.search(text, from, true)? else { return Ok(None) };
        let spans = slots.chunks(2).map(|pair| if let [Some(a), Some(b)] = pair { Some((*a, *b)) } else { None }).collect();
        Ok(Some(Captures { spans }))
    }

    /// [§9.28 GR11] The match vectors of `text` from character `from`, at
    /// most `most`: each the first match beginning after the one before
    /// it begins, and not before it ends.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if a search with back-references passes `MAX_STEPS`.
    pub fn list(&self, text: &[char], from: usize, most: usize) -> Result<Vec<Captures>> {
        let mut found = Vec::new();
        let mut at = from;
        while found.len() < most && at <= text.len() {
            let Some(next) = self.find_at(text, at)? else { break };
            let (start, end) = next.get(0).unwrap_or((at, at));
            at = end.max(start.saturating_add(1));
            found.push(next);
        }
        Ok(found)
    }

    fn search(&self, text: &[char], from: usize, capture: bool) -> Result<Option<pike::Slots>> {
        if self.back_references {
            return backtrack::search(&self.program, text, from, self.caseless.then_some(&self.table));
        }
        Ok(pike::search(&self.program, text, from, capture))
    }
}

#[cfg(test)]
mod tests;
