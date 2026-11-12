//! An `<XQuery replacement string>` [ISO/IEC 9075-2:2023 §9.29; F&O 3.1
//! §5.6.4]: what fn:replace puts in place of each match, its `$N` the
//! group N captured, and the replacing itself.

use super::{Captures, Regex};
use crate::error::{Code, Result, SqlError, SqlState};

/// The radix of the digits after a `$`.
const DECIMAL: u32 = 10;
/// What a digit before another counts for, ten.
const PLACE: usize = 10;

/// The greatest `$N` that is never cut short, a group or nothing
/// [F&O 3.1 §5.6.4].
const ALWAYS_A_GROUP: usize = 9;

/// A part of a replacement string, read.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    /// Characters as they are.
    Text(String),
    /// What group N captured, the whole match group 0.
    Group(usize),
}

/// The error of a replacement string that is none, `why` [§9.29 GR5].
fn invalid(why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-2:2023 §9.29 GR5] the replacement is not an XQuery replacement string: {why}"))
        .with_state(SqlState::INVALID_XQUERY_REPLACEMENT_STRING)
}

/// The error of a match of no characters, which nothing replaces, `why`
/// [§9.29 GR3].
pub(crate) fn zero_length(why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-2:2023 §9.29 GR3] a zero-length string cannot be replaced: {why}"))
        .with_state(SqlState::ATTEMPT_TO_REPLACE_A_ZERO_LENGTH_STRING)
}

/// `replacement` read for a pattern of `groups` groups, as it is under the
/// flag `q` if `literal`.
///
/// # Errors
///
/// SQLSTATE 2201V of a `$` before no digit, or a `\` before neither `\`
/// nor `$` [F&O 3.1 §5.6.4].
fn template(replacement: &str, groups: usize, literal: bool) -> Result<Vec<Piece>> {
    if literal {
        return Ok(vec![Piece::Text(replacement.to_owned())]);
    }
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(escaped @ ('\\' | '$')) => text.push(escaped),
                Some(other) => return Err(invalid(&format!("'\\{other}' escapes neither '\\' nor '$'"))),
                None => return Err(invalid("a '\\' ends it")),
            },
            '$' => {
                let mut digits = Vec::new();
                while let Some(d) = chars.peek().and_then(|next| next.to_digit(DECIMAL)) {
                    digits.push(d);
                    chars.next();
                }
                if digits.is_empty() {
                    return Err(invalid("a '$' is followed by no digit"));
                }
                pieces.push(Piece::Text(std::mem::take(&mut text)));
                pieces.extend(variable(&digits, groups));
            }
            other => text.push(other),
        }
    }
    pieces.push(Piece::Text(text));
    Ok(pieces)
}

/// [F&O 3.1 §5.6.4] What `$` and `digits` stand for beside `groups` groups:
/// group N; nothing if N names no group but is at most 9; else the last
/// digit as itself after what the others stand for.
fn variable(digits: &[u32], groups: usize) -> Vec<Piece> {
    let mut kept = digits.len();
    let number =
        |count: usize| digits.iter().take(count).try_fold(0_usize, |n, d| n.checked_mul(PLACE)?.checked_add(usize::try_from(*d).ok()?));
    while kept > 1 && number(kept).is_none_or(|n| n > groups && n > ALWAYS_A_GROUP) {
        kept = kept.saturating_sub(1);
    }
    let mut pieces = Vec::new();
    if let Some(n) = number(kept).filter(|n| *n <= groups) {
        pieces.push(Piece::Group(n));
    }
    let rest: String = digits.iter().skip(kept).filter_map(|d| char::from_digit(*d, DECIMAL)).collect();
    pieces.push(Piece::Text(rest));
    pieces
}

impl Regex {
    /// [§9.29 GR9a; F&O 3.1 §5.6.4] fn:replace of `input`: each match, the
    /// first of those that overlap, replaced by `replacement`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2201V if `replacement` is no XQuery replacement string;
    /// 2201U if the pattern matches the zero-length string, which F&O
    /// refuses; SQLCODE -900 if a search passes `MAX_STEPS`.
    pub fn replace(&self, input: &[char], replacement: &str) -> Result<String> {
        let pieces = template(replacement, self.groups, self.literal)?;
        if self.is_match("")? {
            return Err(zero_length("the pattern matches the zero-length string [F&O 3.1 §5.6.4]"));
        }
        let mut out = String::new();
        let mut at = 0;
        while let Some(found) = self.find_at(input, at)? {
            let Some((start, end)) = found.get(0).filter(|(start, end)| end > start) else {
                return Err(zero_length("the pattern matches a zero-length string within the match"));
            };
            out.extend(input.get(at..start).unwrap_or_default());
            substituted(&mut out, &pieces, &found, input);
            at = end;
        }
        out.extend(input.get(at..).unwrap_or_default());
        Ok(out)
    }
}

