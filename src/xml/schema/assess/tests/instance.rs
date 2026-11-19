//! Tests of the instance attributes [Schema1 §3.2.7], nilled and
//! abstract elements, all groups and wildcards in content, and attribute
//! wildcards' IDs, as an element is validated [Schema1 §3.3.4, §3.4.4].

use super::*;

/// The declaration of the instance namespace's prefix `xsi`, and of
/// `xs`.
const XSI: &str = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xs="http://www.w3.org/2001/XMLSchema""#;

/// Each `(text, rule)` of `cases` breaks the rule named, against the
/// schema of `body`.
fn each_broken(body: &str, cases: &[(&str, &str)]) {
    for (text, rule) in cases {
        let got = broken(body, text);
        assert!(got.contains(rule), "{text}: {rule} expected, {got}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.2.7,
/// §3.3.4 cvc-elt.4.1, 4.2] the instance attributes are allowed on an
/// element of a simple type and annotated with their own types;
/// `xsi:type` names a type by a QName in scope, a built-in type included,
/// never `xs:NOTATION`. `xsi:schemaLocation`'s list of `xs:anyURI` is
/// anonymous, its type-name found again as that list [XQueryDM §3.3.1.1].
#[test]
fn instance_attributes_are_typed_and_xsi_type_resolved() {
    let body = r#"<xs:element name="n" type="xs:decimal"/>"#;
    let out =
        valid(body, &format!(r#"<n {XSI} xsi:schemaLocation="urn:a a.xsd" xsi:noNamespaceSchemaLocation="n.xsd" xsi:type="xs:int">5</n>"#));
    let tree = out.tree();
    let n = element(tree, "n");
    assert_eq!(type_of(tree, n), "int");
    assert_eq!(type_of(tree, attribute(tree, n, "noNamespaceSchemaLocation").unwrap()), "anyURI");
    let locations = tree.annotation(attribute(tree, n, "schemaLocation").unwrap()).unwrap().typing.clone();
    let Typing::List(item) = locations else { panic!("{locations:?}") };
    assert_eq!(item.kind, crate::xml::AtomicType::AnyUri);
    let location_type = &tree.annotation(attribute(tree, n, "schemaLocation").unwrap()).unwrap().name;
    assert_eq!(&*location_type.local, "#anonymous-list-anyURI");
    let uris =
        super::super::super::component::TypeDef::Simple(super::super::super::component::Simple::List(crate::xml::AtomicType::AnyUri));
    let s = schema(body).unwrap();
    assert_eq!(super::super::super::named::annotated_type(&s, location_type), Some(uris));
    assert_eq!(super::super::super::check::type_name(&s, uris), None);
    each_broken(
        body,
        &[
            (&format!(r#"<n {XSI} xsi:type="p:int">5</n>"#), "cvc-elt.4.1"),
            (&format!(r#"<n {XSI} xsi:type="xs:NOTATION">5</n>"#), "cvc-elt.4.2"),
        ],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.3.4
/// cvc-elt.3.2, 3.2.2, cvc-type.2] `xsi:nil` is a boolean; false leaves
/// the element to its type, true nils it, of no content and no fixed
/// value; no element is validated by an abstract type.
#[test]
fn nilled_elements_and_abstract_types() {
    let body = r#"<xs:element name="r"><xs:complexType><xs:sequence>
        <xs:element name="n" type="xs:int" nillable="true" minOccurs="0"/>
        <xs:element name="c" nillable="true" minOccurs="0"><xs:complexType><xs:sequence><xs:element name="x"/></xs:sequence></xs:complexType></xs:element>
        <xs:element name="f" type="xs:int" nillable="true" fixed="1" minOccurs="0"/>
        </xs:sequence></xs:complexType></xs:element>
        <xs:complexType name="a" abstract="true"/><xs:element name="z" type="a"/>"#;
    let out = valid(body, &format!(r#"<r {XSI}><n xsi:nil="false">5</n><c xsi:nil="true"/></r>"#));
    assert!(type_of(out.tree(), element(out.tree(), "c")).starts_with("#anonymous"));
    valid(body, &format!(r#"<r {XSI}><n xsi:nil="true"/></r>"#));
    each_broken(
        body,
        &[
            (&format!(r#"<r {XSI}><n xsi:nil="maybe">5</n></r>"#), "cvc-elt.3.2"),
            (&format!(r#"<r {XSI}><f xsi:nil="true"/></r>"#), "cvc-elt.3.2.2"),
            (&format!(r#"<r {XSI}><c xsi:nil="true">text</c></r>"#), "cvc-elt.3.2.1"),
            ("<z/>", "cvc-type.2"),
        ],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.8.4
/// cvc-model-group, §3.4.4 cvc-complex-type.2.1, 2.4] the children of an
/// all group occur in any order, each at most once, the required ones all
/// present, or none if the group is optional; an element of empty content
/// has no child, element or character.
#[test]
fn all_groups_and_empty_content() {
    let body = r#"<xs:element name="r"><xs:complexType><xs:all minOccurs="0"><xs:element name="a"/><xs:element name="b" minOccurs="0"/>
        </xs:all></xs:complexType></xs:element><xs:element name="e"><xs:complexType/></xs:element>"#;
    valid(body, "<r><b/><a/></r>");
    valid(body, "<r><a/></r>");
    valid(body, "<r/>");
    valid(body, "<e></e>");
    each_broken(
        body,
        &[
            ("<r><a/><a/></r>", "cvc-complex-type.2.4"),
            ("<r><c/></r>", "cvc-complex-type.2.4"),
            ("<r><b/></r>", "cvc-complex-type.2.4"),
            ("<e><a/></e>", "cvc-complex-type.2.1"),
            ("<e>text</e>", "cvc-complex-type.2.1"),
        ],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.10.4
/// cvc-wildcard, §3.3.4 cvc-assess-elt.1.1.1] an element matched by a
/// wildcard of strict processing has a declaration; of lax, is validated
/// by one if there is one; of skip, is not assessed, its attributes and
/// children left untyped.
#[test]
fn element_wildcards_process_contents() {
    let body = r###"<xs:element name="g" type="xs:int"/><xs:attribute name="at" type="xs:int"/>
        <xs:element name="r"><xs:complexType><xs:sequence>
        <xs:element name="m"/><xs:any namespace="##local" processContents="strict"/><xs:element name="k"/>
        <xs:any namespace="##local" processContents="lax"/><xs:element name="n"/><xs:any namespace="##local" processContents="skip"/></xs:sequence></xs:complexType></xs:element>"###;
    let out = valid(body, r#"<r><m/><g>1</g><k/><g>2</g><n/><x at="no"><g>three</g></x></r>"#);
    let tree = out.tree();
    assert_eq!(type_of(tree, element(tree, "g")), "int");
    let x = element(tree, "x");
    assert_eq!(type_of(tree, x), "anyType");
    assert_eq!(type_of(tree, attribute(tree, x, "at").unwrap()), "untyped");
    each_broken(
        body,
        &[("<r><m/><y/><k/><g/><n/><x/></r>", "cvc-assess-elt.1.1.1"), ("<r><m/><g>two</g><k/><g/><n/><x/></r>", "cvc-type.3.1.3")],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.4.4
/// cvc-complex-type.4, 5.1, 5.2, §3.2.4 cvc-assess-attr.1] a required
/// attribute is present; of the attributes a wildcard matches, at most one
/// is an ID, and none if the type declares one; a strict attribute
/// wildcard's attribute has a declaration, by which it is typed.
#[test]
fn attribute_wildcards_and_required_attributes() {
    let body = r###"<xs:attribute name="i1" type="xs:ID"/><xs:attribute name="i2" type="xs:ID"/><xs:attribute name="n" type="xs:int"/>
        <xs:element name="w"><xs:complexType><xs:anyAttribute namespace="##local" processContents="strict"/></xs:complexType></xs:element>
        <xs:element name="d"><xs:complexType><xs:attribute name="k" type="xs:ID"/><xs:attribute name="q" use="required"/>
        <xs:anyAttribute processContents="lax"/></xs:complexType></xs:element>"###;
    let out = valid(body, r#"<w i1="a" n="5"/>"#);
    let tree = out.tree();
    assert_eq!(type_of(tree, attribute(tree, element(tree, "w"), "n").unwrap()), "int");
    valid(body, r#"<d q="x" n="1"/>"#);
    each_broken(
        body,
        &[
            (r#"<w i1="a" i2="b"/>"#, "cvc-complex-type.5.1"),
            (r#"<d q="x" i1="a"/>"#, "cvc-complex-type.5.2"),
            ("<d/>", "cvc-complex-type.4"),
            (r#"<w other="1"/>"#, "cvc-assess-attr.1"),
        ],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [XQueryDM §3.3.1.2,
/// §3.3.1.3] an element of a union type is typed by the member that
/// validated it; of a list of a union, as a list of one type if its items
/// are of one, each item of its own if not, and empty if it has none.
#[test]
fn union_values_are_typed_by_their_members() {
    let body = r#"<xs:simpleType name="u"><xs:union memberTypes="xs:int xs:date"/></xs:simpleType>
        <xs:simpleType name="l"><xs:list itemType="u"/></xs:simpleType>
        <xs:element name="r"><xs:complexType><xs:sequence><xs:element name="v" type="u"/><xs:element name="w" type="l" maxOccurs="3"/>
        </xs:sequence></xs:complexType></xs:element>"#;
    let out = valid(body, "<r><v>2026-01-01</v><w>1 2</w><w>1 2026-01-01</w><w></w></r>");
    let tree = out.tree();
    let typings: Vec<Typing> = tree
        .descendants(NodeId::ROOT)
        .filter(|&n| tree.kind(n) == Some(Kind::Element) && tree.name(n).is_some_and(|q| &*q.local != "r"))
        .map(|n| tree.annotation(n).unwrap().typing.clone())
        .collect();
    let Typing::Atomic(v) = &typings[0] else { panic!("{typings:?}") };
    assert_eq!(v.kind, crate::xml::AtomicType::Date);
    let Typing::List(w) = &typings[1] else { panic!("{typings:?}") };
    assert_eq!(w.kind, crate::xml::AtomicType::Int);
    let Typing::Items(kinds) = &typings[2] else { panic!("{typings:?}") };
    assert_eq!(kinds.len(), 2);
    assert_eq!(typings[3], Typing::Empty);
}
