//! Tests that a schema document not valid by the schema for schemas
//! [Schema1 Appendix A], or breaking a representation constraint of
//! [Schema1 §3], is refused with SQLSTATE 22000 naming what is wrong:
//! of each kind of element, an attribute of a value not of its type, a
//! name not resolved, text or a foreign element held, and the
//! constraints of its representation.

use super::*;

/// Checks each schema body of `cases` is refused, its message holding
/// the text paired with it.
fn each_refused(cases: &[(&str, &str)]) {
    for (body, expected) in cases {
        let message = refused(body);
        assert!(message.contains(expected), "{body}: {message}");
    }
}

/// A complex type `c` of a sequence of `particles`.
fn of_particles(particles: &str) -> String {
    format!(r#"<xs:complexType name="c"><xs:sequence>{particles}</xs:sequence></xs:complexType>"#)
}

/// Verifies: [EXTENSION] [Schema1 Appendix A] [Schema1 §3.3.3, §3.3.6,
/// §3.15.3] a global element declaration of a boolean, derivation set or
/// QName not of its type, of a substitution group or type not resolved,
/// of a default and a fixed value, of a value not of its type, of text, a
/// foreign element or two anonymous types, is refused.
#[test]
fn global_element_declarations_are_checked() {
    each_refused(&[
        (r#"<xs:element name="a" nillable="maybe"/>"#, "'maybe' is no value of nillable of <element>"),
        (r#"<xs:element name="a" abstract="maybe"/>"#, "'maybe' is no value of abstract of <element>"),
        (r#"<xs:element name="a" final="sideways"/>"#, "'sideways' is no value of final of <element>"),
        (r#"<xs:element name="a" block="sideways"/>"#, "'sideways' is no value of block of <element>"),
        (r#"<xs:element name="a" substitutionGroup="q:b"/>"#, "'q:b' is no QName whose prefix is in scope"),
        (r#"<xs:element name="a" substitutionGroup="b"/>"#, "b names no element declaration"),
        (r#"<xs:element name="a" type="q:t"/>"#, "'q:t' is no QName whose prefix is in scope"),
        (r#"<xs:element name="a" type="t"/>"#, "t names no type definition"),
        (r#"<xs:element name="a" default="x" fixed="y"/>"#, "src-element.1"),
        (r#"<xs:element name="a" type="xs:int" default="x"/>"#, "e-props-correct.2"),
        (r#"<xs:element name="a" type="xs:int" fixed="x"/>"#, "e-props-correct.2"),
        (r#"<xs:element name="a">text</xs:element>"#, "<element> holds text"),
        (r#"<xs:element name="a"><foo/></xs:element>"#, "<foo> is not of the XML Schema namespace"),
        (
            r#"<xs:element name="a" type="xs:int"><xs:simpleType><xs:restriction base="xs:int"/></xs:simpleType></xs:element>"#,
            "src-element.3",
        ),
        (
            r#"<xs:element name="a"><xs:simpleType><xs:restriction base="xs:int"/></xs:simpleType><xs:complexType/></xs:element>"#,
            "<element> has one <complexType> at most here",
        ),
        (r#"<xs:element name="1a"/>"#, "'1a' is no value of name of <element>"),
    ]);
}

/// Verifies: [EXTENSION] [Schema1 Appendix A] [Schema1 §3.3.3, §3.9.6]
/// a local element declaration of occurrences, a name, a form or a
/// nillable not of their types, of minOccurs past maxOccurs, of a ref not
/// resolved, of both or neither of name and ref, of a ref and content or
/// another property, of a value not of its type or of text, is refused.
#[test]
fn local_element_declarations_are_checked() {
    let b = r#"<xs:element name="b"/>"#;
    each_refused(&[
        (&of_particles(r#"<xs:element name="a" minOccurs="x"/>"#), "'x' is no value of minOccurs of <element>"),
        (&of_particles(r#"<xs:element name="a" maxOccurs="-1"/>"#), "'-1' is no value of maxOccurs of <element>"),
        (&of_particles(r#"<xs:element name="a" minOccurs="2" maxOccurs="1"/>"#), "p-props-correct.2"),
        (&of_particles(r#"<xs:element ref="q:b"/>"#), "'q:b' is no QName whose prefix is in scope"),
        (&of_particles(r#"<xs:element ref="b"/>"#), "b names no element declaration"),
        (&of_particles(r#"<xs:element name="1a"/>"#), "'1a' is no value of name of <element>"),
        (&of_particles(r#"<xs:element name="a" form="weird"/>"#), "'weird' is no value of form of <element>"),
        (&format!("{b}{}", of_particles(r#"<xs:element name="a" ref="b"/>"#)), "src-element.2.1"),
        (&of_particles("<xs:element/>"), "src-element.2.1"),
        (&format!("{b}{}", of_particles(r#"<xs:element ref="b"><xs:complexType/></xs:element>"#)), "src-element.2.2"),
        (&format!("{b}{}", of_particles(r#"<xs:element ref="b" nillable="true"/>"#)), "src-element.2.2"),
        (&of_particles(r#"<xs:element name="a" nillable="x"/>"#), "'x' is no value of nillable of <element>"),
        (&of_particles(r#"<xs:element name="a" type="xs:int" default="x"/>"#), "e-props-correct.2"),
        (&of_particles(r#"<xs:element name="a">text</xs:element>"#), "<element> holds text"),
    ]);
}

/// Verifies: [EXTENSION] [Schema1 Appendix A] [Schema1 §3.2.3, §3.2.6,
/// §3.5.6, §3.15.3] attribute declarations and uses, global and local: a
/// type not resolved or with an anonymous one, a value not of its type, a
/// default and a fixed value, text, a ref not resolved or with a form or
/// type, both or neither of name and ref, a use fixing another value, a
/// form, name or use not of its type, are refused.
#[test]
fn attribute_declarations_and_uses_are_checked() {
    let g = r#"<xs:attribute name="g"/>"#;
    let in_c = |inner: &str| format!(r#"<xs:complexType name="c">{inner}</xs:complexType>"#);
    each_refused(&[
        (r#"<xs:attribute name="a" type="q:t"/>"#, "'q:t' is no QName whose prefix is in scope"),
        (r#"<xs:attribute name="a" type="t"/>"#, "t names no type definition"),
        (
            r#"<xs:attribute name="a" type="xs:int"><xs:simpleType><xs:restriction base="xs:int"/></xs:simpleType></xs:attribute>"#,
            "src-attribute.4",
        ),
        (r#"<xs:attribute name="a" type="xs:int" default="x"/>"#, "a-props-correct.2"),
        (r#"<xs:attribute name="a" default="x" fixed="y"/>"#, "src-attribute.1"),
        (r#"<xs:attribute name="a">text</xs:attribute>"#, "<attribute> holds text"),
        (r#"<xs:attribute name="a"><xs:simpleType><xs:list itemType="q:x"/></xs:simpleType></xs:attribute>"#, "'q:x' is no QName"),
        (&in_c(r#"<xs:attribute ref="q:a"/>"#), "'q:a' is no QName whose prefix is in scope"),
        (&format!("{g}{}", in_c(r#"<xs:attribute ref="g" type="xs:int"/>"#)), "src-attribute.3.2"),
        (
            &format!(
                "{g}{}",
                in_c(r#"<xs:attribute ref="g"><xs:simpleType><xs:restriction base="xs:int"/></xs:simpleType></xs:attribute>"#)
            ),
            "src-attribute.3.2",
        ),
        (&in_c(r#"<xs:attribute ref="g"/>"#), "g names no attribute declaration"),
        (&in_c(r#"<xs:attribute name="a" form="x"/>"#), "'x' is no value of form of <attribute>"),
        (&in_c("<xs:attribute/>"), "src-attribute.3.1"),
        (&format!("{g}{}", in_c(r#"<xs:attribute name="a" ref="g"/>"#)), "src-attribute.3.1"),
        (
            &format!(r#"<xs:attribute name="f" type="xs:string" fixed="1"/>{}"#, in_c(r#"<xs:attribute ref="f" fixed="2"/>"#)),
            "au-props-correct.2",
        ),
        (&format!(r#"<xs:attribute name="i" type="xs:int"/>{}"#, in_c(r#"<xs:attribute ref="i" default="x"/>"#)), "a-props-correct.2"),
        (&in_c(r#"<xs:attribute name="1a"/>"#), "'1a' is no value of name of <attribute>"),
        (&in_c(r#"<xs:attribute name="a" type="q:t"/>"#), "'q:t' is no QName whose prefix is in scope"),
    ]);
}

/// Verifies: [EXTENSION] [Schema1 Appendix A] use is one of optional,
/// prohibited and required; any other value is refused. Regression test:
/// any other value was read as optional, so a schema not valid by the
/// schema for schemas was registered.
#[test]
fn an_attribute_use_of_no_value_of_use_is_refused() {
    let use_of = |value: &str| format!(r#"<xs:complexType name="c"><xs:attribute name="a" use="{value}"/></xs:complexType>"#);
    assert!(refused(&use_of("sometimes")).contains("'sometimes' is no value of use of <attribute>"));
    for value in ["optional", "prohibited", "required", " required "] {
        assert!(schema(&use_of(value)).is_ok(), "{value}");
    }
}

/// Verifies: [EXTENSION] [Schema1 Appendix A] [Schema1 §3.4.2, §3.6.2,
/// §3.15.3] a complex type definition of a derivation set or boolean not
/// of its type, of text, of complex content of a mixed not of its type,
/// of two derivations or none, of a base missing, not in scope or not
/// resolved, of an attribute group reference missing or not resolved, of
/// an attribute wildcard's processContents not of its type, is refused.
#[test]
fn complex_type_definitions_are_checked() {
    let c = |inner: &str| format!(r#"<xs:complexType name="c">{inner}</xs:complexType>"#);
    let complex = |inner: &str| c(&format!("<xs:complexContent>{inner}</xs:complexContent>"));
    let simple = |inner: &str| c(&format!("<xs:simpleContent>{inner}</xs:simpleContent>"));
    each_refused(&[
        (r#"<xs:complexType name="c" block="x"/>"#, "'x' is no value of block of <complexType>"),
        (r#"<xs:complexType name="c" final="x"/>"#, "'x' is no value of final of <complexType>"),
        (r#"<xs:complexType name="c" mixed="x"/>"#, "'x' is no value of mixed of <complexType>"),
        (r#"<xs:complexType name="c" abstract="x"/>"#, "'x' is no value of abstract of <complexType>"),
        (&c("text"), "<complexType> holds text"),
        ("<xs:complexType/>", "a top-level <complexType> has a name"),
        (
            &c(r#"<xs:complexContent mixed="x"><xs:extension base="xs:anyType"/></xs:complexContent>"#),
            "'x' is no value of mixed of <complexContent>",
        ),
        (&complex(r#"<xs:extension base="xs:anyType"/><xs:restriction base="xs:anyType"/>"#), "has one <restriction> at most here"),
        (&complex(""), "<complexContent> has one <restriction> or <extension>"),
        (&complex("<xs:extension/>"), "<extension> has a base"),
        (&complex(r#"<xs:extension base="q:t"/>"#), "'q:t' is no QName whose prefix is in scope"),
        (&complex(r#"<xs:extension base="t"/>"#), "t names no type definition"),
        (&complex(r#"<xs:extension base="xs:anyType">text</xs:extension>"#), "<extension> holds text"),
        (&complex(r#"text<xs:extension base="xs:anyType"/>"#), "<complexContent> holds text"),
        (&simple(r#"<xs:extension base="t"/>"#), "t names no type definition"),
        (&simple(r#"<xs:extension base="xs:int"/><xs:extension base="xs:int"/>"#), "has one <extension> at most here"),
        (&simple(r#"<xs:extension base="xs:int">text</xs:extension>"#), "<extension> holds text"),
        (&simple(r#"<xs:extension base="xs:int"><xs:attribute name="1a"/></xs:extension>"#), "'1a' is no value of name of <attribute>"),
        (&c("<xs:attributeGroup/>"), "a nested <attributeGroup> has a ref"),
        (&c(r#"<xs:attributeGroup ref="q:g"/>"#), "'q:g' is no QName whose prefix is in scope"),
        (&c(r#"<xs:attributeGroup ref="g"/>"#), "g names no attribute group"),
        (&c(r#"<xs:anyAttribute processContents="sometimes"/>"#), "'sometimes' is no value of processContents of <anyAttribute>"),
        (&c("<xs:anyAttribute>text</xs:anyAttribute>"), "<anyAttribute> holds text"),
        (r#"<xs:attributeGroup name="g">text</xs:attributeGroup>"#, "<attributeGroup> holds text"),
        (r#"<xs:attributeGroup name="g"><xs:attributeGroup/></xs:attributeGroup>"#, "a nested <attributeGroup> has a ref"),
        (r#"<xs:attributeGroup name="g"><xs:attribute name="1a"/></xs:attributeGroup>"#, "'1a' is no value of name of <attribute>"),
    ]);
    assert!(schema(&c(r#"<xs:anyAttribute processContents="skip"/>"#)).is_ok());
}

/// Verifies: [EXTENSION] [Schema1 Appendix A] [Schema1 §3.7.2, §3.8.2]
/// a wildcard's and a group reference's occurrences not of their types, a
/// group reference of no ref, a ref not in scope or not resolved, a model
/// group definition of two model groups or none, and text in a model
/// group, are refused.
#[test]
fn particles_and_model_groups_are_checked() {
    let g = r#"<xs:group name="g"><xs:sequence/></xs:group>"#;
    each_refused(&[
        (&of_particles(r#"<xs:any minOccurs="x"/>"#), "'x' is no value of minOccurs of <any>"),
        (&of_particles("<xs:group/>"), "a nested <group> has a ref"),
        (&of_particles(r#"<xs:group ref="q:g"/>"#), "'q:g' is no QName whose prefix is in scope"),
        (&of_particles(r#"<xs:group ref="g"/>"#), "g names no model group definition"),
        (&format!("{g}{}", of_particles(r#"<xs:group ref="g" minOccurs="x"/>"#)), "'x' is no value of minOccurs of <group>"),
        (r#"<xs:group name="g"><xs:sequence/><xs:choice/></xs:group>"#, "<group> has one <choice> at most here"),
        (r#"<xs:group name="g"/>"#, "a model group definition has one <all>, <choice> or <sequence>"),
        (r#"<xs:group name="g">text<xs:sequence/></xs:group>"#, "<group> holds text"),
        (r#"<xs:complexType name="c"><xs:choice>text</xs:choice></xs:complexType>"#, "<choice> holds text"),
        (&of_particles("<xs:sequence>text</xs:sequence>"), "<sequence> holds text"),
    ]);
}

/// Verifies: [EXTENSION] [Schema1 Appendix A] minOccurs and maxOccurs of
/// a type's empty model group are read, so refused if not of their types;
/// the model group of a model group definition has neither. Regression
/// test: both were never read, so a schema not valid by the schema for
/// schemas was registered.
#[test]
fn occurrences_of_empty_and_defined_model_groups_are_checked() {
    each_refused(&[
        (r#"<xs:complexType name="c"><xs:sequence minOccurs="x"/></xs:complexType>"#, "'x' is no value of minOccurs of <sequence>"),
        (r#"<xs:complexType name="c"><xs:all maxOccurs="y"/></xs:complexType>"#, "'y' is no value of maxOccurs of <all>"),
        (r#"<xs:group name="g"><xs:sequence minOccurs="x"/></xs:group>"#, "the <sequence> of a model group definition has no minOccurs"),
        (r#"<xs:group name="g"><xs:choice maxOccurs="2"/></xs:group>"#, "the <choice> of a model group definition has no maxOccurs"),
    ]);
    assert!(schema(r#"<xs:complexType name="c"><xs:sequence minOccurs="0"/></xs:complexType>"#).is_ok());
}

/// A simple type `s` of `derivation`.
fn simple_type(derivation: &str) -> String {
    format!(r#"<xs:simpleType name="s">{derivation}</xs:simpleType>"#)
}

/// A simple type `s` restricting `base` by `facets`.
fn faceted(base: &str, facets: &str) -> String {
    simple_type(&format!(r#"<xs:restriction base="{base}">{facets}</xs:restriction>"#))
}

/// Verifies: [EXTENSION] [Schema1 Appendix A] [Schema1 §3.14.2,
/// §3.14.3, §3.15.3] [Schema2 §4.1.3, §4.1.5] a simple type definition of
/// no derivation or two, of text, of a final not of its type, of a base
/// or item or member type missing, doubled, not in scope, not resolved or
/// not simple, of a list of lists or a union of no member, is refused.
#[test]
fn simple_type_definitions_are_checked() {
    let int = r#"<xs:simpleType><xs:restriction base="xs:int"/></xs:simpleType>"#;
    let unbound = r#"<xs:simpleType><xs:restriction base="q:x"/></xs:simpleType>"#;
    each_refused(&[
        (&simple_type("<xs:restriction/>"), "src-simple-type.2"),
        (&simple_type(&format!(r#"<xs:restriction base="xs:int">{int}</xs:restriction>"#)), "src-simple-type.2"),
        (&simple_type(r#"<xs:restriction base="q:t"/>"#), "'q:t' is no QName whose prefix is in scope"),
        (&simple_type(r#"<xs:restriction base="t"/>"#), "t names no type definition"),
        (&simple_type(r#"<xs:restriction base="xs:anyType"/>"#), "anyType is no simple type definition"),
        (&simple_type(r#"<xs:list itemType="q:x"/>"#), "'q:x' is no QName whose prefix is in scope"),
        (&simple_type(&format!(r#"<xs:list itemType="xs:int">{int}</xs:list>"#)), "src-simple-type.3"),
        (&simple_type("<xs:list/>"), "src-simple-type.3"),
        (
            &format!(
                r#"<xs:simpleType name="l"><xs:list itemType="xs:int"/></xs:simpleType>{}"#,
                simple_type(r#"<xs:list itemType="l"/>"#)
            ),
            "the item type l is a list",
        ),
        (&simple_type(&format!("<xs:list>{unbound}</xs:list>")), "'q:x' is no QName whose prefix is in scope"),
        (&simple_type(r#"<xs:union memberTypes="q:x"/>"#), "'q:x' is no QName whose prefix is in scope"),
        (&simple_type(r#"<xs:union memberTypes="x"/>"#), "x names no type definition"),
        (&simple_type("<xs:union/>"), "a union has no member type"),
        (&simple_type(&format!("<xs:union>{unbound}</xs:union>")), "'q:x' is no QName whose prefix is in scope"),
        (&simple_type(r#"<xs:union memberTypes="xs:anyType"/>"#), "anyType is no simple type definition"),
        (&simple_type(r#"<xs:restriction base="xs:int"/><xs:list itemType="xs:int"/>"#), "<simpleType> has one <list> at most here"),
        (&simple_type(""), "a <simpleType> has one <restriction>, <list> or <union>"),
        (
            r#"<xs:simpleType name="s" final="x"><xs:restriction base="xs:int"/></xs:simpleType>"#,
            "'x' is no value of final of <simpleType>",
        ),
        (&simple_type(r#"text<xs:restriction base="xs:int"/>"#), "<simpleType> holds text"),
    ]);
}

/// Verifies: [EXTENSION] [Schema2 Appendix A] [Schema1 Appendix A] a
/// facet of no value, of a count not a non-negative integer, of a
/// whiteSpace or fixed not of its type, or no facet at all, is refused;
/// each value of whiteSpace is read.
#[test]
fn facets_are_checked() {
    each_refused(&[
        (&faceted("xs:string", "<xs:length/>"), "<length> has a value"),
        (&faceted("xs:string", r#"<xs:minLength value="x"/>"#), "'x' is no value of value of <minLength>"),
        (&faceted("xs:string", r#"<xs:maxLength value="-1"/>"#), "'-1' is no value of value of <maxLength>"),
        (&faceted("xs:decimal", r#"<xs:totalDigits value="x"/>"#), "'x' is no value of value of <totalDigits>"),
        (&faceted("xs:decimal", r#"<xs:fractionDigits value="x"/>"#), "'x' is no value of value of <fractionDigits>"),
        (&faceted("xs:string", r#"<xs:whiteSpace value="sometimes"/>"#), "'sometimes' is no value of value of <whiteSpace>"),
        (&faceted("xs:string", r#"<xs:length value="1" fixed="x"/>"#), "'x' is no value of fixed of <length>"),
        (&faceted("xs:string", r#"<xs:bogus value="1"/>"#), "<restriction> has no <bogus> here"),
    ]);
    for value in ["preserve", "replace", "collapse"] {
        assert!(schema(&faceted("xs:string", &format!(r#"<xs:whiteSpace value="{value}"/>"#))).is_ok(), "{value}");
    }
    assert!(schema(&faceted("xs:int", r#"<xs:minInclusive value="1"/>"#)).is_ok());
}

/// Verifies: [DESIGN] RULES.md D18: a facet's count past what 64 bits
/// hold is beyond turborel's limits, SQLCODE -900.
#[test]
fn a_facet_count_past_64_bits_is_beyond_the_limits() {
    let e = schema(&faceted("xs:string", r#"<xs:length value="99999999999999999999999"/>"#)).unwrap_err();
    assert_eq!(e.sqlcode(), -900_i32, "{e}");
    assert!(e.to_string().contains("past what 64 bits hold"), "{e}");
}

/// An element `e` holding identity-constraint definitions `constraints`.
fn constrained(constraints: &str) -> String {
    format!(r#"<xs:element name="e">{constraints}</xs:element>"#)
}

/// Verifies: [EXTENSION] [Schema1 Appendix A] [Schema1 §3.11.1, §3.11.2,
/// §3.11.6, §3.12.1] identity-constraint definitions of no selector or
/// field, of a selector or field of no xpath or of one not of the subset,
/// of text, of a keyref of no refer, of one not in scope or not resolved,
/// of two of one name, of a name missing or not an NCName; a notation of
/// no identifier or of text, are refused.
#[test]
fn identity_constraints_and_notations_are_checked() {
    let paths = r#"<xs:selector xpath="."/><xs:field xpath="."/>"#;
    each_refused(&[
        (&constrained(r#"<xs:key name="k"/>"#), "an identity-constraint definition has one <selector>"),
        (&constrained(r#"<xs:key name="k"><xs:selector/><xs:field xpath="."/></xs:key>"#), "<selector> has an xpath"),
        (&constrained(r#"<xs:key name="k"><xs:selector xpath="."/><xs:field/></xs:key>"#), "<field> has an xpath"),
        (&constrained(r#"<xs:key name="k"><xs:selector xpath="."/><xs:field xpath="@"/></xs:key>"#), "c-fields-xpaths"),
        (&constrained(r#"<xs:key name="k"><xs:selector xpath="."/></xs:key>"#), "an identity-constraint definition has a <field>"),
        (&constrained(&format!(r#"<xs:key name="k">text{paths}</xs:key>"#)), "<key> holds text"),
        (&constrained(&format!(r#"<xs:keyref name="r">{paths}</xs:keyref>"#)), "a <keyref> has a refer"),
        (&constrained(&format!(r#"<xs:keyref name="r" refer="q:k">{paths}</xs:keyref>"#)), "'q:k' is no QName whose prefix is in scope"),
        (&constrained(&format!(r#"<xs:keyref name="r" refer="k">{paths}</xs:keyref>"#)), "k names no key or unique"),
        (
            &constrained(&format!(r#"<xs:key name="k">{paths}</xs:key><xs:unique name="k">{paths}</xs:unique>"#)),
            "two identity-constraint definitions are named k",
        ),
        (&constrained(&format!(r#"<xs:key name="1k">{paths}</xs:key>"#)), "'1k' is no value of name of <key>"),
        (&constrained(&format!("<xs:key>{paths}</xs:key>")), "a top-level <key> has a name"),
        (r#"<xs:notation name="n"/>"#, "notation n has a public or a system identifier"),
        (r#"<xs:notation name="n" public="p">text</xs:notation>"#, "<notation> holds text"),
    ]);
}

/// Verifies: [EXTENSION] [Schema1 Appendix A] [Schema1 §3.1.2, §4.2.2,
/// §4.2.3] a schema document of an include or redefine of no
/// schemaLocation, of an import of no namespace, of a redefinition naming
/// no registered document, of an element not of the schema for schemas
/// or not of its namespace, of text, or of a form or derivation default
/// not of its type, is refused.
#[test]
fn schema_documents_are_checked() {
    each_refused(&[
        ("<xs:include/>", "<include> has a schemaLocation"),
        ("<xs:import/>", "src-import.1.2"),
        ("<xs:redefine/>", "<redefine> has a schemaLocation"),
        (
            r#"<xs:redefine schemaLocation="nowhere.xsd"><xs:simpleType name="s"><xs:restriction base="s"/></xs:simpleType></xs:redefine>"#,
            "src-redefine.1",
        ),
        ("<xs:bogus/>", "<schema> has no <bogus> here"),
        ("<foo/>", "<foo> is not of the XML Schema namespace"),
        ("text", "<schema> holds text"),
    ]);
    for (attribute, expected) in [
        ("attributeFormDefault", "'x' is no value of attributeFormDefault of <schema>"),
        ("elementFormDefault", "'x' is no value of elementFormDefault of <schema>"),
        ("finalDefault", "'x' is no value of finalDefault of <schema>"),
        ("blockDefault", "'x' is no value of blockDefault of <schema>"),
    ] {
        let e = read(&source(&format!(r#"<xs:schema {XS} {attribute}="x"/>"#), None), &Library::default()).unwrap_err();
        assert_eq!(e.sqlstate(), SqlState::DATA_EXCEPTION, "{e}");
        assert!(e.to_string().contains(expected), "{attribute}: {e}");
    }
}
