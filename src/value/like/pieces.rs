//! \[DESIGN\] (RULES.md D1074) A `<pattern>` of no arbitrary character
//! specifier, `_`, matched by its pieces: the strings between its
//! arbitrary string specifiers, `%`. A value matches it [X3.135-1986
//! §5.14 GR5] if it begins with the first piece, ends with the last past
//! the first, and holds the pieces between, in order and apart, in what
//! lies between those two. Each piece between is looked for from where
//! the piece before it ended, and the first place it is found is taken,
//! which leaves the most of the value to the pieces after it, so no
//! other choice finds a match this one misses. A piece is searched for as
//! a substring (`str::find`), not a character at a time with the pattern
//! begun again at each place it fails.
//!
//! A piece is whole characters, and the encoding of a character never
//! begins inside another's, so a piece found among a value's bytes is
//! found at a character of it, and a value of any characters is matched
//! by its bytes.

use super::Spec;

/// The pieces of a pattern of no `_`: more than one if it has a `%`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pieces {
    /// What a value begins with; all of it, of a pattern of no `%`.
    first: String,
    /// The pieces a value holds past the first and before the last, none
    /// empty.
    between: Vec<String>,
    /// What a value ends with, of a pattern of some `%`.
    last: Option<String>,
}

impl Pieces {
    /// The pieces of the pattern of `specs`; none if one of them is `_`.
    pub(super) fn of(specs: &[Spec]) -> Option<Pieces> {
        let mut pieces = vec![String::new()];
        for spec in specs {
            match spec {
                Spec::AnyCharacter => return None,
                Spec::AnyString => pieces.push(String::new()),
                Spec::Character(c) => pieces.last_mut()?.push(*c),
            }
        }
        let last = if pieces.len() > 1 { pieces.pop() } else { None };
        let mut rest = pieces.into_iter();
        let first = rest.next()?;
        Some(Pieces { first, between: rest.filter(|piece| !piece.is_empty()).collect(), last })
    }

    /// [X3.135-1986 §5.14 GR5] Whether `value` matches the pattern.
    pub(super) fn matches(&self, value: &str) -> bool {
        let Some(last) = &self.last else { return value == self.first };
        let Some(after) = value.strip_prefix(self.first.as_str()) else { return false };
        let Some(mut rest) = after.strip_suffix(last.as_str()) else { return false };
        for piece in &self.between {
            let Some(at) = rest.find(piece.as_str()) else { return false };
            rest = rest.get(at.saturating_add(piece.len())..).unwrap_or_default();
        }
        true
    }
}

// Unit tests: pieces against the specifiers' own matching (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::super::{LikePattern, matched};
    use super::*;

    /// Whether `value` matches `pattern` a specifier at a time.
    fn slowly(value: &str, pattern: &LikePattern) -> bool {
        let text: Vec<char> = value.chars().collect();
        matched(&pattern.specs, &text, |t, c| t == c)
    }

    /// Verifies: [X3.135-1986 §5.14 GR5] (RULES.md D1074) a pattern of no
    /// `_` matches by its pieces the values it matches a specifier at a
    /// time: of no `%`, the value itself alone; a piece at each end, which
    /// may not overlap; pieces between, in order and apart, the first place
    /// each is found taken; `%` alone and doubled; the empty value; and
    /// characters of several bytes, one a suffix of another's bytes never
    /// found inside it.
    #[test]
    fn pieces_match_as_the_specifiers_do() {
        let patterns = [
            "",
            "%",
            "%%",
            "ab",
            "ab%",
            "%ab",
            "%ab%",
            "a%b",
            "ab%ab",
            "%a%b%",
            "a%b%c",
            "%ab%ba%",
            "aba%ba",
            "é%",
            "%é",
            "%€%é%",
            "%ab%ab%ab",
        ];
        let values = [
            "", "a", "ab", "aba", "abab", "ababab", "ba", "abba", "abXba", "aXbXc", "abc", "é", "éé", "aé", "€é", "é€", "x€yéz", "ab ",
            " ab",
        ];
        for p in patterns {
            let pattern = LikePattern::new(p, None).unwrap();
            let pieces = Pieces::of(&pattern.specs).unwrap_or_else(|| panic!("{p:?} has no pieces"));
            for v in values {
                assert_eq!(pieces.matches(v), slowly(v, &pattern), "{v:?} LIKE {p:?}");
                assert_eq!(pattern.matches(v), slowly(v, &pattern), "{v:?} LIKE {p:?}, as the pattern matches");
            }
        }
    }

    /// Verifies: [X3.135-1986 §5.14 GR2a, GR2b] (RULES.md D1074) a pattern
    /// with `_` has no pieces, and matches a specifier at a time; an
    /// escaped `_` or `%` is a character of a piece.
    #[test]
    fn an_arbitrary_character_has_no_pieces() {
        for p in ["_", "a_b", "%a_%", "_%"] {
            assert_eq!(Pieces::of(&LikePattern::new(p, None).unwrap().specs), None, "{p:?}");
        }
        let escaped = LikePattern::new("%50!%!_%", Some('!')).unwrap();
        let pieces = Pieces::of(&escaped.specs).unwrap();
        assert!(pieces.matches("at 50%_ off") && !pieces.matches("at 50%x off") && !pieces.matches("at 500_ off"));
        assert!(escaped.matches("at 50%_ off") && !escaped.matches("at 500_ off"));
    }
}