/// Puts on `out` `pieces`, each group what `found` captured of `input`.
fn substituted(out: &mut String, pieces: &[Piece], found: &Captures, input: &[char]) {
    for piece in pieces {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Group(g) => {
                if let Some((a, b)) = found.get(*g) {
                    out.extend(input.get(a..b).unwrap_or_default());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replaced(input: &str, pattern: &str, replacement: &str, flag: &str) -> Result<String> {
        let chars: Vec<char> = input.chars().collect();
        Regex::new(pattern, flag)?.replace(&chars, replacement)
    }

    /// Verifies: [EXTENSION] [F&O 3.1 §5.6.4] fn:replace's examples: each
    /// match replaced, the first of overlapping ones; `$N` a group, `$0`
    /// the whole match; the flag `q` taking the replacement as it is.
    #[test]
    fn the_examples_of_fn_replace() {
        assert_eq!(replaced("abracadabra", "bra", "*", "").unwrap(), "a*cada*");
        assert_eq!(replaced("abracadabra", "a.*a", "*", "").unwrap(), "*");
        assert_eq!(replaced("abracadabra", "a.*?a", "*", "").unwrap(), "*c*bra");
        assert_eq!(replaced("abracadabra", "a", "", "").unwrap(), "brcdbr");
        assert_eq!(replaced("abracadabra", "a(.)", "a$1$1", "").unwrap(), "abbraccaddabbra");
        assert_eq!(replaced("AAAA", "A+", "b", "").unwrap(), "b");
        assert_eq!(replaced("AAAA", "A+?", "b", "").unwrap(), "bbbb");
        assert_eq!(replaced("darted", "^(.*?)d(.*)$", "$1c$2", "").unwrap(), "carted");
        assert_eq!(replaced("abcd", "(ab)|(a)", "[1=$1][2=$2]", "").unwrap(), "[1=ab][2=]cd");
        assert_eq!(replaced("a.b", ".", "$0\\$", "q").unwrap(), "a$0\\$b");
        assert_eq!(replaced("ab", "b", "[$0]\\\\", "").unwrap(), "a[b]\\");
    }

    /// Verifies: [EXTENSION] [F&O 3.1 §5.6.4] `$N` takes every digit after
    /// the `$`, then gives back the last while N names no group and is
    /// more than 9; N at most 9 and of no group is nothing.
    #[test]
    fn a_variable_takes_the_digits_it_can() {
        let five = "(a)(b)(c)(d)(e)";
        assert_eq!(replaced("abcde", five, "$23", "").unwrap(), "b3");
        assert_eq!(replaced("abcde", five, "$7", "").unwrap(), "");
        assert_eq!(replaced("abcde", five, "$99", "").unwrap(), "9");
        assert_eq!(replaced("abcde", five, "$123456789012345678901234567890", "").unwrap(), "a23456789012345678901234567890");
        let eleven = "(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)(k)";
        assert_eq!(replaced("abcdefghijk", eleven, "$11-$12", "").unwrap(), "k-a2");
        assert_eq!(replaced("abc", "(a)(b)(c)", "[$05]", "").unwrap(), "[]");
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.29 GR3, GR5; F&O 3.1
    /// §5.6.4] a replacement with a `$` before no digit or a `\` before
    /// neither `\` nor `$` is SQLSTATE 2201V, unless the flag is `q`; a
    /// pattern that matches the zero-length string is 2201U.
    #[test]
    fn replacements_that_are_none() {
        for bad in ["$", "a$b", "\\", "\\n", "x\\"] {
            assert_eq!(replaced("a", "a", bad, "").unwrap_err().sqlstate().as_str(), "2201V", "{bad}");
            assert_eq!(replaced("a", "a", bad, "q").unwrap(), bad);
        }
        assert_eq!(replaced("abc", "x*", "-", "").unwrap_err().sqlstate().as_str(), "2201U");
        assert_eq!(replaced("abc", "$", "-", "").unwrap_err().sqlstate().as_str(), "2201U");
        assert_eq!(replaced("a", "a|^$", "b", "").unwrap_err().sqlstate().as_str(), "2201U", "though no match in the input is empty");
        assert_eq!(replaced("abc", "x*", "$", "").unwrap_err().sqlstate().as_str(), "2201V", "the replacement read first");
    }
}
