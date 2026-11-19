//! Feature X400, "Name and identifier mapping" [ISO/IEC 9075-14:2006 draft
//! §9.1, §9.3, Annex A]: \[EXTENSION\] XMLNAME_TO_IDENTIFIER and
//! IDENTIFIER_TO_XMLNAME, by which an SQL statement invokes the mappings
//! (RULES.md D106).

use turborel::db::Database;

use crate::xml_validate::{one, refused, registered};

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.3 GR2, GR5]
/// [ISO/IEC 9075:1992 §5.2] XMLNAME_TO_IDENTIFIER maps an XML Name to the
/// delimited identifier it stands for: each escape of four or six hexits
/// of an assigned character read as it, one of none kept, the identifier
/// in double quotes, each double quote it holds doubled; null to null.
#[test]
fn xml_names_map_to_identifiers_9_3() {
    let db = registered();
    let s = db.session("A");
    let cases = [
        ("'_x0020_a'", "\" a\""),
        ("'my_x0020_column'", "\"my column\""),
        ("'_x0022_q_x0022_'", "\"\"\"q\"\"\""),
        ("'a_x0F0000_'", "\"a\u{F0000}\""),
        ("'_x0378_'", "\"_x0378_\""),
        ("'plain'", "\"plain\""),
    ];
    for (name, expected) in cases {
        assert_eq!(one(&s, &format!("XMLNAME_TO_IDENTIFIER({name})")), expected, "{name}");
    }
    assert_eq!(one(&s, "XMLNAME_TO_IDENTIFIER(CAST(NULL AS VARCHAR(5)))"), "NULL");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.1 GR4] [ISO/IEC
/// 9075-14:2006 draft §4.10.3] IDENTIFIER_TO_XMLNAME maps an identifier's
/// characters to an XML Name, partially escaped unless FULLY ESCAPED is
/// written: a colon kept after the first character only partially, a
/// leading `xml` escaped only fully, a character no name holds escaped by
/// either; the two mappings read back to the identifier.
#[test]
fn identifiers_map_to_xml_names_9_1() {
    let db = registered();
    let s = db.session("A");
    let cases = [
        ("IDENTIFIER_TO_XMLNAME('a:b c')", "a:b_x0020_c"),
        ("IDENTIFIER_TO_XMLNAME('a:b c', PARTIALLY ESCAPED)", "a:b_x0020_c"),
        ("IDENTIFIER_TO_XMLNAME('a:b c', FULLY ESCAPED)", "a_x003A_b_x0020_c"),
        ("IDENTIFIER_TO_XMLNAME('xmlns')", "xmlns"),
        ("IDENTIFIER_TO_XMLNAME('xmlns', fully escaped)", "_x0078_mlns"),
        ("IDENTIFIER_TO_XMLNAME('1st')", "_x0031_st"),
        ("XMLNAME_TO_IDENTIFIER(IDENTIFIER_TO_XMLNAME('my \"col\"', FULLY ESCAPED))", "\"my \"\"col\"\"\""),
    ];
    for (expression, expected) in cases {
        assert_eq!(one(&s, expression), expected, "{expression}");
    }
}

/// Verifies: [EXTENSION] RULES.md D106: the mappings take a character
/// string, a dynamic parameter as one; another argument is refused as the
/// statement is bound, SQLCODE -103; a variant neither PARTIALLY nor
/// FULLY, or one with no ESCAPED, is a syntax error, SQLCODE -102; the
/// words are no key words, so a column may have them as its name.
#[test]
fn what_the_mappings_take() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    assert_eq!(refused(&s, "VALUES (IDENTIFIER_TO_XMLNAME(1))").0, -103);
    assert_eq!(refused(&s, "VALUES (IDENTIFIER_TO_XMLNAME('a', HALF ESCAPED))").0, -102);
    assert_eq!(refused(&s, "VALUES (IDENTIFIER_TO_XMLNAME('a', FULLY))").0, -102);
    let prepared = s.prepare("VALUES (XMLNAME_TO_IDENTIFIER(?))").unwrap();
    assert_eq!(prepared.parameters().len(), 1);
    s.run("CREATE TABLE ESCAPED (FULLY VARCHAR(5), PARTIALLY INTEGER)").unwrap();
    s.run("INSERT INTO ESCAPED VALUES ('a b', 1)").unwrap();
    assert_eq!(one(&s, "(SELECT IDENTIFIER_TO_XMLNAME(FULLY, FULLY ESCAPED) FROM ESCAPED)"), "a_x0020_b");
}
