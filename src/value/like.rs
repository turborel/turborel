//! [X3.135-1986 §5.14] `<like predicate>`: whether a character string
//! matches a pattern.
//!
//! The pattern is split into substring specifiers [GR2, GR3]: an arbitrary
//! character specifier (`_`), an arbitrary string specifier (`%`), or a
//! character that represents itself. With an `<escape character>` z, the
//! pattern must be partitioned into substrings of length 1, none of them z,
//! and of length 2, z followed by z, `_` or `%`, which represent their
//! second character [GR2a]; a pattern with no such partitioning is a data
//! exception.
//!
//! The string matches if it can be split into as many substrings, in order,
//! each a single character for `_`, any sequence for `%`, and for any other
//! specifier a substring equal to it and of the same length [GR5]. So no
//! padding is involved: a `CHARACTER(5)` value 'ABC  ' does not match the
//! pattern 'ABC', but matches 'ABC%' and 'ABC  '.

use std::collections::HashMap;

use super::Value;
use crate::error::{Code, Result, SqlState, fail_with};

/// A substring specifier of a pattern, one character long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spec {
    /// `_`: any single character.
    AnyCharacter,
    /// `%`: any sequence of zero or more characters.
    AnyString,
    /// A character that represents itself.
    Character(char),
}

/// The specifiers of `pattern`, with `escape` as its escape character if
/// one was given [GR2].
fn specifiers(pattern: &str, escape: Option<char>) -> Result<Vec<Spec>> {
    let mut specs = Vec::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        let spec = if Some(c) == escape {
            match chars.next() {
                Some(next) if Some(next) == escape || next == '_' || next == '%' => Spec::Character(next),
                Some(_) | None => {
                    return fail_with(
                        Code::Data,
                        SqlState::INVALID_ESCAPE_SEQUENCE,
                        "[X3.135-1986 §5.14 GR2a] in the <pattern>, the <escape character> must be followed by itself, _ or %",
                    );
                }
            }
        } else if c == '_' {
            Spec::AnyCharacter
        } else if c == '%' {
            Spec::AnyString
        } else {
            Spec::Character(c)
        };
        specs.push(spec);
    }
    Ok(specs)
}

/// [X3.135-1986 §5.14 GR5] whether `value` matches `pattern`, `escape` its
/// escape character if one was given.
///
/// # Errors
///
/// SQLCODE -301 if the pattern has no partitioning by the escape character
/// [X3.135-1986 §5.14 GR2a ii].
pub fn like(value: &str, pattern: &str, escape: Option<char>) -> Result<bool> {
    Ok(LikePattern::new(pattern, escape)?.matches(value))
}

/// \[DESIGN\] A `<pattern>` split into its specifiers once, to match many
/// strings by (`exec/batch/judge.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LikePattern {
    specs: Vec<Spec>,
}

impl LikePattern {
    /// `pattern`, `escape` its escape character if one was given.
    ///
    /// # Errors
    ///
    /// As `like`.
    pub fn new(pattern: &str, escape: Option<char>) -> Result<LikePattern> {
        Ok(LikePattern { specs: specifiers(pattern, escape)? })
    }

    /// [X3.135-1986 §5.14 GR5] Whether `value` matches it: an ASCII string
    /// by its bytes, no character gathered; any other by its characters.
    pub fn matches(&self, value: &str) -> bool {
        if value.is_ascii() {
            return matched(&self.specs, value.as_bytes(), |b, c| u32::from(b) == u32::from(c));
        }
        let text: Vec<char> = value.chars().collect();
        matched(&self.specs, &text, |t, c| t == c)
    }
}

/// \[DESIGN\] The most patterns `LikePatterns` holds split: past them, all
/// are let go, so a LIKE of a pattern a row's column gives, unlike in
/// every row, holds no more memory than this many.
const PATTERNS_HELD: usize = 256;

