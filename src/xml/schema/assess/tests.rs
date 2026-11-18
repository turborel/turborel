//! Tests of schema-validity assessment and the post-schema-validation
//! infoset it makes.

use super::super::component::Expanded;
use super::super::tests::structures::schema;
use super::*;
use crate::xml::{Kind, Node, NodeId, Syntax, Tree, Typing, Whitespace, parse};

mod more;

/// The document node of `text`.
fn document(text: &str) -> Node {
    Node::root(parse(text, Syntax::Document, Whitespace::Preserve).unwrap())
}

/// `text`, valid against the schema of `body`, validated strictly.
fn valid(body: &str, text: &str) -> Node {
    match validate(&schema(body).unwrap(), &document(text), Mode::Strict).unwrap() {
        Ok(node) => node,
        Err(f) => panic!("{text} is not valid: {f:?}"),
    }
}

/// The rule `text` breaks, validated strictly against the schema of
/// `body`.
fn broken(body: &str, text: &str) -> String {
    let Err(f) = validate(&schema(body).unwrap(), &document(text), Mode::Strict).unwrap() else { panic!("{text} is valid") };
    assert_eq!(f.code, INVALID, "{f:?}");
    f.rule
}

/// The first element of `tree` named `local`.
fn element(tree: &Tree, local: &str) -> NodeId {
    tree.descendants(NodeId::ROOT)
        .find(|&n| tree.kind(n) == Some(Kind::Element) && tree.name(n).is_some_and(|q| &*q.local == local))
        .unwrap()
}

/// The attribute of element `e` of `tree` named `local`.
fn attribute(tree: &Tree, e: NodeId, local: &str) -> Option<NodeId> {
    tree.attributes(e).find(|&a| tree.name(a).is_some_and(|q| &*q.local == local))
}

/// The local name of the type annotation of node `n` of `tree`.
fn type_of(tree: &Tree, n: NodeId) -> String {
    tree.annotation(n).map_or_else(|| "untyped".to_owned(), |a| a.name.local.to_string())
}

/// A purchase order schema, after [Schema1 §3.4]'s example.
const ORDER: &str = r#"
<xs:complexType name="USAddress">
  <xs:sequence><xs:element name="name" type="xs:string"/><xs:element name="zip" type="xs:decimal"/></xs:sequence>
  <xs:attribute name="country" type="xs:NMTOKEN" fixed="US"/>
</xs:complexType>
<xs:element name="order">
  <xs:complexType>
    <xs:sequence>
      <xs:element name="shipTo" type="USAddress"/>
      <xs:element name="billTo" type="USAddress" minOccurs="0"/>
      <xs:element name="item" maxOccurs="unbounded" type="xs:string"/>
    </xs:sequence>
    <xs:attribute name="orderDate" type="xs:date"/>
    <xs:attribute name="priority" type="xs:int" default="3"/>
  </xs:complexType>
