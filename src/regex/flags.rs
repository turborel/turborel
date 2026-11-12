//! An `<XQuery option flag>` [ISO/IEC 9075-2:2023 §9.28 GR5; F&O 3.1
//! §5.6.2]: letters each turning an option on, in any order, repeated or
//! not, and what the flag `x` strips from a pattern.

use crate::error::{Code, Result, SqlState, fail_with};

/// An option a flag letter turns on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Flag {
    /// `s`: dot-all, `.` matching a newline too.
    DotAll,
    /// `m`: multi-line, `^` and `$` matching at each line's ends.
    MultiLine,
    /// `i`: case-insensitive, a character matching its case variants.
    Caseless,
    /// `x`: white space in the pattern, outside a class, ignored.
    Extended,
    /// `q`: every character of the pattern itself, none a metacharacter.
    Literal,
}

/// Each flag, and its letter.
const LETTERS: [(char, Flag); 5] =
    [('s', Flag::DotAll), ('m', Flag::MultiLine), ('i', Flag::Caseless), ('x', Flag::Extended), ('q', Flag::Literal)];

/// The options a flag turns on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Flags(u8);

impl Flags {
    /// The options of `text`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2201T if a character of it is no flag's letter \[GR5\].
    pub(super) fn of(text: &str) -> Result<Flags> {
        let mut bits = 0_u8;
        for c in text.chars() {
            let Some((_, flag)) = LETTERS.iter().find(|(letter, _)| *letter == c) else {
                return fail_with(
                    Code::Data,
                    SqlState::INVALID_XQUERY_OPTION_FLAG,
                    format!("[ISO/IEC 9075-2:2023 §9.28 GR5] '{text}' is not an XQuery option flag: '{c}' is none of s, m, i, x and q"),
                );
            };
            bits |= Flags::bit(*flag);
        }
        Ok(Flags(bits))
    }

    fn bit(flag: Flag) -> u8 {
        match flag {
            Flag::DotAll => 1,
            Flag::MultiLine => 1 << 1,
            Flag::Caseless => 1 << 2,
            Flag::Extended => 1 << 3,
            Flag::Literal => 1 << 4,
        }
    }

    /// Whether `flag` is on: `m`, `s` and `x` having no effect beside `q`
    /// [F&O 3.1 §5.6.2].
    pub(super) fn has(self, flag: Flag) -> bool {
        let literal = self.0 & Flags::bit(Flag::Literal) != 0;
        let ignored = literal && matches!(flag, Flag::DotAll | Flag::MultiLine | Flag::Extended);
        self.0 & Flags::bit(flag) != 0 && !ignored
    }
}

/// [F&O 3.1 §5.6.2] `pattern` without its white space, #x9, #xA, #xD and
/// #x20, but within a character class expression, as the flag `x` reads
/// it. An escaping backslash outside a class escapes the next character
/// kept, so "\ s" is "\s".
pub(super) fn stripped(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let (mut depth, mut escaped) = (0_usize, false);
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if depth == 0 && matches!(c, '\t' | '\n' | '\r' | ' ') {
            continue;
        }
        out.push(c);
        if depth > 0 && c == '\\' {
            out.extend(chars.next());
            continue;
        }
        match (escaped, c) {
            (false, '\\') => escaped = true,
            (false, '[') => depth = depth.saturating_add(1),
            (false, ']') => depth = depth.saturating_sub(1),
            (true, _) | (false, _) => escaped = false,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.28 GR5; F&O 3.1
    /// §5.6.2] each letter turns its option on, repeated or in any order;
    /// any other character is SQLSTATE 2201T; `q` leaves `m`, `s` and `x`
    /// without effect, but not `i`.
    #[test]
    fn flags_9_28() {
        let f = Flags::of("ixsi").unwrap();
        assert!(f.has(Flag::Caseless) && f.has(Flag::Extended) && f.has(Flag::DotAll) && !f.has(Flag::MultiLine));
        assert_eq!(Flags::of("").unwrap(), Flags::default());
        let e = Flags::of("ig").unwrap_err();
        assert_eq!(e.sqlstate().as_str(), "2201T");
        assert!(Flags::of("I").is_err());
        let q = Flags::of("qmsxi").unwrap();
        assert!(q.has(Flag::Literal) && q.has(Flag::Caseless));
        assert!(!q.has(Flag::MultiLine) && !q.has(Flag::DotAll) && !q.has(Flag::Extended));
    }

    /// Verifies: [EXTENSION] [F&O 3.1 §5.6.2] the flag `x` strips white
    /// space outside character class expressions, a subtraction's
    /// included, and keeps it within them; an escaped bracket opens none.
    #[test]
    fn extended_strips_white_space() {
        assert_eq!(stripped("hello world"), "helloworld");
        assert_eq!(stripped("hello[ ]world"), "hello[ ]world");
        assert_eq!(stripped("hello\\ sworld"), "hello\\sworld");
        assert_eq!(stripped("[a-z-[ b]] c"), "[a-z-[ b]]c");
        assert_eq!(stripped("\\[ a ]"), "\\[a]");
        assert_eq!(stripped("[\\] ] x"), "[\\] ]x");
    }
}
