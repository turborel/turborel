//! IS VALID [ISO/IEC 9075-14:2006 draft §8.5]: whether an XML value is
//! valid according to a registered XML Schema, True, False or Unknown,
//! in a query, a simple CASE and a CHECK.

use crate::common::rows;
use crate::xml_validate::{one, refused, registered};

/// `value` IS `rest`, as True, False or Unknown.
fn is(s: &turborel::db::Session, value: &str, rest: &str) -> String {
    one(s, &format!("CASE WHEN {value} IS {rest} THEN 'T' WHEN NOT ({value} IS {rest}) THEN 'F' ELSE 'U' END"))
}

/// The text `text` parsed as `syntax`.
fn parsed(syntax: &str, text: &str) -> String {
    format!("XMLPARSE({syntax} '{text}' PRESERVE WHITESPACE)")
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §8.5 GR2 to GR8] IS VALID is
/// Unknown of the null value; False of a value not of the shape DCS asks;
/// True of an empty sequence of SEQUENCE; True if each element is valid,
/// False if one is not or is of another name or namespace than the
/// element clause's, by the registered XML Schema indicated.
#[test]
fn is_valid_according_to_a_schema_8_5() {
    let db = registered();
    let a = db.session("A");
    let n = parsed("DOCUMENT", "<n>1</n>");
    let p = "ACCORDING TO XMLSCHEMA ID A.P";
    assert_eq!(is(&a, &n, &format!("VALID DOCUMENT {p}")), "T");
    assert_eq!(is(&a, &n, &format!("VALID DOCUMENT {p} ELEMENT \"n\"")), "T");
    assert_eq!(is(&a, &n, &format!("VALID DOCUMENT {p} ELEMENT \"n\"")), "T");
    assert_eq!(is(&a, &parsed("DOCUMENT", "<n>x</n>"), &format!("VALID DOCUMENT {p}")), "F", "not valid");
    assert_eq!(is(&a, &parsed("CONTENT", "<n>1</n><n>2</n>"), &format!("VALID DOCUMENT {p}")), "F", "no document");
    assert_eq!(is(&a, &parsed("CONTENT", "<n>1</n><n>2</n>"), &format!("VALID CONTENT {p}")), "T");
    assert_eq!(is(&a, &parsed("CONTENT", "<n>1</n>t"), &format!("VALID CONTENT {p}")), "F", "a text child");
    assert_eq!(is(&a, "XMLQUERY('1' RETURNING SEQUENCE BY REF EMPTY ON EMPTY)", &format!("VALID SEQUENCE {p}")), "F", "atomic");
    assert_eq!(is(&a, "XMLQUERY('()' RETURNING SEQUENCE BY REF EMPTY ON EMPTY)", &format!("VALID SEQUENCE {p}")), "T", "empty");
    let order = parsed("DOCUMENT", r#"<order xmlns="urn:o"><qty>2</qty></order>"#);
    assert_eq!(is(&a, &order, "VALID DOCUMENT ACCORDING TO XMLSCHEMA URI 'urn:o' ELEMENT \"note\""), "F", "another QName");
    assert_eq!(is(&a, &order, "VALID DOCUMENT ACCORDING TO XMLSCHEMA URI 'urn:o' ELEMENT \"order\""), "T");
    let both = format!("XMLCONCAT({n}, {order} RETURNING SEQUENCE)");
    assert_eq!(is(&a, &both, "VALID SEQUENCE ACCORDING TO XMLSCHEMA URI 'urn:o' NAMESPACE 'urn:o'"), "F", "another namespace");
    assert_eq!(is(&a, "CAST(NULL AS XML)", &format!("VALID SEQUENCE {p}")), "U");
    assert_eq!(is(&a, &n, &format!("NOT VALID DOCUMENT {p}")), "F", "NOT");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §8.5 GR7a iii 3 A II, GR7b i 2]
/// implementation-defined: of no `<XML valid according to clause>`, an
/// element whose namespace has no registered XML Schema the user may use
/// makes IS VALID Unknown, unless another is not valid, which makes it
/// False.
#[test]
fn data_driven_is_valid_is_unknown_without_a_schema_8_5() {
    let db = registered();
    let a = db.session("A");
    assert_eq!(is(&a, &parsed("CONTENT", r#"<n>1</n><note xmlns="urn:o">x</note>"#), "VALID CONTENT"), "T");
    assert_eq!(is(&a, &parsed("CONTENT", r#"<n>1</n><x xmlns="urn:x"/>"#), "VALID CONTENT"), "U");
    assert_eq!(is(&a, &parsed("CONTENT", r#"<n>z</n><x xmlns="urn:x"/>"#), "VALID CONTENT"), "F");
    let b = db.session("B");
    assert_eq!(is(&b, &parsed("DOCUMENT", "<n>1</n>"), "VALID DOCUMENT"), "U", "no schema B may use");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §8.5 SR1, §6.4] IS VALID takes an
/// XML value alone, else SQLCODE -103; it is a `<when operand>` of a
/// simple CASE.
#[test]
fn is_valid_operands_8_5() {
    let db = registered();
    let a = db.session("A");
    assert_eq!(refused(&a, "VALUES (CASE WHEN 1 IS VALID DOCUMENT THEN 1 END)").0, -103);
    let case =
        format!("CASE {} WHEN IS VALID DOCUMENT ACCORDING TO XMLSCHEMA ID A.P THEN 'yes' ELSE 'no' END", parsed("DOCUMENT", "<n>1</n>"));
    assert_eq!(one(&a, &case), "yes");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §8.5, §4.2.6] a CHECK (X IS
/// VALID ...) of a column of an UNTYPED type takes the values valid
/// according to the registered XML Schema it names, and the null value,
/// and refuses another, SQLSTATE 23000.
#[test]
fn a_check_of_is_valid_8_5() {
    let db = registered();
    let a = db.session("A");
    a.run("CREATE TABLE T (X XML(DOCUMENT(UNTYPED)) CHECK (X IS VALID DOCUMENT ACCORDING TO XMLSCHEMA ID A.P ELEMENT \"n\"))").unwrap();
    a.run(&format!("INSERT INTO T VALUES ({})", parsed("DOCUMENT", "<n>5</n>"))).unwrap();
    a.run("INSERT INTO T VALUES (NULL)").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(&a, &format!("INSERT INTO T VALUES ({})", parsed("DOCUMENT", "<n>five</n>"))).1, "23000");
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T").unwrap(), ["2"]);
}
