//! More tests of assessment: mixed content, lax and skipped attributes,
//! types stipulated by `xsi:type` alone, identity constraints' paths, and
//! non-deterministic declarations.

use super::*;
use crate::xml::{Builder, ElementStart, QName};

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.4.4 cvc-complex-type.2.3, 2.4] [Schema1 §3.3.4
/// cvc-elt.5.2.2.2.1] [Schema1 §3.3.5] mixed content takes text among its
/// elements; an empty mixed element of a default takes it; a fixed one's
/// text must be its value.
#[test]
fn mixed_content_and_its_value_constraints() {
    let body = r#"<xs:complexType name="m" mixed="true"><xs:sequence><xs:element name="b" minOccurs="0"/></xs:sequence></xs:complexType>
        <xs:element name="r"><xs:complexType><xs:sequence>
        <xs:element name="p" type="m"/><xs:element name="d" type="m" default="dv"/><xs:element name="f" type="m" fixed="fv" minOccurs="0"/>
        </xs:sequence></xs:complexType></xs:element>"#;
    let out = valid(body, "<r><p>a<b/>c</p><d/><f>fv</f></r>");
    let tree = out.tree();
    assert_eq!(tree.string_value(element(tree, "p")), "ac");
    assert_eq!(tree.string_value(element(tree, "d")), "dv");
    assert_eq!(tree.annotation(element(tree, "p")).map(|a| a.typing), Some(Typing::Mixed));
    assert!(broken(body, "<r><p/><d/><f>other</f></r>").contains("cvc-elt.5.2.2.2.1"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.10.4] [Schema1 §3.2.4 cvc-assess-attr] attribute
/// wildcards: strict needs a top-level declaration, lax validates by one
/// if there is one and keeps others untyped, skip keeps every one as it
/// is; a namespace not allowed is refused.
#[test]
fn attribute_wildcards_process_contents() {
    let body = |process: &str| {
        format!(
            r###"<xs:attribute name="n" type="xs:int"/><xs:element name="r"><xs:complexType>
            <xs:anyAttribute processContents="{process}" namespace="##local"/></xs:complexType></xs:element>"###
        )
    };
    assert!(broken(&body("strict"), r#"<r m="1"/>"#).contains("cvc-assess-attr"));
    valid(&body("strict"), r#"<r n="1"/>"#);
    assert!(broken(&body("lax"), r#"<r n="x"/>"#).contains("cvc-attribute.3"));
    let out = valid(&body("lax"), r#"<r n=" 1 " m="x"/>"#);
    let tree = out.tree();
    let r = element(tree, "r");
    assert_eq!(type_of(tree, attribute(tree, r, "n").unwrap()), "int");
    assert_eq!(tree.content(attribute(tree, r, "n").unwrap()), "1");
    assert_eq!(type_of(tree, attribute(tree, r, "m").unwrap()), "untyped");
    valid(&body("skip"), r#"<r n="x"/>"#);
    assert!(broken(&body("skip"), r#"<r xmlns:p="urn:p" p:n="x"/>"#).contains("cvc-complex-type.3.2.2"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.3.4 cvc-assess-elt.1.2] [Schema1 §5.2 item 3] an
/// element with no declaration but an `xsi:type`, validated laxly, is
/// validated by that type.
#[test]
fn xsi_type_alone_stipulates_a_type() {
    let s = schema(r#"<xs:element name="n" type="xs:int"/>"#).unwrap();
    let xsi = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;
    let d = document(
        &format!(r#"<x {xsi} xsi:type="xs:date">2026-11-17</x>"#)
            .replace("xsi:type", r#"xmlns:xs="http://www.w3.org/2001/XMLSchema" xsi:type"#),
    );
    let out = validate(&s, &d, Mode::Lax).unwrap().unwrap();
    assert_eq!(type_of(out.tree(), element(out.tree(), "x")), "date");
    let bad = document(&format!(r#"<x {xsi} xmlns:xs="http://www.w3.org/2001/XMLSchema" xsi:type="xs:date">soon</x>"#));
    assert!(validate(&s, &bad, Mode::Lax).unwrap().is_err());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.11.4 cvc-identity-constraint.2, 3, 4.1] a unique
/// constraint over descendants; a selector of the element itself, a field
/// of two nodes, and a field of element-only content, refused.
#[test]
fn identity_constraint_paths() {
    let body = |selector: &str, field: &str| {
        format!(
            r#"<xs:element name="r"><xs:complexType><xs:sequence>
            <xs:element name="g" maxOccurs="unbounded"><xs:complexType><xs:sequence>
              <xs:element name="i" minOccurs="0" maxOccurs="unbounded"><xs:complexType><xs:sequence><xs:element name="k" type="xs:string" minOccurs="0" maxOccurs="2"/></xs:sequence></xs:complexType></xs:element>
            </xs:sequence></xs:complexType></xs:element></xs:sequence></xs:complexType>
            <xs:unique name="u"><xs:selector xpath="{selector}"/><xs:field xpath="{field}"/></xs:unique></xs:element>"#
        )
    };
    let text = "<r><g><i><k>a</k></i><i><k>b</k></i></g><g><i/></g></r>";
    valid(&body(".//i", "k"), text);
    assert!(broken(&body(".//i", "k"), "<r><g><i><k>a</k></i></g><g><i><k>a</k></i></g></r>").contains("cvc-identity-constraint.4.1"));
    assert!(broken(&body(".", "g/i/k"), text).contains("cvc-identity-constraint.2"));
    assert!(broken(&body("g/i", "k"), "<r><g><i><k>a</k><k>b</k></i></g></r>").contains("cvc-identity-constraint.3"));
    assert!(broken(&body("g", "i"), "<r><g><i/></g></r>").contains("cvc-identity-constraint.3"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [XQuery §3.13] a document node whose children are not one
/// element, comments and processing instructions is err:XQDY0061.
#[test]
fn documents_validated_have_one_element() {
    let mut b = Builder::document();
    for name in ["a", "b"] {
        b.start_element(ElementStart { name: QName::local(name), bindings: Vec::new(), attributes: Vec::new() }).unwrap();
        b.end_element().unwrap();
    }
    let two = Node::root(b.finish().unwrap());
    let s = schema(r#"<xs:element name="a"/>"#).unwrap();
    assert_eq!(validate(&s, &two, Mode::Lax).unwrap().unwrap_err().code, NOT_A_DOCUMENT);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.20 GR6b iv, §23.1] a failure
/// is raised by XMLVALIDATE as data exception, validation failure,
/// SQLSTATE 2201R, naming the rule broken.
#[test]
fn failures_are_validation_failures() {
    let f = Failure::new("[Schema1 §3.4.4 cvc-complex-type.2.4]", "element x is not expected here");
    let e = f.validation_failure();
    assert_eq!(e.sqlstate().as_str(), "2201R");
    assert!(e.to_string().contains("cvc-complex-type.2.4"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.6] a global element
/// declaration is non-deterministic if it holds or refers to a wildcard of
/// any namespace or of not one, strict or lax, as an `<xs:any>` or
/// `<xs:anyAttribute>`; of a set of namespaces, or skip, it is not; and a
/// namespace or schema of one non-deterministic declaration is too.
#[test]
fn non_deterministic_declarations() {
    let s = schema(
        r###"<xs:complexType name="open"><xs:sequence><xs:any namespace="##other"/></xs:sequence></xs:complexType>
        <xs:element name="a" type="open"/>
        <xs:element name="b"><xs:complexType><xs:sequence><xs:element ref="a"/></xs:sequence></xs:complexType></xs:element>
        <xs:element name="c"><xs:complexType><xs:anyAttribute/></xs:complexType></xs:element>
        <xs:element name="d"><xs:complexType><xs:sequence><xs:any namespace="urn:x" processContents="lax"/><xs:any processContents="skip"/></xs:sequence></xs:complexType></xs:element>
        <xs:element name="e" type="xs:int"/>"###,
    )
    .unwrap();
    let decl = |n: &str| s.element_names[&Expanded::new(None, n)];
    assert!(s.non_deterministic(decl("a")) && s.non_deterministic(decl("b")) && s.non_deterministic(decl("c")));
    assert!(!s.non_deterministic(decl("d")) && !s.non_deterministic(decl("e")));
    assert!(s.namespace_non_deterministic(None) && !s.namespace_non_deterministic(Some("urn:x")));
    assert!(s.is_non_deterministic());
}
