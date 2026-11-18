//! Tests of assessment by simple type definitions read from schema
//! documents: element and attribute values validated by their facets,
//! annotations of named and anonymous types, typed values of lists and
//! unions, IDs and IDREFs of derived types, simple content restricted.

use super::*;
use crate::xml::AtomicType;

/// [Schema1 §3.14] [Schema2 §4.1.2] Simple types of the examples, and
/// elements and attributes of them.
const TYPES: &str = r#"
<xs:simpleType name="farenheitWaterTemp">
 <xs:restriction base="xs:decimal">
  <xs:fractionDigits value="2"/>
  <xs:minExclusive value="0.00"/>
  <xs:maxExclusive value="100.00"/>
 </xs:restriction>
</xs:simpleType>
<xs:simpleType name="listOfFloat"><xs:list itemType="xs:float"/></xs:simpleType>
<xs:simpleType name="sizes"><xs:union>
  <xs:simpleType><xs:restriction base="xs:positiveInteger"><xs:minInclusive value="8"/><xs:maxInclusive value="72"/></xs:restriction></xs:simpleType>
  <xs:simpleType><xs:restriction base="xs:NMTOKEN"><xs:enumeration value="small"/><xs:enumeration value="large"/></xs:restriction></xs:simpleType>
</xs:union></xs:simpleType>
<xs:element name="r"><xs:complexType><xs:sequence>
  <xs:element name="t" type="farenheitWaterTemp" maxOccurs="unbounded"/>
  <xs:element name="font" minOccurs="0" maxOccurs="unbounded"><xs:complexType><xs:attribute name="size" type="sizes"/></xs:complexType></xs:element>
  <xs:element name="code" minOccurs="0"><xs:simpleType><xs:restriction base="xs:token"><xs:length value="2"/></xs:restriction></xs:simpleType></xs:element>