/// \[DESIGN\] Patterns split once each (`LikePattern`), each kept by its
/// text and escape character as it is first matched, to match many
/// strings by: so a LIKE whose pattern is not a constant splits each
/// pattern once, not once a row (`exec/eval.rs`, `exec/batch/judge.rs`).
#[derive(Debug, Default)]
pub struct LikePatterns {
    /// By escape character, each pattern split.
    held: HashMap<Option<char>, HashMap<String, LikePattern>>,
    /// How many are held.
    count: usize,
}

impl LikePatterns {
    /// [X3.135-1986 §5.14 GR5] Whether `value` matches `pattern`, `escape`
    /// its escape character if one was given, as `like` says: the pattern
    /// split the first time it is matched.
    ///
    /// # Errors
    ///
    /// As `like`; a pattern that raises is split again each time.
    pub fn matches(&mut self, value: &str, pattern: &str, escape: Option<char>) -> Result<bool> {
        if let Some(split) = self.held.get(&escape).and_then(|patterns| patterns.get(pattern)) {
            return Ok(split.matches(value));
        }
        let split = LikePattern::new(pattern, escape)?;
        let matched = split.matches(value);
        if self.count >= PATTERNS_HELD {
            self.held.clear();
            self.count = 0;
        }
        self.held.entry(escape).or_default().insert(pattern.to_owned(), split);
        self.count = self.count.saturating_add(1);
        Ok(matched)
    }
}

/// Whether `text` matches `specs`, `same` telling a unit of it equal to a
/// character. Matching left to right; on a mismatch, the last `%` takes one
/// more unit and matching resumes after it. Every earlier `%` keeps what it
/// took, which suffices as each matches any sequence.
fn matched<T: Copy>(specs: &[Spec], text: &[T], same: impl Fn(T, char) -> bool) -> bool {
    let (mut at, mut spec) = (0_usize, 0_usize);
    let mut last_any: Option<(usize, usize)> = None;
    while at < text.len() {
        let here = text.get(at).copied();
        let step = match specs.get(spec) {
            Some(Spec::AnyCharacter) => true,
            Some(Spec::Character(c)) => here.is_some_and(|t| same(t, *c)),
            Some(Spec::AnyString) => {
                last_any = Some((spec, at));
                spec = spec.saturating_add(1);
                continue;
            }
            None => false,
        };
        if step {
            at = at.saturating_add(1);
            spec = spec.saturating_add(1);
            continue;
        }
        let Some((any, from)) = last_any else { return false };
        let resumed = from.saturating_add(1);
        last_any = Some((any, resumed));
        (at, spec) = (resumed, any.saturating_add(1));
    }
    specs.get(spec..).unwrap_or_default().iter().all(|s| *s == Spec::AnyString)
}

/// \[DESIGN\] Of a LIKE of constant `pattern` and `escape`, if it cannot
/// raise an error [X3.135-1986 §5.14 GR2a ii; ISO/IEC 9075:1992 §8.5 GR3a
/// i]: the pattern, split once; none if a null makes every LIKE of it
/// unknown [ISO/IEC 9075:1992 §8.5 GR1, GR2]. Not if it may raise one.
pub fn constant_pattern(pattern: &Value, escape: Option<&Value>) -> Option<Option<LikePattern>> {
    let escape = match escape {
        None => None,
        Some(Value::Char(e)) => {
            let mut chars = e.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return None };
            Some(c)
        }
        Some(Value::Null) => return Some(None),
        Some(Value::Exact(_) | Value::Approx(_) | Value::Bits(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_)) => {
            return None;
        }
    };
    match pattern {
        Value::Char(p) => LikePattern::new(p, escape).ok().map(Some),
        Value::Null => Some(None),
        Value::Exact(_) | Value::Approx(_) | Value::Bits(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_) => None,
    }
}

/// \[DESIGN\] Of a LIKE of constant `pattern` and `escape`, if it cannot
/// raise an error (`constant_pattern`): the characters every string it
/// matches begins with, those its pattern gives before its first `_` or
/// `%`; none if it may raise one; empty if a null makes every LIKE of it
/// unknown.
pub fn constant_prefix(pattern: &Value, escape: Option<&Value>) -> Option<String> {
    let Some(compiled) = constant_pattern(pattern, escape)? else { return Some(String::new()) };
    let literal = compiled.specs.iter().map_while(|s| if let Spec::Character(c) = s { Some(*c) } else { None });
    Some(literal.collect())
}

