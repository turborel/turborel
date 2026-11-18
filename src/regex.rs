//! XQuery regular expressions [ISO/IEC 9075-2:2023 §9.28; XQuery and XPath
//! Functions and Operators 3.1 (F&O 3.1) §5.6; XML Schema 1.1 Part 2 (XSD
//! 1.1 Part 2) Appendix G], which LIKE_REGEX matches, by a matcher of our
//! own (RULES.md D3, D61).
//!
//! A pattern is read under its flags (`regex/flags.rs`, `regex/parse.rs`,
//! `regex/class_expr.rs`) and compiled to a program (`regex/compile.rs`).
//! A program with no back-reference runs on `regex/pike.rs`, in time
//! linear in the string; one with a back-reference on `regex/backtrack.rs`,
//! which counts its steps. Both read a text by position (`regex/text.rs`),
//! a string's characters or a character large object's read a chunk at a
//! time (`regex/lob_text.rs`), so what a search holds is bounded whatever
//! the text's length. Character classes come from Unicode 13.0's General
//! Categories, blocks and case mappings (`regex/unicode.rs`).
//!
//! [§9.28 GR9a] The standard's changes to F&O are kept: `^`, `$` and `.`
//! know UTS #18 RL1.6's newlines, LF, VT, FF, CR, NEL, LINE SEPARATOR,
//! PARAGRAPH SEPARATOR and CRLF; `\s` matches a space, a tab or a newline
//! sequence, and `\S` any one character `\s` does not. Matching takes no
//! account of collations [F&O 3.1 §5.6.3].
//!
//! XQuery 1.0's functions read a pattern as [XQueryFO §7.6.1] defines it
//! (`Regex::xquery`): a newline is a line feed alone, for `.`, `^` and
//! `$`; `\s` is XML Schema's four white space characters; there is no
//! `(?:` and no flag `q`.

mod backtrack;
pub(crate) mod class;
mod class_expr;
mod compile;
mod flags;
mod lob_text;
mod parse;
mod pike;
mod replace;
mod text;
pub(crate) mod unicode;

use compile::{Program, compile};
use flags::{Flag, Flags};
use parse::{parse, parse_schema};
use unicode::Table;

pub(crate) use unicode::general_category;

pub use lob_text::LobText;
pub(crate) use replace::zero_length;
pub use replace::{Emit, Replacement};
pub use text::{Text, Window};

