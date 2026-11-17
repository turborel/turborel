//! The characters of XML text, by version: which characters a document
//! may hold [XML 1.0 §2.2 \[2\]] [XML 1.1 §2.2 \[2\], \[2a\]], white space
//! [§2.3 \[3\]], and the characters names are made of [XML 1.0 §2.3 \[4\],
//! \[5\], Appendix B] [XML 1.1 §2.3 \[4\], \[4a\], \[5\]], with the names
//! Namespaces in XML makes of them, which hold no colon [Namespaces 1.0
//! §2 \[4\], \[5\]] [Namespaces 1.1 §3 \[4\], \[5\]].
//!
//! XML 1.0 Third Edition names its letters by Unicode 2.0's tables
//! (`chars/names10.rs`); XML 1.1 by open ranges, so a character Unicode
//! adds later may be in a name.

mod names10;

/// \[DESIGN\] The version of XML a text is read under: its XML or text
/// declaration's `VersionNum`, 1.0 if it has none [XML 1.0 §2.8 \[26\]]
/// [XML 1.1 §2.8 \[26\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    /// XML 1.0 Third Edition.
    V10,
    /// XML 1.1.
    V11,
}

/// The `VersionNum` of XML 1.0 [XML 1.0 §2.8 \[26\]].
pub const VERSION_10: &str = "1.0";
/// The `VersionNum` of XML 1.1 [XML 1.1 §2.8 \[26\]].
pub const VERSION_11: &str = "1.1";

impl Version {
    /// The `VersionNum` a declaration writes for it.
    #[must_use]
    pub fn number(self) -> &'static str {
        match self {
            Version::V10 => VERSION_10,
            Version::V11 => VERSION_11,
        }
    }
}

// [XML 1.0 §2.2 [2]] [XML 1.1 §2.2 [2]] The characters a document holds.
/// The tab, which every version allows below the space.
const TAB: char = '\u{9}';
/// The line feed.
pub(super) const LINE_FEED: char = '\u{A}';
/// The carriage return.
pub(super) const CARRIAGE_RETURN: char = '\u{D}';
/// The first character of XML 1.0's range from the space.
const SPACE: char = '\u{20}';
/// The last character before the surrogates.
const BEFORE_SURROGATES: char = '\u{D7FF}';
/// The first character after the surrogates.
const AFTER_SURROGATES: char = '\u{E000}';
/// The last character of the Basic Multilingual Plane a document holds:
/// FFFE and FFFF are excluded.
const LAST_OF_PLANE: char = '\u{FFFD}';
/// The first supplementary character.
const SUPPLEMENTARY: char = '\u{10000}';
/// The first character of XML 1.1's range, which holds the controls but
/// NUL [XML 1.1 §2.2 \[2\]].
const FIRST_11: char = '\u{1}';

// [XML 1.1 §2.2 [2a]] The restricted characters: in the range of `Char`,
// but in a document only by a character reference.
/// The first restricted characters, the controls before the tab.
const RESTRICTED_LOW: (char, char) = ('\u{1}', '\u{8}');
/// The vertical tab and form feed.
const RESTRICTED_VT_FF: (char, char) = ('\u{B}', '\u{C}');
/// The controls after the carriage return.
const RESTRICTED_HIGH: (char, char) = ('\u{E}', '\u{1F}');
/// The delete and the C1 controls before NEL.
const RESTRICTED_DEL: (char, char) = ('\u{7F}', '\u{84}');
/// The C1 controls after NEL.
const RESTRICTED_C1: (char, char) = ('\u{86}', '\u{9F}');

/// Whether `c` matches `Char` of `version` [XML 1.0 §2.2 \[2\]] [XML 1.1
/// §2.2 \[2\]]: a character a document may hold, literally or, of XML 1.1,
/// by a character reference.
#[must_use]
pub fn is_char(version: Version, c: char) -> bool {
    let low = match version {
        Version::V10 => matches!(c, TAB | LINE_FEED | CARRIAGE_RETURN) || (SPACE..=BEFORE_SURROGATES).contains(&c),
        Version::V11 => (FIRST_11..=BEFORE_SURROGATES).contains(&c),
    };
    low || (AFTER_SURROGATES..=LAST_OF_PLANE).contains(&c) || c >= SUPPLEMENTARY
}

/// Whether `c` is a restricted character of XML 1.1 [XML 1.1 §2.2 \[2a\]],
/// which a document holds only by a character reference [XML 1.1 §2.1
/// \[1\]].
#[must_use]
pub fn is_restricted(c: char) -> bool {
    [RESTRICTED_LOW, RESTRICTED_VT_FF, RESTRICTED_HIGH, RESTRICTED_DEL, RESTRICTED_C1].iter().any(|&(lo, hi)| (lo..=hi).contains(&c))
}

/// Whether `c` may be written as it is in text of `version`: a `Char`,
/// and of XML 1.1 not a restricted one.
#[must_use]
pub fn is_literal(version: Version, c: char) -> bool {
    is_char(version, c) && !(version == Version::V11 && is_restricted(c))
}