// Unit tests: the specifiers, escapes, and padding (RULES.md R-TEST-1,
// R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [X3.135-1986 §5.14 GR2b, GR5] `_` takes one character, `%`
    /// any number, others themselves; no padding: 'ABC  ' does not match
    /// 'ABC' but matches 'ABC%' and 'ABC  '; characters of several bytes are
    /// one character each.
    #[test]
    fn patterns_match_by_their_specifiers() {
        let m = |v: &str, p: &str| like(v, p, None).unwrap();
        for (value, pattern) in [
            ("ABC", "ABC"),
            ("ABC", "A_C"),
            ("ABC", "%"),
            ("", "%"),
            ("ABC", "A%"),
            ("ABC", "%C"),
            ("ABC", "%B%"),
            ("ABCABC", "%A_C"),
            ("ABC  ", "ABC%"),
            ("ABC  ", "ABC  "),
            ("ÉTÉ", "_T_"),
            ("AXXXBXXXC", "A%B%C"),
            ("ABAB", "%AB"),
        ] {
            assert!(m(value, pattern), "{value:?} LIKE {pattern:?}");
        }
        for (value, pattern) in [("ABC  ", "ABC"), ("ABC", "AB"), ("ABC", "ABCD"), ("ABC", "A_"), ("", "_"), ("ABC", "%D%"), ("AB", "A%C")]
        {
            assert!(!m(value, pattern), "{value:?} NOT LIKE {pattern:?}");
        }
    }

    /// Verifies: [X3.135-1986 §5.14 GR2a] with an escape character, it
    /// followed by `_`, `%` or itself represents that character; followed by
    /// anything else, or last, the pattern has no partitioning: SQLCODE -301.
    #[test]
    fn escapes_represent_their_next_character() {
        let m = |v: &str, p: &str| like(v, p, Some('!'));
        assert!(m("50%", "50!%").unwrap());
        assert!(!m("500", "50!%").unwrap());
        assert!(m("A_B", "A!_B").unwrap());
        assert!(!m("AXB", "A!_B").unwrap());
        assert!(m("A!", "A!!").unwrap());
        assert!(m("50% off", "50!%%").unwrap());
        for bad in ["A!B", "A!"] {
            assert_eq!(m("A", bad).unwrap_err().code, Code::Data, "{bad}");
        }
    }

    /// Verifies: [DESIGN] [X3.135-1986 §5.14 GR2a, GR5] patterns split once
    /// each match as `like` does: each pattern held once, by its escape
    /// character too, so the same text with another escape is another
    /// pattern; one with no partitioning raises SQLCODE -301 each time and
    /// is not held; past the most held, all are let go and matching goes
    /// on alike.
    #[test]
    fn patterns_split_once_each_match_as_like() {
        let mut patterns = LikePatterns::default();
        let cases =
            [("50%", "50!%", Some('!')), ("500", "50!%", Some('!')), ("50!", "50!%", None), ("ABC", "A_C", None), ("AB", "A_C", None)];
        for _ in 0_u8..2_u8 {
            for (value, pattern, escape) in cases {
                assert_eq!(patterns.matches(value, pattern, escape).unwrap(), like(value, pattern, escape).unwrap(), "{value} {pattern}");
            }
        }
        assert_eq!(patterns.count, 3, "50!% twice, by its escapes, and A_C");
        for _ in 0_u8..2_u8 {
            assert_eq!(patterns.matches("A", "A!B", Some('!')).unwrap_err().code, Code::Data);
        }
        assert_eq!(patterns.count, 3, "not held");
        for n in 0..PATTERNS_HELD {
            assert!(patterns.matches(&format!("x{n}"), &format!("x{n}%"), None).unwrap());
        }
        assert!(patterns.count <= PATTERNS_HELD, "let go: {}", patterns.count);
        assert!(patterns.matches("ABC", "A_C", None).unwrap(), "after");
    }
}
