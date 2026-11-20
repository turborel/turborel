//! Tests of reading schema documents into schemas, and of the schema
//! component and representation constraints they are checked against.
//! Several schemas are the examples of [Schema1 §3] themselves.

use std::sync::Arc;

use super::super::*;
use crate::error::{Result, SqlState};
use crate::xml::{Syntax, Whitespace, parse};

mod complex;
mod compose;
mod depth;
mod refusals;
mod restriction;
mod simple;

/// The declaration of the prefix `xs`.
const XS: &str = r#"xmlns:xs="http://www.w3.org/2001/XMLSchema""#;

/// A schema document of `text`, at `location`.
pub(in crate::xml::schema) fn source(text: &str, location: Option<&str>) -> Source {
    Source { location: location.map(Into::into), tree: Arc::new(parse(text, Syntax::Document, Whitespace::Preserve).unwrap()) }
}

/// The schema of `body`, the content of a `<xs:schema>` of no target
/// namespace.
pub(in crate::xml::schema) fn schema(body: &str) -> Result<Schema> {
    read(&source(&format!("<xs:schema {XS}>{body}</xs:schema>"), None), &Library::default())
}

/// The message of the error reading `body` raises, which is SQLSTATE
/// 22000.
fn refused(body: &str) -> String {
    let e = schema(body).unwrap_err();
    assert_eq!(e.sqlstate(), SqlState::DATA_EXCEPTION, "{e}");
    e.to_string()
}

/// The complex type definition of `schema` named `name`.
fn complex<'s>(schema: &'s Schema, name: &str) -> &'s ComplexType {
    let Some(TypeDef::Complex(id)) = schema.type_names.get(&Expanded::new(None, name)) else { panic!("no type {name}") };
    schema.complex(*id).unwrap()
}

/// [Schema1 §3.4] The purchase order type of the example of complex type
/// definitions.
const PURCHASE_ORDER: &str = r#"
<xs:complexType name="PurchaseOrderType">
  <xs:sequence>
   <xs:element name="shipTo" type="USAddress"/>
   <xs:element name="billTo" type="USAddress"/>
   <xs:element ref="comment" minOccurs="0"/>
   <xs:element name="items"  type="Items"/>
  </xs:sequence>
  <xs:attribute name="orderDate" type="xs:date"/>
</xs:complexType>
<xs:complexType name="USAddress">
  <xs:sequence><xs:element name="name" type="xs:string"/><xs:element name="zip" type="xs:decimal"/></xs:sequence>
  <xs:attribute name="country" type="xs:NMTOKEN" fixed="US"/>
</xs:complexType>
<xs:complexType name="Items">
  <xs:sequence><xs:element name="item" minOccurs="0" maxOccurs="unbounded" type="xs:string"/></xs:sequence>
</xs:complexType>
<xs:element name="comment" type="xs:string"/>
<xs:element name="purchaseOrder" type="PurchaseOrderType"/>
"#;

/// Verifies: [EXTENSION] [Schema1 §3.4.2, §3.3.2, §3.2.2, §3.15.2] the
/// purchase order of the example of [Schema1 §3.4]: its named types,
/// global and local element declarations, an attribute use with a fixed
/// value, references made before what they name, and an automaton for
/// each content model.
#[test]
fn the_purchase_order_example_reads() {
    let s = schema(PURCHASE_ORDER).unwrap();
    let po = complex(&s, "PurchaseOrderType");
    assert_eq!(po.method, Method::Restriction);
    assert_eq!(po.base, TypeDef::Complex(ComplexId::ANY_TYPE));
    assert!(matches!(po.content, ContentType::ElementOnly(_)));
    assert_eq!(po.attribute_uses.len(), 1);
    let address = complex(&s, "USAddress");
    let country = &address.attribute_uses[0];
    assert!(s.attribute(country.decl).unwrap().value.is_none());
    assert_eq!(country.value.as_ref().map(|v| (v.kind, v.value.canonical())), Some((ValueKind::Fixed, "US".to_owned())));
    assert!(s.element_names.contains_key(&Expanded::new(None, "purchaseOrder")));
    let id = match s.type_names.get(&Expanded::new(None, "PurchaseOrderType")) {
        Some(TypeDef::Complex(id)) => *id,
        other => panic!("{other:?}"),
    };
    assert!(s.automaton(id).is_some());
}

