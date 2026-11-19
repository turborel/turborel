//! Tests of complex type definitions' representation constraints
//! [Schema1 §3.4.3], their attribute wildcards [Schema1 §3.4.2], the
//! redefinition of groups and attribute groups [Schema1 §4.2.2], and
//! identity-constraint definitions [Schema1 §3.11].

use super::super::super::component::NamespaceConstraint;
use super::*;

/// The schema of `redefining`, a document of no target namespace
/// redefining `v1.xsd`, of `original`.
fn redefined(original: &str, redefining: &str) -> Result<Schema> {
    let v1 = source(&format!("<xs:schema {XS}>{original}</xs:schema>"), Some("v1.xsd"));
    let v2 = source(&format!(r#"<xs:schema {XS}><xs:redefine schemaLocation="v1.xsd">{redefining}</xs:redefine></xs:schema>"#), None);
    read(&v2, &Library::new(vec![v1]))
}

/// Verifies: [EXTENSION] [Schema1 §3.4.3 src-ct.1, src-ct.2] [Schema1
/// §3.4.6 cos-ct-extends.1.4] complex content does not derive from a
/// simple type; simple content derives from a complex type of simple
/// content, or extends a simple type, and does not restrict one; a type
/// of simple content is not extended by a content model.
#[test]
fn derivations_of_the_wrong_content_are_refused() {
    let price = r#"<xs:complexType name="p"><xs:simpleContent><xs:extension base="xs:decimal"/></xs:simpleContent></xs:complexType>"#;
    for (body, rule) in [
        (r#"<xs:complexType name="c"><xs:complexContent><xs:restriction base="xs:int"/></xs:complexContent></xs:complexType>"#, "src-ct.1"),
        (r#"<xs:complexType name="c"><xs:simpleContent><xs:restriction base="xs:int"/></xs:simpleContent></xs:complexType>"#, "src-ct.2"),
        (
            r#"<xs:complexType name="e"/><xs:complexType name="c"><xs:simpleContent><xs:extension base="e"/></xs:simpleContent></xs:complexType>"#,
            "src-ct.2",
        ),
        (
            &format!(
                r#"{price}<xs:complexType name="c"><xs:complexContent><xs:extension base="p"><xs:sequence><xs:element name="a"/>
                </xs:sequence></xs:extension></xs:complexContent></xs:complexType>"#
            ),
            "cos-ct-extends.1.4",
        ),
    ] {
        assert!(refused(body).contains(rule), "{rule}: {body}");
    }
}

/// Verifies: [EXTENSION] [Schema1 §3.4.2 complex content 1.1] an empty
/// `<choice>` of minOccurs 0 is no content, so the type is empty; one of
/// minOccurs 1 is a content model, which nothing satisfies.
#[test]
fn an_empty_choice_is_empty_content_only_if_optional() {
    let s = schema(
        r#"<xs:complexType name="o"><xs:choice minOccurs="0"/></xs:complexType><xs:complexType name="m"><xs:choice/></xs:complexType>"#,
    )
    .unwrap();
    assert_eq!(complex(&s, "o").content, ContentType::Empty);
    assert!(matches!(complex(&s, "m").content, ContentType::ElementOnly(_)));
}

/// Verifies: [EXTENSION] [Schema1 §3.4.2 {attribute wildcard}] an
/// extension's attribute wildcard is the union of its own and its base's,
/// or its base's if it has none; a type's complete wildcard is the
/// intersection of its own and its attribute groups', or the first
/// group's if it has none.
#[test]
fn attribute_wildcards_combine_by_union_and_intersection() {
    let s = schema(
        r#"<xs:attributeGroup name="g"><xs:anyAttribute namespace="urn:a urn:b" processContents="skip"/></xs:attributeGroup>
        <xs:complexType name="b"><xs:anyAttribute namespace="urn:a"/></xs:complexType>
        <xs:complexType name="u"><xs:complexContent><xs:extension base="b"><xs:anyAttribute namespace="urn:b"/></xs:extension></xs:complexContent></xs:complexType>
        <xs:complexType name="k"><xs:complexContent><xs:extension base="b"/></xs:complexContent></xs:complexType>
        <xs:complexType name="i"><xs:attributeGroup ref="g"/><xs:anyAttribute namespace="urn:b urn:c"/></xs:complexType>
        <xs:complexType name="f"><xs:attributeGroup ref="g"/></xs:complexType>"#,
    )
    .unwrap();
    let allows = |t: &str, ns: &str| complex(&s, t).wildcard.as_ref().is_some_and(|w| w.constraint.allows(Some(ns)));
    assert!(allows("u", "urn:a") && allows("u", "urn:b") && !allows("u", "urn:c"));
    assert!(allows("k", "urn:a") && !allows("k", "urn:b"));
    assert!(allows("i", "urn:b") && !allows("i", "urn:a") && !allows("i", "urn:c"));
    assert_eq!(complex(&s, "i").wildcard.as_ref().map(|w| w.process), Some(Process::Strict));
    assert_eq!(complex(&s, "f").wildcard.as_ref().map(|w| w.process), Some(Process::Skip));
    assert!(matches!(complex(&s, "f").wildcard.as_ref().map(|w| &w.constraint), Some(NamespaceConstraint::Set(_))));
}

/// Verifies: [EXTENSION] [Schema1 §4.2.2 src-redefine.7] a redefined
/// attribute group referring to itself once reads, twice is refused
/// \[7.1\]; one referring to itself never restricts the one it redefines
/// \[7.2.2\].
#[test]
fn redefined_attribute_groups_are_checked() {
    let original = r#"<xs:attributeGroup name="g"><xs:attribute name="a" type="xs:int"/></xs:attributeGroup>"#;
    redefined(original, r#"<xs:attributeGroup name="g"><xs:attributeGroup ref="g"/><xs:attribute name="b"/></xs:attributeGroup>"#).unwrap();
    redefined(original, r#"<xs:attributeGroup name="g"><xs:attribute name="a" type="xs:short"/></xs:attributeGroup>"#).unwrap();
    for (redefining, rule) in [
        (r#"<xs:attributeGroup name="g"><xs:attributeGroup ref="g"/><xs:attributeGroup ref="g"/></xs:attributeGroup>"#, "src-redefine.7.1"),
        (r#"<xs:attributeGroup name="g"><xs:attribute name="a" type="xs:string"/></xs:attributeGroup>"#, "src-redefine.7.2.2"),
    ] {
        let e = redefined(original, redefining).unwrap_err();
        assert_eq!(e.sqlstate(), SqlState::DATA_EXCEPTION, "{e}");
        assert!(e.to_string().contains(rule), "{rule}: {e}");
    }
}

/// Verifies: [EXTENSION] [Schema1 §4.2.2 src-redefine.5, 6] a redefined
/// type derives from the type it redefines; a redefined group referring
/// to itself twice is refused \[6.1.1\], and one referring to itself never
/// restricts the one it redefines \[6.2.2\].
#[test]
fn redefined_types_and_groups_are_checked() {
    let original = r#"<xs:complexType name="t"/><xs:complexType name="o"/>
        <xs:group name="g"><xs:sequence><xs:element name="a"/><xs:element name="b" minOccurs="0"/></xs:sequence></xs:group>"#;
    redefined(original, r#"<xs:group name="g"><xs:sequence><xs:element name="a"/></xs:sequence></xs:group>"#).unwrap();
    for (redefining, rule) in [
        (r#"<xs:complexType name="t"><xs:complexContent><xs:extension base="o"/></xs:complexContent></xs:complexType>"#, "src-redefine.5"),
        (r#"<xs:group name="g"><xs:sequence><xs:group ref="g"/><xs:group ref="g"/></xs:sequence></xs:group>"#, "src-redefine.6.1.1"),
        (r#"<xs:group name="g"><xs:sequence><xs:element name="c"/></xs:sequence></xs:group>"#, "src-redefine.6.2.2"),
    ] {
        let e = redefined(original, redefining).unwrap_err();
        assert!(e.to_string().contains(rule), "{rule}: {e}");
    }
}

/// Verifies: [EXTENSION] [Schema1 §3.11.2, §3.11.6 c-props-correct.2] an
/// identity-constraint definition has one `<selector>` and a `<field>`, a
/// keyref a refer naming a key or unique of as many fields; each broken
/// is refused, SQLSTATE 22000.
#[test]
fn identity_constraint_representations_are_checked() {
    let element = |c: &str| format!(r#"<xs:element name="e">{c}</xs:element>"#);
    let key = r#"<xs:key name="k"><xs:selector xpath="x"/><xs:field xpath="@a"/></xs:key>"#;
    schema(&element(&format!(r#"{key}<xs:keyref name="r" refer="k"><xs:selector xpath="y"/><xs:field xpath="@b"/></xs:keyref>"#))).unwrap();
    for (constraints, rule) in [
        (r#"<xs:unique name="u"><xs:field xpath="@a"/></xs:unique>"#.to_owned(), "has one <selector>"),
        (
            r#"<xs:unique name="u"><xs:selector xpath="x"/><xs:selector xpath="y"/><xs:field xpath="@a"/></xs:unique>"#.to_owned(),
            "has one <selector>",
        ),
        (r#"<xs:unique name="u"><xs:selector xpath="x"/></xs:unique>"#.to_owned(), "has a <field>"),
        (format!(r#"{key}<xs:keyref name="r"><xs:selector xpath="y"/><xs:field xpath="@b"/></xs:keyref>"#), "has a refer"),
        (
            format!(
                r#"{key}<xs:keyref name="r" refer="k"><xs:selector xpath="y"/><xs:field xpath="@b"/><xs:field xpath="@c"/></xs:keyref>"#
            ),
            "c-props-correct.2",
        ),
    ] {
        assert!(refused(&element(&constraints)).contains(rule), "{rule}: {constraints}");
    }
}

/// Verifies: [EXTENSION] [Schema1 §3.15.2] a schema document's element is
/// a `<schema>` of the XML Schema namespace, of no empty targetNamespace,
/// with form defaults of its type.
#[test]
fn schema_document_elements_are_checked() {
    let read_text = |text: &str| read(&source(text, None), &Library::default()).unwrap_err().to_string();
    assert!(read_text("<schema/>").contains("is a <schema> of the XML Schema namespace"));
    assert!(read_text(&format!(r#"<xs:schema {XS} targetNamespace=""/>"#)).contains("never empty"));
    assert!(read_text(&format!(r#"<xs:schema {XS} elementFormDefault="sometimes"/>"#)).contains("elementFormDefault"));
}