use crate::error::{Code, Result, SqlError, SqlState, fail_with};

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
        Regex::compiled(pattern, flag, false)
    }

    /// `pattern` compiled under `flag` as XQuery 1.0 reads them [XQueryFO
    /// §7.6.1, §7.6.1.1], what fn:matches, fn:replace and fn:tokenize
    /// match: a newline is a line feed alone, `.` matching any other
    /// character, `^` and `$` of each line after and before one; `\s` is
    /// XML Schema's four white space characters; there is no `(?:`; the
    /// flags are `s`, `m`, `i` and `x`.
    ///
    /// # Errors
    ///
    /// As `new`; 2201T of the flag `q` too.
    pub fn xquery(pattern: &str, flag: &str) -> Result<Regex> {
        let regex = Regex::compiled(pattern, flag, true)?;
        if regex.literal {
            return fail_with(
                Code::Data,
                SqlState::INVALID_XQUERY_OPTION_FLAG,
                format!("[XQueryFO §7.6.1.1] '{flag}' is not a flag of XQuery 1.0: 'q' is none of s, m, i and x"),
            );
        }
        Ok(regex)
    }

    /// `pattern` compiled under `flag`, as XQuery 1.0 reads it if
    /// `line_feeds`.
    fn compiled(pattern: &str, flag: &str, line_feeds: bool) -> Result<Regex> {
        let table = Table::read()?;
        let flags = match Flags::of(flag) {
            Ok(flags) => flags,
            Err(e) => {
                // [GR4] before [GR5]: a pattern no expression is that first.
                parse(pattern, Flags::default(), &table, line_feeds)?;
                return Err(e);
            }
        };
        let parsed = parse(pattern, flags, &table, line_feeds)?;
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

    /// `pattern` compiled as the value of an XML Schema `pattern` facet
    /// [Schema2 §4.3.4, Appendix F]: `is_match` is then true of a string
    /// the whole of which it matches [Schema2 §4.3.4.4], as no `^` or `$`
    /// anchors it there.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2201S if `pattern` is no regular expression of XML Schema;
    /// SQLCODE -900 past `MAX_INSTRUCTIONS`, or groups nested too deep.
    pub fn schema(pattern: &str) -> Result<Regex> {
        let table = Table::read()?;
        let parsed = parse_schema(pattern, &table)?;
        let program = compile(&parsed, Flags::default())?;
        Ok(Regex { program, groups: parsed.groups, back_references: false, caseless: false, literal: false, table })
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
        self.is_match_in(chars.as_slice())
    }

    /// Whether `text`, or some substring of it, matches [F&O 3.1 §5.6.3].
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if a search with back-references passes `MAX_STEPS`;
    /// as reading `text` fails.
    pub fn is_match_in<T: Text + ?Sized>(&self, text: &T) -> Result<bool> {
        Ok(self.search(text, 0, false)?.is_some())
    }

    /// The first match in `text` beginning at character `from` or after,
    /// characters from 0, `^` still the string's start.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if a search with back-references passes `MAX_STEPS`.
    pub fn find_at(&self, text: &[char], from: usize) -> Result<Option<Captures>> {
        self.find_in(text, from)
    }

    /// The first match in `text` beginning at character `from` or after,
    /// as `find_at`.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if a search with back-references passes `MAX_STEPS`;
    /// as reading `text` fails.
    pub fn find_in<T: Text + ?Sized>(&self, text: &T, from: usize) -> Result<Option<Captures>> {
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
        self.each(text, from, most, |next| {
            found.push(next);
            Ok(true)
        })?;
        Ok(found)
    }

    /// [§9.28 GR11] Gives `f` the match vectors of `text` from character
    /// `from`, at most `most`, as `list` finds them, one at a time and
    /// none kept, until it returns false.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if a search with back-references passes `MAX_STEPS`;
    /// as reading `text` fails, or `f` does.
    pub fn each<T, F>(&self, text: &T, from: usize, most: usize, mut f: F) -> Result<()>
    where
        T: Text + ?Sized,
        F: FnMut(Captures) -> Result<bool>,
    {
        let (mut at, mut given) = (from, 0_usize);
        while given < most && at <= text.len() {
            let Some(next) = self.find_in(text, at)? else { break };
            let (start, end) = next.get(0).unwrap_or((at, at));
            at = end.max(start.saturating_add(1));
            given = given.saturating_add(1);
            if !f(next)? {
                break;
            }
        }
        Ok(())
    }

    /// [§9.28 GR11] The match vector `n` of `text` from character `from`,
    /// from 1, as `list` finds them; none if there are fewer.
    ///
    /// # Errors
    ///
    /// As `each`.
    pub fn nth_in<T: Text + ?Sized>(&self, text: &T, from: usize, n: usize) -> Result<Option<Captures>> {
        let (mut seen, mut last) = (0_usize, None);
        self.each(text, from, n, |mv| {
            seen = seen.saturating_add(1);
            last = (seen == n).then_some(mv);
            Ok(true)
        })?;
        Ok(last)
    }

    /// [§9.28 GR11] How many match vectors `text` has from character
    /// `from`, as `list` finds them.
    ///
    /// # Errors
    ///
    /// As `each`.
    pub fn count_in<T: Text + ?Sized>(&self, text: &T, from: usize) -> Result<usize> {
        let mut seen = 0_usize;
        self.each(text, from, usize::MAX, |_| {
            seen = seen.saturating_add(1);
            Ok(true)
        })?;
        Ok(seen)
    }

    fn search<T: Text + ?Sized>(&self, text: &T, from: usize, capture: bool) -> Result<Option<pike::Slots>> {
        if self.back_references {
            return backtrack::search(&self.program, text, from, self.caseless.then_some(&self.table));
        }
        pike::search(&self.program, text, from, capture)
    }
}

#[cfg(test)]
mod tests;