/// [Schema1 §3.4.2] The personal names of the examples of complex content
/// extended and restricted.
const NAMES: &str = r#"
<xs:complexType name="personName">
 <xs:sequence>
  <xs:element name="title" minOccurs="0"/>
  <xs:element name="forename" minOccurs="0" maxOccurs="unbounded"/>
  <xs:element name="surname"/>
 </xs:sequence>
</xs:complexType>
<xs:complexType name="extendedName">
 <xs:complexContent>
  <xs:extension base="personName">
   <xs:sequence><xs:element name="generation" minOccurs="0"/></xs:sequence>
  </xs:extension>
 </xs:complexContent>
</xs:complexType>
<xs:complexType name="simpleName">
 <xs:complexContent>
  <xs:restriction base="personName">
   <xs:sequence>
    <xs:element name="forename" minOccurs="1" maxOccurs="1"/>
    <xs:element name="surname"/>
   </xs:sequence>
  </xs:restriction>
 </xs:complexContent>
</xs:complexType>
<xs:element name="addressee" type="extendedName"/>
<xs:element name="who" type="simpleName"/>
"#;

/// Verifies: [EXTENSION] [Schema1 §3.4.2, §3.4.6 cos-ct-extends,
/// derivation-ok-restriction, §3.9.6 rcase-Recurse] the extension of
/// personName is a sequence of its base's content and its own, and the
/// restriction that drops an optional child and fixes another to occur
/// once is a valid restriction.
#[test]
fn names_extend_and_restrict_as_the_examples_show() {
    let s = schema(NAMES).unwrap();
    let base = complex(&s, "personName");
    let extended = complex(&s, "extendedName");
    assert_eq!(extended.method, Method::Extension);
    let ContentType::ElementOnly(p) = &extended.content else { panic!() };
    let Term::Group(g) = &p.term else { panic!() };
    let group = s.group(*g).unwrap();
    assert_eq!(group.compositor, Compositor::Sequence);
    assert_eq!(group.particles.first(), base.content.particle());
    assert_eq!(complex(&s, "simpleName").method, Method::Restriction);
}

/// Verifies: [EXTENSION] [Schema1 §3.4.6 derivation-ok-restriction.5.3]
/// [Schema1 §3.9.6 rcase-Recurse, rcase-NameAndTypeOK.3] a restriction
/// that lets surname occur twice, more than its base does, is refused.
#[test]
fn a_restriction_wider_than_its_base_is_refused() {
    let wider = NAMES.replace(
        r#"<xs:element name="surname"/>
   </xs:sequence>
  </xs:restriction>"#,
        r#"<xs:element name="surname" maxOccurs="2"/>
   </xs:sequence>
  </xs:restriction>"#,
    );
    assert!(refused(&wider).contains("derivation-ok-restriction.5.3"));
}

