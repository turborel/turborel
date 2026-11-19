//! Tests of a schema's type definitions by annotation name, and of the
//! names two schemas define twice.

use super::super::tests::structures::{schema, source};
use super::super::{Library, read};
use super::*;
use crate::xml::AtomicType;

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [XQueryDM §3.3.1.1] the
/// name a node of each kind of type definition is annotated with names
/// that definition again: a named one, an anonymous complex one, an
/// anonymous simple one and a built-in one; a name no definition has,
/// and an anonymous one of no such place, name none.
#[test]
fn annotations_name_their_types() {
    let s = schema(
        r#"<xs:complexType name="t"><xs:sequence><xs:element name="v"><xs:simpleType><xs:restriction base="xs:int"/></xs:simpleType></xs:element></xs:sequence></xs:complexType>
        <xs:element name="e"><xs:complexType><xs:attribute name="a" type="xs:int"/></xs:complexType></xs:element>"#,
    )
    .unwrap();
    let named = s.type_names.get(&Expanded::new(None, "t")).copied().unwrap();
    let anonymous = s.element(s.element_names[&Expanded::new(None, "e")]).unwrap().type_def;
    let simple = s.simple_types.iter().position(|d| d.name.is_none()).map(|p| TypeDef::Simple(Simple::Defined(SimpleId(p)))).unwrap();
    for t in [named, anonymous, simple, TypeDef::Simple(Simple::Atomic(AtomicType::Int))] {
        assert_eq!(annotated_type(&s, &annotation_name(&s, t)), Some(t), "{t:?}");
    }
    let nameless = |local: &str| QName { prefix: None, uri: None, local: local.into() };
    assert_eq!(annotated_type(&s, &nameless("u")), None);
    assert_eq!(annotated_type(&s, &nameless("#anonymous-99")), None);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.20 SR2e] [XQuery §4.10
/// err:XQST0035] two schemas define a name twice when they hold
/// definitions of it from different schema documents, not when they hold
/// those of one document, nor when they are one schema.
#[test]
fn names_defined_twice() {
    let xs = r#"xmlns:xs="http://www.w3.org/2001/XMLSchema""#;
    let base = source(&format!(r#"<xs:schema {xs} targetNamespace="urn:x"><xs:element name="e"/></xs:schema>"#), Some("x.xsd"));
    let other = source(&format!(r#"<xs:schema {xs} targetNamespace="urn:x"><xs:element name="e"/></xs:schema>"#), Some("x.xsd"));
    let user = |library: Library| {
        let text = format!(r#"<xs:schema {xs} targetNamespace="urn:y"><xs:import namespace="urn:x" schemaLocation="x.xsd"/></xs:schema>"#);
        read(&source(&text, None), &library).unwrap()
    };
    let x = read(&base, &Library::default()).unwrap();
    let same = user(Library::new(vec![base.clone()]));
    let different = user(Library::new(vec![other]));
    assert_eq!(defined_twice(&x, &same), None);
    assert_eq!(defined_twice(&x, &x), None);
    assert_eq!(defined_twice(&x, &different), Some(Expanded::new(Some("urn:x"), "e")));
}
