//! Tests of the characters of XML text, by version.

use super::*;

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR2, GR3] [XML 1.0 §2.2
/// \[2\]] [XML 1.1 §2.2 \[2\]] XML 1.0 holds no control but tab, line feed
/// and carriage return; XML 1.1 holds every control but NUL; neither
/// holds FFFE or FFFF, and both hold the supplementary planes.
#[test]
fn the_characters_of_each_version() {
    for version in [Version::V10, Version::V11] {
        assert!(is_char(version, '\t') && is_char(version, '\n') && is_char(version, '\r'));
        assert!(is_char(version, 'a') && is_char(version, '\u{D7FF}') && is_char(version, '\u{E000}'));
        assert!(is_char(version, '\u{FFFD}') && is_char(version, '\u{10000}') && is_char(version, '\u{10FFFF}'));
        assert!(!is_char(version, '\0') && !is_char(version, '\u{FFFE}') && !is_char(version, '\u{FFFF}'));
    }
    assert!(!is_char(Version::V10, '\u{1}') && !is_char(Version::V10, '\u{1F}'));
    assert!(is_char(Version::V11, '\u{1}') && is_char(Version::V11, '\u{1F}'));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR3] [XML 1.1 §2.2 \[2a\]]
/// the restricted characters of XML 1.1 are written only as references,
/// and NEL, a line end, is none of them.
#[test]
fn restricted_characters_are_written_only_as_references() {
    for c in ['\u{1}', '\u{8}', '\u{B}', '\u{C}', '\u{E}', '\u{1F}', '\u{7F}', '\u{84}', '\u{86}', '\u{9F}'] {
        assert!(is_restricted(c), "{c:?}");
        assert!(!is_literal(Version::V11, c), "{c:?}");
    }
    for c in ['\t', '\n', '\r', ' ', '\u{85}', '\u{A0}'] {
        assert!(!is_restricted(c), "{c:?}");
        assert!(is_literal(Version::V11, c), "{c:?}");
    }
    assert!(is_literal(Version::V10, '\u{7F}'));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR2] [XML 1.0 §2.3 \[3\]]
/// white space is a space, a tab, a carriage return or a line feed.
#[test]
fn white_space_is_four_characters() {
    assert!([' ', '\t', '\r', '\n'].into_iter().all(is_space));
    assert!(!['\u{A0}', '\u{85}', '\u{2028}', 'a'].into_iter().any(is_space));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR2] [XML 1.0 §2.3 \[4\],
/// \[5\], Appendix B \[84\] to \[89\]] an XML 1.0 name starts with a letter,
/// '_' or ':', a letter being a base character or an ideographic one,
/// and goes on with digits, combining characters and extenders too.
#[test]
fn names_of_xml_1_0() {
    let v = Version::V10;
    for c in ['A', 'z', '_', ':', '\u{E9}', '\u{0E01}', '\u{4E00}', '\u{9FA5}', '\u{3007}', '\u{3021}', '\u{AC00}'] {
        assert!(is_name_start(v, c), "{c:?}");
    }
    for c in ['0', '-', '.', '\u{0300}', '\u{0660}', '\u{B7}', '\u{3005}'] {
        assert!(!is_name_start(v, c) && is_name_char(v, c), "{c:?}");
    }
    for c in ['\u{0221}', '\u{9FA6}', '\u{3008}', ' ', '\u{D7}', '\u{10000}'] {
        assert!(!is_name_char(v, c), "{c:?}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR3] [XML 1.1 §2.3 \[4\],
/// \[4a\]] an XML 1.1 name is made of open ranges, so it holds characters
/// XML 1.0's tables leave out, and not those the ranges exclude.
#[test]
fn names_of_xml_1_1() {
    let v = Version::V11;
    for c in ['A', '_', ':', '\u{0221}', '\u{9FA6}', '\u{10000}', '\u{EFFFF}', '\u{200C}'] {
        assert!(is_name_start(v, c), "{c:?}");
    }
    for c in ['0', '-', '.', '\u{B7}', '\u{0300}', '\u{036F}', '\u{203F}', '\u{2040}'] {
        assert!(!is_name_start(v, c) && is_name_char(v, c), "{c:?}");
    }
    for c in ['\u{D7}', '\u{F7}', '\u{37E}', '\u{2000}', '\u{F0000}', ' '] {
        assert!(!is_name_char(v, c), "{c:?}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR2] [XML 1.0 Appendix B]
/// every table of characters is in order of code point, its ranges
/// apart, so a search finds each.
#[test]
fn the_tables_are_sorted_and_apart() {
    let tables: [&[(char, char)]; 6] =
        [&names10::BASE_CHAR, &names10::IDEOGRAPHIC, &names10::COMBINING_CHAR, &names10::DIGIT, &names10::EXTENDER, &NAME_START_11];
    for table in tables {
        assert!(table.iter().all(|(lo, hi)| lo <= hi));
        assert!(table.windows(2).all(|w| w[0].1 < w[1].0));
        for &(lo, hi) in table {
            assert!(in_ranges(table, lo) && in_ranges(table, hi));
        }
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR2] [XML 1.0 §2.3 \[5\],
/// \[7\], \[13\]] [Namespaces 1.0 §2 \[4\]] a name, a name with no colon, a
/// name token, and the characters of a public identifier.
#[test]
fn names_tokens_and_public_identifiers() {
    let v = Version::V10;
    assert!(is_name(v, "a:b") && is_name(v, "_x.1") && !is_name(v, "1a") && !is_name(v, ""));
    assert!(is_ncname(v, "a") && !is_ncname(v, "a:b"));
    assert!(is_nmtoken(v, "1a") && is_nmtoken(v, "-") && !is_nmtoken(v, "") && !is_nmtoken(v, "a b"));
    assert!("aZ09 -'()+,./:=?;!*#@$_%\r\n".chars().all(is_pubid_char));
    assert!(!['"', '&', '<', '>', '\t', '\u{E9}'].into_iter().any(is_pubid_char));
    assert_eq!((Version::V10.number(), Version::V11.number()), ("1.0", "1.1"));
}