</xs:element>
"#;

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [XQuery §3.13] [XQueryDM
/// §3.3.1, §6.2.4, §6.3.4] a valid document validates to a new document
/// whose elements and attributes are annotated with their types, their
/// typed values those types give, a default attribute added and a fixed
/// one's value kept, and an element of an anonymous type annotated with
/// a name of its own.
#[test]
fn a_valid_order_is_annotated() {
    let out = valid(
        ORDER,
        r#"<order orderDate="2026-11-17"><shipTo><name>A</name><zip> 12345.0 </zip></shipTo><item>x</item><item>y</item></order>"#,
    );
    let tree = out.tree();
    assert_eq!(tree.kind(out.id()), Some(Kind::Document));
    let order = element(tree, "order");
    assert!(type_of(tree, order).starts_with("#anonymous-"));
    assert_eq!(type_of(tree, element(tree, "shipTo")), "USAddress");
    let zip = element(tree, "zip");
    assert_eq!(type_of(tree, zip), "decimal");
    assert_eq!(tree.string_value(zip), "12345.0");
    let typed = tree.typed_value(zip).unwrap();
    assert_eq!((typed[0].kind, &*typed[0].lexical), (crate::xml::AtomicType::Decimal, "12345"));
    let date = attribute(tree, order, "orderDate").unwrap();
    assert_eq!(type_of(tree, date), "date");
    let priority = attribute(tree, order, "priority").unwrap();
    assert_eq!((tree.content(priority), type_of(tree, priority)), ("3", "int".to_owned()));
    assert_eq!(tree.annotation(order).map(|a| a.typing), Some(Typing::ElementOnly));
    assert!(tree.typed_value(order).is_err());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.4.4
/// cvc-complex-type.2.4, §3.3.4 cvc-type.3.1.3, §3.2.4 cvc-attribute.3,
/// §3.5.4 cvc-au] [Schema1 §3.4.4 cvc-complex-type.3.2.1, .4] what makes
/// an order invalid: a child missing, one not expected, a value not of its
/// type, a fixed value not kept, an attribute undeclared.
#[test]
fn invalid_orders_name_the_rule_broken() {
    let cases = [
        ("<order><item>x</item></order>", "cvc-complex-type.2.4"),
        ("<order><shipTo><name>A</name><zip>1</zip></shipTo></order>", "cvc-complex-type.2.4"),
        ("<order><shipTo><name>A</name><zip>1</zip><extra/></shipTo><item/></order>", "cvc-complex-type.2.4"),
        ("<order><shipTo><name>A</name><zip>one</zip></shipTo><item/></order>", "cvc-type.3.1.3"),
        (r#"<order orderDate="17 Nov"><shipTo><name>A</name><zip>1</zip></shipTo><item/></order>"#, "cvc-attribute.3"),
        (r#"<order><shipTo country="UK"><name>A</name><zip>1</zip></shipTo><item/></order>"#, "cvc-au"),
        (r#"<order colour="red"><shipTo><name>A</name><zip>1</zip></shipTo><item/></order>"#, "cvc-complex-type.3.2.1"),
        ("<order><shipTo>text<name>A</name><zip>1</zip></shipTo><item/></order>", "cvc-complex-type.2.3"),
        ("<order><shipTo><name>A<b/></name><zip>1</zip></shipTo><item/></order>", "cvc-complex-type.2.2"),
    ];
    for (text, rule) in cases {
        assert!(broken(ORDER, text).contains(rule), "{text}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [XQuery §3.13] strict validation of an element with no
/// top-level declaration is err:XQDY0084; lax validation of it validates
/// it laxly, annotating it `xs:anyType` and its children by their
/// declarations if they have them.
#[test]
fn strict_and_lax_modes() {
    let s = schema(r#"<xs:element name="n" type="xs:int"/>"#).unwrap();
    let d = document("<other><n>7</n><m>x</m></other>");
    assert_eq!(validate(&s, &d, Mode::Strict).unwrap().unwrap_err().code, UNDECLARED);
    let out = validate(&s, &d, Mode::Lax).unwrap().unwrap();
    let tree = out.tree();
    assert_eq!(type_of(tree, element(tree, "other")), "anyType");
    assert_eq!(type_of(tree, element(tree, "n")), "int");
    assert_eq!(type_of(tree, element(tree, "m")), "anyType");
    assert!(validate(&s, &document("<other><n>seven</n></other>"), Mode::Lax).unwrap().is_err());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [XQuery §3.13] an element node validates to a new element
/// node with no parent; a text node is no operand, err:XQTY0030.
#[test]
fn operands_are_documents_or_elements() {
    let s = schema(r#"<xs:element name="n" type="xs:int"/>"#).unwrap();
    let d = document("<n>7</n>");
    let n = Node::new(std::sync::Arc::new(d.tree().clone()), element(d.tree(), "n"));
    let out = validate(&s, &n, Mode::Strict).unwrap().unwrap();
    assert_eq!(out.kind(), Some(Kind::Element));
    assert!(out.tree().parent(out.id()).is_none());
    let text = Node::new(std::sync::Arc::new(d.tree().clone()), d.tree().first_child(element(d.tree(), "n")).unwrap());
    assert_eq!(validate(&s, &text, Mode::Strict).unwrap().unwrap_err().code, NOT_A_NODE);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.3.4 cvc-elt.3] [XQueryDM §6.2.4] xsi:nil: of a
/// nillable declaration, true, the element is nilled, empty, with no
/// typed value; with content, or of a declaration not nillable, invalid.
#[test]
fn xsi_nil_nils_elements_declared_nillable() {
    let body = r#"<xs:element name="r"><xs:complexType><xs:sequence>
        <xs:element name="n" type="xs:int" nillable="true"/><xs:element name="m" type="xs:int" minOccurs="0"/>
        </xs:sequence></xs:complexType></xs:element>"#;
    let xsi = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;
    let out = valid(body, &format!(r#"<r {xsi}><n xsi:nil="true"/></r>"#));
    let n = element(out.tree(), "n");
    assert!(out.tree().nilled(n));
    assert_eq!(out.tree().typed_value(n).unwrap(), Vec::new());
    assert!(broken(body, &format!(r#"<r {xsi}><n xsi:nil="true">1</n></r>"#)).contains("cvc-elt.3.2.1"));
    assert!(broken(body, &format!(r#"<r {xsi}><n>1</n><m xsi:nil="false">2</m></r>"#)).contains("cvc-elt.3.1"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.3.4 cvc-elt.4] xsi:type names a type validly
/// derived from the declared one, which then validates the element; a
/// type not derived, or a derivation the declaration blocks, is invalid.
#[test]
fn xsi_type_chooses_a_derived_type() {
    let body = r#"<xs:complexType name="base"><xs:sequence><xs:element name="a"/></xs:sequence></xs:complexType>
        <xs:complexType name="more"><xs:complexContent><xs:extension base="base"><xs:sequence><xs:element name="b"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>
        <xs:complexType name="other"><xs:sequence><xs:element name="a"/></xs:sequence></xs:complexType>
        <xs:element name="e" type="base"/><xs:element name="f" type="base" block="extension"/>"#;
    let xsi = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;
    let out = valid(body, &format!(r#"<e {xsi} xsi:type="more"><a/><b/></e>"#));
    assert_eq!(type_of(out.tree(), element(out.tree(), "e")), "more");
    assert!(broken(body, &format!(r#"<e {xsi} xsi:type="other"><a/></e>"#)).contains("cvc-elt.4.3"));
    assert!(broken(body, &format!(r#"<f {xsi} xsi:type="more"><a/><b/></f>"#)).contains("cvc-elt.4.3"));
    assert!(broken(body, &format!(r#"<e {xsi} xsi:type="missing"><a/></e>"#)).contains("cvc-elt.4.2"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.3.5] [Schema1 §3.3.4 cvc-elt.5] an empty element
/// of a declaration with a default takes it as its value; a fixed value
/// is matched as a value, not a string.
#[test]
fn element_defaults_and_fixed_values() {
    let body = r#"<xs:element name="r"><xs:complexType><xs:sequence>
        <xs:element name="d" type="xs:int" default="42"/><xs:element name="f" type="xs:decimal" fixed="1.5" minOccurs="0"/>
        </xs:sequence></xs:complexType></xs:element>"#;
    let out = valid(body, "<r><d/><f>1.50</f></r>");
    assert_eq!(out.tree().string_value(element(out.tree(), "d")), "42");
    assert!(broken(body, "<r><d/><f>2</f></r>").contains("cvc-elt.5.2.2.2.2"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.9.4 cvc-particle.2.3.3] [Schema1 §3.3.6
/// cos-equiv-derived-ok-rec] a member of a substitution group stands for
/// its abstract head, which no element is validated by itself.
#[test]
fn substitution_groups_stand_for_their_head() {
    let body = r#"<xs:element name="shape" abstract="true"/><xs:element name="circle" substitutionGroup="shape"/>
        <xs:element name="r"><xs:complexType><xs:sequence><xs:element ref="shape" maxOccurs="unbounded"/></xs:sequence></xs:complexType></xs:element>"#;
    valid(body, "<r><circle/><circle/></r>");
    assert!(broken(body, "<r><shape/></r>").contains("cvc-complex-type.2.4"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.10.4] [Schema1 §3.10.1] wildcards: strict needs a
/// declaration, lax validates by one if there is one, skip validates
/// nothing; a namespace not allowed is not matched.
#[test]
fn wildcards_process_contents() {
    let body = |process: &str| {
        format!(
            r###"<xs:element name="n" type="xs:int"/><xs:element name="r"><xs:complexType><xs:sequence>
        <xs:any processContents="{process}" namespace="##local" maxOccurs="unbounded"/></xs:sequence></xs:complexType></xs:element>"###
        )
    };
    assert!(broken(&body("strict"), "<r><m/></r>").contains("cvc-assess-elt"));
    valid(&body("strict"), "<r><n>1</n></r>");
    assert!(broken(&body("lax"), "<r><n>x</n></r>").contains("cvc-type.3.1.3"));
    let out = valid(&body("lax"), "<r><n>1</n><m><x/></m></r>");
    assert_eq!(type_of(out.tree(), element(out.tree(), "m")), "anyType");
    valid(&body("skip"), "<r><n>x</n></r>");
    assert!(broken(&body("skip"), r#"<r><p:n xmlns:p="urn:p"/></r>"#).contains("cvc-complex-type.2.4"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.8.4 clause 3] an all group's elements come in any
/// order, each at most once, the required ones each once.
#[test]
fn all_groups_take_any_order() {
    let body = r#"<xs:element name="r"><xs:complexType><xs:all><xs:element name="a"/><xs:element name="b" minOccurs="0"/></xs:all></xs:complexType></xs:element>"#;
    valid(body, "<r><b/><a/></r>");
    valid(body, "<r><a/></r>");
    assert!(broken(body, "<r><b/></r>").contains("cvc-complex-type.2.4"));
    assert!(broken(body, "<r><a/><a/></r>").contains("cvc-complex-type.2.4"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.11.4 cvc-identity-constraint] [XQuery §3.13] keys
/// and keyrefs of a document: a key's values unique and present, a
/// keyref's found among its key's; not checked of an element node.
#[test]
fn identity_constraints_hold_in_documents() {
    let body = r#"<xs:element name="r"><xs:complexType><xs:sequence>
        <xs:element name="p" maxOccurs="unbounded"><xs:complexType><xs:attribute name="id" type="xs:int"/></xs:complexType></xs:element>
        <xs:element name="ref" minOccurs="0" maxOccurs="unbounded"><xs:complexType><xs:attribute name="to" type="xs:int"/></xs:complexType></xs:element>
        </xs:sequence></xs:complexType>
        <xs:key name="k"><xs:selector xpath="p"/><xs:field xpath="@id"/></xs:key>
        <xs:keyref name="kr" refer="k"><xs:selector xpath="ref"/><xs:field xpath="@to"/></xs:keyref></xs:element>"#;
    valid(body, r#"<r><p id="1"/><p id="02"/><ref to="2"/></r>"#);
    assert!(broken(body, r#"<r><p id="1"/><p id="01"/></r>"#).contains("cvc-identity-constraint.4.2.2"));
    assert!(broken(body, r#"<r><p id="1"/><p/></r>"#).contains("cvc-identity-constraint.4.2.1"));
    assert!(broken(body, r#"<r><p id="1"/><ref to="3"/></r>"#).contains("cvc-identity-constraint.4.3"));
    let s = schema(body).unwrap();
    let d = document(r#"<r><p id="1"/><p id="1"/></r>"#);
    let root = Node::new(std::sync::Arc::new(d.tree().clone()), element(d.tree(), "r"));
    assert!(validate(&s, &root, Mode::Strict).unwrap().is_ok());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.3.4 cvc-id] [XQueryDM §6.3.4] the ID/IDREF table:
/// an IDREF to no ID, and an ID given twice, are invalid; IDs and IDREFS
/// are marked is-id and is-idrefs, an IDREFS's typed value a list.
#[test]
fn ids_and_idrefs() {
    let body = r#"<xs:element name="r"><xs:complexType><xs:sequence>
        <xs:element name="p" maxOccurs="unbounded"><xs:complexType><xs:attribute name="id" type="xs:ID"/><xs:attribute name="refs" type="xs:IDREFS"/></xs:complexType></xs:element>
        </xs:sequence></xs:complexType></xs:element>"#;
    let out = valid(body, r#"<r><p id="a" refs="a  b"/><p id="b"/></r>"#);
    let tree = out.tree();
    let p = element(tree, "p");
    assert!(tree.is_id(attribute(tree, p, "id").unwrap()));
    let refs = attribute(tree, p, "refs").unwrap();
    assert!(tree.is_idrefs(refs));
    assert_eq!(tree.content(refs), "a b");
    assert_eq!(tree.typed_value(refs).unwrap().len(), 2);
    assert!(broken(body, r#"<r><p id="a" refs="c"/></r>"#).contains("cvc-id.1"));
    assert!(broken(body, r#"<r><p id="a"/><p id="a"/></r>"#).contains("cvc-id.2"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.22] [Schema1 §3.3.4 cvc-type.3.1.1, cvc-elt.2] an element of a
/// simple type has no attribute but those of the xsi namespace; an
/// abstract declaration validates no element.
#[test]
fn simple_elements_have_no_attributes() {
    assert!(broken(r#"<xs:element name="n" type="xs:int"/>"#, r#"<n a="1">1</n>"#).contains("cvc-type.3.1.1"));
    let xsi = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;
    valid(r#"<xs:element name="n" type="xs:int"/>"#, &format!(r#"<n {xsi} xsi:schemaLocation="urn:a b">1</n>"#));
    assert!(broken(r#"<xs:element name="n" abstract="true"/>"#, "<n/>").contains("cvc-elt.2"));
}

/// Verifies: [DESIGN] RULES.md D86 [XQueryDM §3.3] a validated tree's
/// annotations, nilled, is-id and is-idrefs properties survive storing it
/// and reading it back, and copying it into another tree.
#[test]
fn annotations_are_stored_and_copied() {
    let body = r#"<xs:element name="r"><xs:complexType><xs:sequence>
        <xs:element name="p" maxOccurs="unbounded" nillable="true"><xs:complexType><xs:attribute name="id" type="xs:ID"/></xs:complexType></xs:element>
        <xs:element name="v" type="xs:IDREFS"/></xs:sequence></xs:complexType></xs:element>"#;
    let xsi = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;
    let out = valid(body, &format!(r#"<r {xsi}><p id="a" xsi:nil="true"/><v>a a</v></r>"#));
    let stored = crate::xml::encode(&crate::xml::Sequence::new(vec![crate::xml::Item::Node(out.clone())]));
    let back = crate::xml::decode(&stored).unwrap();
    let crate::xml::Item::Node(read) = &back.items()[0] else { panic!("a node") };
    let mut copier = crate::xml::Builder::document();
    copier.copy(read.tree(), read.id()).unwrap();
    let copy = copier.finish().unwrap();
    for tree in [out.tree(), read.tree(), &copy] {
        let p = element(tree, "p");
        assert!(tree.nilled(p));
        assert!(tree.is_id(attribute(tree, p, "id").unwrap()));
        let v = element(tree, "v");
        assert_eq!(type_of(tree, v), "IDREFS");
        assert!(tree.is_idrefs(v));
        assert_eq!(tree.typed_value(v).unwrap().len(), 2);
        assert!(type_of(tree, element(tree, "r")).starts_with("#anonymous-"));
    }
}
