//! Tests of the mapping of SQL identifiers to XML Names and back.

use super::*;

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.1 GR4e, GR4f] an identifier
/// maps to an XML Name: a character no XML 1.1 name holds, or none begins
/// with, escaped by its code, four hexits or six; any other as it is.
#[test]
fn identifiers_map_to_names_9_1() {
    let partial = |s: &str| name_of(s, Escape::Partial);
    assert_eq!(partial("a b"), "a_x0020_b");
    assert_eq!(partial("1a"), "_x0031_a");
    assert_eq!(partial("a1-é"), "a1-é");
    assert_eq!(partial("a\u{F0000}"), "a_x0F0000_");
    assert_eq!(partial("\u{10000}"), "\u{10000}");
    assert_eq!(partial(""), "");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.1 GR4b to GR4d] a colon is
/// escaped first or fully escaped; an underscore before a lowercase x
/// escaped; a leading xml's first letter escaped if fully escaped.
#[test]
fn escape_variants_9_1() {
    let full = |s: &str| name_of(s, Escape::Full);
    let partial = |s: &str| name_of(s, Escape::Partial);
    assert_eq!(partial(":a"), "_x003A_a");
    assert_eq!(partial("a:b"), "a:b");
    assert_eq!(full("a:b"), "a_x003A_b");
    assert_eq!(partial("_xa"), "_x005F_xa");
    assert_eq!(partial("_Xa_"), "_Xa_");
    assert_eq!(full("xmlA"), "_x0078_mlA");
    assert_eq!(full("XmLa"), "_x0058_mLa");
    assert_eq!(partial("xmlA"), "xmlA");
    assert_eq!(full("xm"), "xm");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.2] a multi-part name maps to
/// one XML Name: each part fully escaped, its periods escaped, joined by
/// periods.
#[test]
fn multi_part_names_9_2() {
    assert_eq!(name_of_parts(&["S", "T.U", "a:b"]), "S.T_x002E_U.a_x003A_b");
    assert_eq!(name_of_parts(&["xml"]), "_x0078_ml");
    assert_eq!(name_of_parts(&[]), "");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.3 GR2] [DESIGN] an XML Name
/// maps back to an identifier: each escape of four or six hexits, of
/// either case, of an assigned character read as it; one of no assigned
/// character, or not an escape, kept as written (implementation-defined
/// GR2a ii, GR2b ii); each identifier read back as it was mapped.
#[test]
fn names_map_back_to_identifiers_9_3() {
    assert_eq!(identifier_of("a_x0020_b"), "a b");
    assert_eq!(identifier_of("_x003a_a"), ":a");
    assert_eq!(identifier_of("a_x0F0000_"), "a\u{F0000}");
    assert_eq!(identifier_of("_x0378_"), "_x0378_", "unassigned");
    assert_eq!(identifier_of("_xD800_"), "_xD800_", "a surrogate");
    assert_eq!(identifier_of("_x00zz_ _x41 _x0041"), "_x00zz_ _x41 _x0041");
    for identifier in ["my column", ":x", "_x", "xml1", "1 2:3", "a.b", "é\u{1F600}", "_x0041_"] {
        assert_eq!(identifier_of(&name_of(identifier, Escape::Full)), identifier, "{identifier}");
        assert_eq!(identifier_of(&name_of(identifier, Escape::Partial)), identifier, "{identifier}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.3 GR5] [ISO/IEC 9075:1992
/// §5.2] the identifier an XML Name maps to is the delimited identifier
/// of it: in double quotes, each double quote it holds doubled.
#[test]
fn the_mapping_is_a_delimited_identifier_9_3() {
    assert_eq!(delimited(&identifier_of("_x0020_a")), "\" a\"");
    assert_eq!(delimited("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(delimited(""), "\"\"");
}
