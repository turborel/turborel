//! [ISO/IEC 9075-2:2023 §6.1] What a character string's length counts:
//! its characters, or its octets as UTF-8 [Feature T062].

/// [ISO/IEC 9075-2:2023 §6.1] A `<char length units>`: what a character
/// string type's length counts \[SR16\], or a function's positions and
/// lengths [§6.31, §6.33; Feature T062].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CharUnits {
    /// CHARACTERS, implicit if none is written \[SR9\].
    #[default]
    Characters,
    /// OCTETS: of the string as UTF-8, its form of use.
    Octets,
}

impl CharUnits {
    /// How many of these units `s` is.
    pub fn count(self, s: &str) -> usize {
        match self {
            CharUnits::Characters => s.chars().count(),
            CharUnits::Octets => s.len(),
        }
    }

    /// The longest start of `s` of at most `most` of these units, whole
    /// characters.
    pub fn prefix(self, s: &str, most: usize) -> &str {
        let end = match self {
            CharUnits::Characters => s.char_indices().nth(most).map_or(s.len(), |(at, _)| at),
            CharUnits::Octets => (0..=most.min(s.len())).rev().find(|at| s.is_char_boundary(*at)).unwrap_or(0),
        };
        s.get(..end).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1 SR16] a string is
    /// counted in characters or in octets of its UTF-8, and its start of at
    /// most so many units is of whole characters, a character cut by the
    /// end left out.
    #[test]
    fn strings_are_counted_in_units_6_1() {
        assert_eq!((CharUnits::Characters.count("café"), CharUnits::Octets.count("café")), (4, 5));
        assert_eq!(CharUnits::Characters.prefix("café", 3), "caf");
        assert_eq!(CharUnits::Octets.prefix("café", 4), "caf", "é is two octets");
        assert_eq!(CharUnits::Octets.prefix("café", 5), "café");
        assert_eq!(CharUnits::Octets.prefix("café", 9), "café");
        assert_eq!(CharUnits::Octets.prefix("é", 1), "");
        assert_eq!(CharUnits::Characters.prefix("ab", 0), "");
    }
}
