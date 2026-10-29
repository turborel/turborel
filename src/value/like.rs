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
    let specs = specifiers(pattern, escape)?;
    let text: Vec<char> = value.chars().collect();
    // Matching left to right; on a mismatch, the last `%` takes one more
    // character and matching resumes after it. Every earlier `%` keeps what
    // it took, which suffices as each matches any sequence.
    let (mut at, mut spec) = (0_usize, 0_usize);
    let mut last_any: Option<(usize, usize)> = None;
    while at < text.len() {
        match specs.get(spec) {
            Some(Spec::AnyCharacter) => {
                at = at.saturating_add(1);
                spec = spec.saturating_add(1);
            }
            Some(Spec::Character(c)) if text.get(at) == Some(c) => {
                at = at.saturating_add(1);
                spec = spec.saturating_add(1);
            }
            Some(Spec::AnyString) => {
                last_any = Some((spec, at));
                spec = spec.saturating_add(1);
            }
            Some(Spec::Character(_)) | None => {
                let Some((any, from)) = last_any else { return Ok(false) };
                let from = from.saturating_add(1);
                last_any = Some((any, from));
                (at, spec) = (from, any.saturating_add(1));
            }
        }
    }
    Ok(specs.get(spec..).unwrap_or_default().iter().all(|s| *s == Spec::AnyString))
}

/// \[DESIGN\] Of a LIKE of constant `pattern` and `escape`, if it cannot
/// raise an error [X3.135-1986 §5.14 GR2a ii; ISO/IEC 9075:1992 §8.5 GR3a
/// i]: the characters every string it matches begins with, those its
/// pattern gives before its first `_` or `%`; none if a null makes every
/// LIKE of it unknown. Not if it may raise one.
pub fn constant_prefix(pattern: &Value, escape: Option<&Value>) -> Option<String> {
    let escape = match escape {
        None => None,
        Some(Value::Char(e)) => {
            let mut chars = e.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return None };
            Some(c)
        }
        Some(Value::Null) => return Some(String::new()),
        Some(Value::Exact(_) | Value::Approx(_) | Value::Bits(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_)) => {
            return None;
        }
    };
    match pattern {
        Value::Char(p) => {
            let specs = specifiers(p, escape).ok()?;
            let literal = specs.iter().map_while(|s| if let Spec::Character(c) = s { Some(*c) } else { None });
            Some(literal.collect())
        }
        Value::Null => Some(String::new()),
        Value::Exact(_) | Value::Approx(_) | Value::Bits(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_) => None,
    }
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
}