/// Whether `c` is white space, `S` [XML 1.0 §2.3 \[3\]] [XML 1.1 §2.3 \[3\]]:
/// a space, a tab, a carriage return or a line feed.
#[must_use]
pub fn is_space(c: char) -> bool {
    matches!(c, SPACE | TAB | CARRIAGE_RETURN | LINE_FEED)
}

// [XML 1.1 §2.3 [4]] The characters an XML 1.1 name starts with, beyond
// ':', '_' and the ASCII letters.
/// The ranges of `NameStartChar` above ASCII, as printed.
const NAME_START_11: [(char, char); 12] = [
    ('\u{C0}', '\u{D6}'),
    ('\u{D8}', '\u{F6}'),
    ('\u{F8}', '\u{2FF}'),
    ('\u{370}', '\u{37D}'),
    ('\u{37F}', '\u{1FFF}'),
    ('\u{200C}', '\u{200D}'),
    ('\u{2070}', '\u{218F}'),
    ('\u{2C00}', '\u{2FEF}'),
    ('\u{3001}', '\u{D7FF}'),
    ('\u{F900}', '\u{FDCF}'),
    ('\u{FDF0}', '\u{FFFD}'),
    ('\u{10000}', '\u{EFFFF}'),
];
/// [XML 1.1 §2.3 \[4a\]] The middle dot, a name character.
const MIDDLE_DOT: char = '\u{B7}';
/// [XML 1.1 §2.3 \[4a\]] The combining diacritical marks.
const COMBINING_11: (char, char) = ('\u{300}', '\u{36F}');
/// [XML 1.1 §2.3 \[4a\]] The undertie and character tie.
const TIES_11: (char, char) = ('\u{203F}', '\u{2040}');

/// The colon, which a name may hold but an `NCName` may not.
pub const COLON: char = ':';

/// Whether `c` is in one of `ranges`, sorted by their first character.
fn in_ranges(ranges: &[(char, char)], c: char) -> bool {
    let at = ranges.partition_point(|&(_, hi)| hi < c);
    ranges.get(at).is_some_and(|&(lo, _)| lo <= c)
}

/// Whether `c` is a `Letter` of XML 1.0 [XML 1.0 Appendix B \[84\]].
fn is_letter_10(c: char) -> bool {
    in_ranges(&names10::BASE_CHAR, c) || in_ranges(&names10::IDEOGRAPHIC, c)
}

/// Whether a name of `version` may start with `c` [XML 1.0 §2.3 \[5\]]
/// [XML 1.1 §2.3 \[4\]].
#[must_use]
pub fn is_name_start(version: Version, c: char) -> bool {
    if c == COLON || c == '_' || c.is_ascii_alphabetic() {
        return true;
    }
    match version {
        Version::V10 => is_letter_10(c),
        Version::V11 => in_ranges(&NAME_START_11, c),
    }
}

/// Whether `c` may follow the first character of a name of `version`
/// [XML 1.0 §2.3 \[4\]] [XML 1.1 §2.3 \[4a\]].
#[must_use]
pub fn is_name_char(version: Version, c: char) -> bool {
    if is_name_start(version, c) || c == '-' || c == '.' || c.is_ascii_digit() {
        return true;
    }
    match version {
        Version::V10 => in_ranges(&names10::DIGIT, c) || in_ranges(&names10::COMBINING_CHAR, c) || in_ranges(&names10::EXTENDER, c),
        Version::V11 => c == MIDDLE_DOT || (COMBINING_11.0..=COMBINING_11.1).contains(&c) || (TIES_11.0..=TIES_11.1).contains(&c),
    }
}

/// Whether `text` matches `Name` of `version` [XML 1.0 §2.3 \[5\]] [XML 1.1
/// §2.3 \[5\]].
#[must_use]
pub fn is_name(version: Version, text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| is_name_start(version, c)) && chars.all(|c| is_name_char(version, c))
}

/// Whether `text` matches `NCName` of `version`: a `Name` with no colon
/// [Namespaces 1.0 §2 \[4\]] [Namespaces 1.1 §3 \[4\]].
#[must_use]
pub fn is_ncname(version: Version, text: &str) -> bool {
    is_name(version, text) && !text.contains(COLON)
}

/// Whether `text` matches `Nmtoken` of `version`: one or more name
/// characters [XML 1.0 §2.3 \[7\]] [XML 1.1 §2.3 \[7\]].
#[must_use]
pub fn is_nmtoken(version: Version, text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| is_name_char(version, c))
}

/// [XML 1.0 §2.3 \[13\]] The punctuation of a `PubidChar`.
const PUBID_PUNCTUATION: &str = "-'()+,./:=?;!*#@$_%";

/// Whether `c` matches `PubidChar` [XML 1.0 §2.3 \[13\]] [XML 1.1 §2.3
/// \[13\]].
#[must_use]
pub fn is_pubid_char(c: char) -> bool {
    matches!(c, SPACE | CARRIAGE_RETURN | LINE_FEED) || c.is_ascii_alphanumeric() || PUBID_PUNCTUATION.contains(c)
}

#[cfg(test)]
mod tests;