/// Verifies: [EXTENSION] [Schema1 §3.8.6 cos-nonambig, Appendix H] a
/// content model in which an element may match either of two particles
/// is refused: an optional `a` then `a`, a choice of `a` or `a`, and a
/// wildcard beside an element of a namespace it allows; one whose
/// particles do not compete reads.
#[test]
fn unique_particle_attribution_is_checked() {
    let model = |m: &str| format!(r#"<xs:complexType name="t">{m}</xs:complexType>"#);
    for ambiguous in [
        r#"<xs:sequence><xs:element name="a" minOccurs="0"/><xs:element name="a"/></xs:sequence>"#,
        r#"<xs:choice><xs:element name="a"/><xs:element name="a"/></xs:choice>"#,
        r###"<xs:sequence><xs:any namespace="##local" minOccurs="0"/><xs:element name="b"/></xs:sequence>"###,
        r#"<xs:sequence><xs:element name="a" maxOccurs="unbounded"/><xs:element name="a" minOccurs="0"/></xs:sequence>"#,
    ] {
        assert!(refused(&model(ambiguous)).contains("cos-nonambig"), "{ambiguous}");
    }
    for fine in [
        r#"<xs:sequence><xs:element name="a"/><xs:element name="a" minOccurs="0"/></xs:sequence>"#,
        r#"<xs:sequence><xs:element name="a" minOccurs="2" maxOccurs="5"/><xs:element name="b"/></xs:sequence>"#,
        r#"<xs:choice maxOccurs="unbounded"><xs:element name="a"/><xs:element name="b"/></xs:choice>"#,
        r###"<xs:sequence><xs:any namespace="##other" processContents="lax"/><xs:element name="b"/></xs:sequence>"###,
    ] {
        schema(&model(fine)).unwrap();
    }
}

/// Verifies: [EXTENSION] [Schema1 §3.8.6 cos-element-consistent] two
/// elements of one name in one content model are of one named type: of
/// two types, or of an anonymous type, they are refused.
#[test]
fn element_declarations_consistent_is_checked() {
    let m = r#"<xs:complexType name="t"><xs:choice><xs:sequence><xs:element name="a" type="xs:int"/><xs:element name="b"/></xs:sequence>
        <xs:sequence><xs:element name="b"/><xs:element name="a" type="xs:string"/></xs:sequence></xs:choice></xs:complexType>"#;
    assert!(refused(m).contains("cos-element-consistent"));
    let same = m.replace("xs:string", "xs:int");
    schema(&same).unwrap();
}

/// Verifies: [EXTENSION] [Schema1 §3.4.6 ct-props-correct.3] [Schema1
/// §3.8.6 mg-props-correct.2] [Schema1 §3.6.3 src-attribute_group.3]
/// circular definitions are refused: types deriving from each other, a
/// group holding itself, an attribute group referring to itself.
#[test]
fn circular_definitions_are_refused() {
    let types = r#"<xs:complexType name="a"><xs:complexContent><xs:extension base="b"/></xs:complexContent></xs:complexType>
        <xs:complexType name="b"><xs:complexContent><xs:extension base="a"/></xs:complexContent></xs:complexType>"#;
    assert!(refused(types).contains("ct-props-correct.3"));
    let groups = r#"<xs:group name="g"><xs:sequence><xs:element name="e"/><xs:group ref="g" minOccurs="0"/></xs:sequence></xs:group>"#;
    assert!(refused(groups).contains("mg-props-correct.2"));
    let attributes = r#"<xs:attributeGroup name="g"><xs:attribute name="a"/><xs:attributeGroup ref="g"/></xs:attributeGroup>"#;
    assert!(refused(attributes).contains("src-attribute_group.3"));
}

/// Verifies: [EXTENSION] [Schema1 §3.15.3 src-resolve] [Schema1 §3.15.6
/// sch-props-correct.2] a reference that names nothing, and two
/// top-level components of one kind and name, are refused; a type and an
/// element of one name are not, as they are of different symbol spaces
/// [Schema1 §2.5].
#[test]
fn names_resolve_and_are_unique_in_their_symbol_space() {
    assert!(refused(r#"<xs:element name="e" type="missing"/>"#).contains("src-resolve"));
    assert!(refused(r#"<xs:element name="e"/><xs:element name="e"/>"#).contains("sch-props-correct.2"));
    schema(r#"<xs:complexType name="e"/><xs:element name="e" type="e"/>"#).unwrap();
}

/// Verifies: [EXTENSION] [Schema1 §3.8.6 cos-all-limited] an all group is
/// the whole of a content model and its elements occur at most once.
#[test]
fn all_groups_are_limited() {
    schema(r#"<xs:complexType name="t"><xs:all><xs:element name="a"/><xs:element name="b" minOccurs="0"/></xs:all></xs:complexType>"#)
        .unwrap();
    let nested = r#"<xs:group name="g"><xs:all><xs:element name="a"/></xs:all></xs:group>
        <xs:complexType name="t"><xs:sequence><xs:group ref="g"/><xs:element name="b"/></xs:sequence></xs:complexType>"#;
    assert!(refused(nested).contains("cos-all-limited"));
    let twice = r#"<xs:complexType name="t"><xs:all><xs:element name="a" maxOccurs="2"/></xs:all></xs:complexType>"#;
    assert!(refused(twice).contains("cos-all-limited.2"));
}

/// Verifies: [EXTENSION] [Schema1 §3.2.3 src-attribute.1, .2] [Schema1
/// §3.2.6 a-props-correct.2, .3, no-xmlns, no-xsi] [Schema1 §3.5.6
/// au-props-correct.2] attribute declarations' constraints.
#[test]
fn attribute_declarations_are_checked() {
    let cases = [
        (r#"<xs:attribute name="a" default="1" fixed="1"/>"#, "src-attribute.1"),
        (r#"<xs:attribute name="a" type="xs:int" default="x"/>"#, "a-props-correct.2"),
        (r#"<xs:attribute name="a" type="xs:ID" default="x"/>"#, "a-props-correct.3"),
        (r#"<xs:attribute name="xmlns"/>"#, "no-xmlns"),
        (r#"<xs:complexType name="t"><xs:attribute name="a" use="required" default="1"/></xs:complexType>"#, "src-attribute.2"),
        (
            r#"<xs:attribute name="a" fixed="1"/><xs:complexType name="t"><xs:attribute ref="a" fixed="2"/></xs:complexType>"#,
            "au-props-correct.2",
        ),
    ];
    for (body, rule) in cases {
        assert!(refused(body).contains(rule), "{body}");
    }
}

/// Verifies: [EXTENSION] [Schema1 §3.3.6 e-props-correct.2, .3, .4,
/// cos-valid-default.2] element declarations' constraints: a default not
/// of the type, of an ID type, or of element-only content, and a
/// substitution group member of a type not derived from its head's.
#[test]
fn element_declarations_are_checked() {
    let cases = [
        (r#"<xs:element name="e" type="xs:int" default="x"/>"#, "e-props-correct.2"),
        (r#"<xs:element name="e" type="xs:ID" fixed="x"/>"#, "e-props-correct.4"),
        (
            r#"<xs:complexType name="t"><xs:sequence><xs:element name="c"/></xs:sequence></xs:complexType><xs:element name="e" type="t" default="x"/>"#,
            "cos-valid-default.2",
        ),
        (r#"<xs:element name="h" type="xs:int"/><xs:element name="m" type="xs:string" substitutionGroup="h"/>"#, "e-props-correct.3"),
    ];
    for (body, rule) in cases {
        assert!(refused(body).contains(rule), "{body}");
    }
    let members = schema(
        r#"<xs:element name="h" type="xs:decimal" abstract="true"/><xs:element name="m" type="xs:int" substitutionGroup="h"/>
        <xs:element name="n" substitutionGroup="h"/>"#,
    )
    .unwrap();
    let n = members.element(members.element_names[&Expanded::new(None, "n")]).unwrap();
    assert_eq!(n.type_def, TypeDef::Simple(Simple::Atomic(AtomicType::Decimal)));
}

/// Verifies: [EXTENSION] [Schema1 §3.4.6 cos-ct-extends.1.4] [Schema1
/// §3.8.6 cos-nonambig] an extension of a mixed type is mixed: element-only
/// content does not extend it, mixed content does; and extending
/// `xs:anyType` by an element makes its wildcard compete with the
/// element.
#[test]
fn extensions_keep_their_base_content_kind() {
    let ext = |base: &str, mixed: &str| {
        format!(
            r#"<xs:complexType name="b" mixed="true"><xs:sequence><xs:element name="x"/></xs:sequence></xs:complexType>
        <xs:complexType name="t"><xs:complexContent {mixed}><xs:extension base="{base}"><xs:sequence><xs:element name="a"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>"#
        )
    };
    assert!(refused(&ext("b", "")).contains("cos-ct-extends.1.4"));
    schema(&ext("b", r#"mixed="true""#)).unwrap();
    assert!(refused(&ext("xs:anyType", r#"mixed="true""#)).contains("cos-nonambig"));
}

/// Verifies: [EXTENSION] [Schema1 §3.9.6 p-props-correct.2] [Schema1
/// Appendix A] occurrence ranges and the Schema for Schemas' structure:
/// minOccurs past maxOccurs, an attribute no element of its kind has, and
/// children out of order are refused.
#[test]
fn representations_follow_the_schema_for_schemas() {
    assert!(
        refused(
            r#"<xs:complexType name="t"><xs:sequence><xs:element name="a" minOccurs="3" maxOccurs="2"/></xs:sequence></xs:complexType>"#
        )
        .contains("p-props-correct.2")
    );
    assert!(refused(r#"<xs:element name="e" colour="red"/>"#).contains("no attribute colour"));
    assert!(refused(r#"<xs:complexType name="t"><xs:attribute name="a"/><xs:sequence/></xs:complexType>"#).contains("no <sequence> here"));
    assert!(refused(r#"<xs:element name="e" minOccurs="1"/>"#).contains("no attribute minOccurs"));
}

/// Verifies: [DESIGN] RULES.md D18: a content model whose occurrence
/// ranges unfold past `MAX_POSITIONS` positions is SQLCODE -900; one
/// within it reads.
#[test]
fn content_models_are_bounded() {
    let model = |max: usize| {
        format!(r#"<xs:complexType name="t"><xs:sequence><xs:element name="a" maxOccurs="{max}"/></xs:sequence></xs:complexType>"#)
    };
    schema(&model(1000)).unwrap();
    assert_eq!(schema(&model(MAX_POSITIONS.saturating_add(1))).unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [EXTENSION] [Schema1 §3.11.2, §3.11.6 c-props-correct.2,
/// c-selector-xpath, c-fields-xpaths] the identity constraints of the
/// vehicle example: keys and keyrefs read, each keyref's key resolved; a
/// keyref of fewer fields than its key, and a selector naming an
/// attribute, refused.
#[test]
fn identity_constraints_read() {
    let body = r#"<xs:element name="root"><xs:complexType><xs:sequence>
        <xs:element name="vehicle" maxOccurs="unbounded"><xs:complexType>
          <xs:attribute name="plateNumber" type="xs:integer"/><xs:attribute name="state" type="xs:string"/></xs:complexType></xs:element>
        </xs:sequence></xs:complexType>
        <xs:key name="regKey"><xs:selector xpath=".//vehicle"/><xs:field xpath="@state"/><xs:field xpath="@plateNumber"/></xs:key>
        <xs:keyref name="carRef" refer="regKey"><xs:selector xpath=".//car"/><xs:field xpath="@regState"/><xs:field xpath="@regPlate"/></xs:keyref>
        </xs:element>"#;
    let s = schema(body).unwrap();
    let keyref = s.constraint(s.constraint_names[&Expanded::new(None, "carRef")]).unwrap();
    assert_eq!(keyref.refer, Some(s.constraint_names[&Expanded::new(None, "regKey")]));
    let short = body.replace(r#"<xs:field xpath="@regPlate"/>"#, "");
    assert!(refused(&short).contains("c-props-correct.2"));
    let attribute = body.replace(r#"xpath=".//car""#, r#"xpath="@car""#);
    assert!(refused(&attribute).contains("c-selector-xpath"));
}

/// Verifies: [EXTENSION] [Schema1 §3.11.6] the XPath subset: selectors of
/// child steps after an optional `.//`, unions, `*` and `p:*`, the child
/// axis written out; fields ending in an attribute; nothing else.
#[test]
fn the_xpath_subset_parses() {
    let scope = [(Some("p"), "urn:p")];
    let paths = xpath::parse(".//p:a/b | child::c/*", false, &scope).unwrap();
    assert_eq!(paths.len(), 2);
    assert!(paths[0].descendants);
    assert_eq!(paths[0].steps[0], Step::Child(NameTest::Name(Expanded::new(Some("urn:p"), "a"))));
    assert_eq!(paths[1].steps[1], Step::Child(NameTest::Any));
    let field = xpath::parse("a / @ p:* ", true, &scope).unwrap();
    assert_eq!(field[0].attribute, Some(NameTest::Namespace("urn:p".into())));
    assert_eq!(xpath::parse("attribute::x", true, &scope).unwrap()[0].attribute, Some(NameTest::Name(Expanded::new(None, "x"))));
    for bad in ["@x", "a/@x/b", "../a", "a[1]", "q:a", "//a", ""] {
        assert!(xpath::parse(bad, bad.contains('@') && bad.ends_with('b'), &scope).is_none(), "{bad}");
    }
}