</xs:sequence><xs:attribute name="floats" type="listOfFloat"/></xs:complexType></xs:element>
"#;

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.3.4
/// cvc-type.3.1.3, §3.2.4 cvc-attribute.3, §3.14.4] [XQueryDM §3.3.1.1,
/// §3.3.1.2] elements and attributes of simple types read from the schema
/// documents are valid by their facets, annotated with their names, an
/// anonymous one's of its own, their typed values of a list's item type
/// and of the union member that validated each; a value a facet refuses
/// is invalid.
#[test]
fn defined_simple_types_validate_and_annotate() {
    let out = valid(TYPES, r#"<r floats=" 1.5  2 "><t> 50.25 </t><font size="large"/><font size="12"/><code>ab</code></r>"#);
    let tree = out.tree();
    let t = element(tree, "t");
    assert_eq!(type_of(tree, t), "farenheitWaterTemp");
    assert_eq!(tree.string_value(t), "50.25");
    assert_eq!(tree.annotation(t).map(|a| a.typing.clone()), Some(Typing::Atomic(AtomicType::Decimal)));
    let floats = attribute(tree, element(tree, "r"), "floats").unwrap();
    assert_eq!((tree.content(floats), type_of(tree, floats)), ("1.5 2", "listOfFloat".to_owned()));
    assert_eq!(tree.typed_value(floats).unwrap().len(), 2);
    let fonts: Vec<NodeId> = tree.descendants(NodeId::ROOT).filter(|&n| tree.name(n).is_some_and(|q| &*q.local == "font")).collect();
    let kinds: Vec<AtomicType> = fonts.iter().map(|&f| tree.typed_value(attribute(tree, f, "size").unwrap()).unwrap()[0].kind).collect();
    assert_eq!(kinds, [AtomicType::NmToken, AtomicType::PositiveInteger]);
    assert_eq!(type_of(tree, attribute(tree, fonts[0], "size").unwrap()), "sizes");
    assert!(type_of(tree, element(tree, "code")).starts_with("#anonymous-simple-"));
    let cases = [
        ("<r><t>100.5</t></r>", "cvc-type.3.1.3"),
        ("<r><t>1.125</t></r>", "cvc-type.3.1.3"),
        (r#"<r><t>1</t><font size="7"/></r>"#, "cvc-attribute.3"),
        (r#"<r floats="1 x"><t>1</t></r>"#, "cvc-attribute.3"),
        ("<r><t>1</t><code>abc</code></r>", "cvc-type.3.1.3"),
    ];
    for (text, rule) in cases {
        assert!(broken(TYPES, text).contains(rule), "{text}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema2 §2.5.1.3,
/// §4.1.2] [XQueryDM §3.3.1.3] a list of a union: each item validated by
/// the first member type that takes it, the node annotated with the item
/// types in order, so its typed value is each item of its own member
/// type, kept so when stored and read back; an item no member takes is
/// invalid.
#[test]
fn a_list_of_a_union_types_each_item() {
    let types = TYPES.split_once("<xs:element").map_or(TYPES, |(types, _)| types);
    let body = format!(
        "{types}{}",
        r#"<xs:simpleType name="sizeList"><xs:list itemType="sizes"/></xs:simpleType><xs:element name="l" type="sizeList"/>"#
    );
    let out = valid(&body, "<l>large 12 small</l>");
    let l = element(out.tree(), "l");
    let kinds = [AtomicType::NmToken, AtomicType::PositiveInteger, AtomicType::NmToken];
    assert_eq!(out.tree().annotation(l).map(|a| a.typing.clone()), Some(Typing::Items(kinds.as_slice().into())));
    let values: Vec<AtomicType> = out.tree().typed_value(l).unwrap().iter().map(|a| a.kind).collect();
    assert_eq!(values, kinds);
    let stored = crate::xml::decode(&crate::xml::encode(&crate::xml::Sequence::new(vec![crate::xml::Item::Node(out)]))).unwrap();
    let Some(crate::xml::Item::Node(back)) = stored.items().first() else { panic!("a node is stored") };
    let read: Vec<AtomicType> = back.tree().typed_value(element(back.tree(), "l")).unwrap().iter().map(|a| a.kind).collect();
    assert_eq!(read, kinds);
    assert!(broken(&body, "<l>large 7</l>").contains("cvc-type.3.1.3"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.3.4 cvc-id]
/// [XQueryDM §6.2.4, §6.3.4] values of types restricting `xs:ID` and
/// `xs:IDREF`, and of a list of the latter, are IDs and IDREFs: marked
/// is-id and is-idrefs and checked against the ID/IDREF table; the facets
/// of the derived ID type are applied.
#[test]
fn ids_and_idrefs_of_derived_types() {
    let body = r#"<xs:simpleType name="key"><xs:restriction base="xs:ID"><xs:pattern value="k\d+"/></xs:restriction></xs:simpleType>
        <xs:simpleType name="ref"><xs:restriction base="xs:IDREF"/></xs:simpleType>
        <xs:simpleType name="refs"><xs:list itemType="ref"/></xs:simpleType>
        <xs:element name="r"><xs:complexType><xs:sequence>
        <xs:element name="p" maxOccurs="unbounded"><xs:complexType><xs:attribute name="id" type="key"/><xs:attribute name="to" type="refs"/></xs:complexType></xs:element>
        </xs:sequence></xs:complexType></xs:element>"#;
    let out = valid(body, r#"<r><p id="k1" to="k2 k1"/><p id="k2"/></r>"#);
    let tree = out.tree();
    let p = element(tree, "p");
    assert!(tree.is_id(attribute(tree, p, "id").unwrap()));
    let to = attribute(tree, p, "to").unwrap();
    assert!(tree.is_idrefs(to));
    assert_eq!(tree.annotation(to).map(|a| a.typing.clone()), Some(Typing::List(AtomicType::IdRef)));
    assert!(broken(body, r#"<r><p id="k1" to="k3"/></r>"#).contains("cvc-id.1"));
    assert!(broken(body, r#"<r><p id="k1"/><p id="k1"/></r>"#).contains("cvc-id.2"));
    assert!(broken(body, r#"<r><p id="x1"/></r>"#).contains("cvc-attribute.3"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.4.4
/// cvc-complex-type.2.2] [Schema1 §3.4.2 {content type} clause 1] an
/// element of a complex type restricting simple content by a facet is
/// valid by the restricted type; a value past the facet is invalid.
#[test]
fn simple_content_restricted_by_facets_validates() {
    let body = r#"<xs:complexType name="price"><xs:simpleContent><xs:extension base="xs:decimal">
        <xs:attribute name="currency" type="xs:token"/></xs:extension></xs:simpleContent></xs:complexType>
        <xs:complexType name="cheap"><xs:simpleContent><xs:restriction base="price"><xs:maxInclusive value="10"/></xs:restriction></xs:simpleContent></xs:complexType>
        <xs:element name="p" type="cheap"/>"#;
    let out = valid(body, r#"<p currency="EUR">9.50</p>"#);
    let p = element(out.tree(), "p");
    assert_eq!(type_of(out.tree(), p), "cheap");
    assert_eq!(out.tree().annotation(p).map(|a| a.typing.clone()), Some(Typing::Atomic(AtomicType::Decimal)));
    assert!(broken(body, r#"<p currency="EUR">10.5</p>"#).contains("cvc-type.3.1.3"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.3.4 cvc-elt.4]
/// [Schema1 §3.14.6 Type Derivation OK (Simple)] `xsi:type` names a simple
/// type read from the schema documents, derived from the declared one or
/// a member of its union, which then validates the element; one not
/// derived is invalid.
#[test]
fn xsi_type_names_a_defined_simple_type() {
    let body = r#"<xs:simpleType name="small"><xs:restriction base="xs:int"><xs:maxInclusive value="5"/></xs:restriction></xs:simpleType>
        <xs:simpleType name="either"><xs:union memberTypes="xs:int xs:date"/></xs:simpleType>
        <xs:element name="n" type="xs:int"/><xs:element name="u" type="either"/>"#;
    let xsi = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;
    let out = valid(body, &format!(r#"<n {xsi} xsi:type="small">4</n>"#));
    assert_eq!(type_of(out.tree(), element(out.tree(), "n")), "small");
    assert!(broken(body, &format!(r#"<n {xsi} xsi:type="small">6</n>"#)).contains("cvc-type.3.1.3"));
    valid(body, &format!(r#"<u {xsi} xsi:type="small">4</u>"#));
    assert!(broken(body, &format!(r#"<n {xsi} xsi:type="either">4</n>"#)).contains("cvc-elt.4.3"));
}
