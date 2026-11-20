//! Tests of assessing instance documents against schemas [Schema1 §3.2.4,
//! §3.3.4, §3.4.4]: the attributes of the XML Schema instance namespace,
//! element values and attribute wildcards.

use super::*;
use crate::xml::{Item, Node, Parameters, Sequence, Version, serialize};

/// How a valid document is written.
const TEXT: Parameters = Parameters { version: Version::V10, declaration: false, encoding: "UTF-8", undeclare: false };

/// What assessing the document `instance` against the schema of `body`
/// strictly gives: `valid` and its text, or the rule it breaks.
fn assessed(body: &str, instance: &str) -> String {
    let s = schema(body).unwrap();
    let node = Node::root(parse(instance, Syntax::Document, Whitespace::Preserve).unwrap());
    match validate(&s, &node, Mode::Strict).unwrap() {
        Ok(valid) => format!("valid {}", serialize(&Sequence::new(vec![Item::Node(valid)]), &TEXT).unwrap()),
        Err(failure) => format!("{} {}", failure.rule, failure.why),
    }
}

/// The declaration of the prefix `xsi`.
const XSI: &str = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;

/// The declaration of the prefix `xs` in an instance.
const XS_IN: &str = r#"xmlns:xs="http://www.w3.org/2001/XMLSchema""#;

/// Checks each instance of `cases` assessed against the schema of `body`
/// gives the outcome paired with it, or one beginning with it.
fn each(body: &str, cases: &[(String, &str)]) {
    for (instance, expected) in cases {
        let outcome = assessed(body, instance);
        assert!(outcome.starts_with(expected), "{instance}: {outcome}");
    }
}

/// An element `r` of an attribute wildcard of `process`, and a global
/// attribute `g` of `xs:int`.
fn wildcard(process: &str) -> String {
    format!(
        r#"<xs:element name="r"><xs:complexType><xs:anyAttribute{process}/></xs:complexType></xs:element><xs:attribute name="g" type="xs:int"/>"#
    )
}

/// Verifies: [EXTENSION] [Schema1 §3.4.4 cvc-complex-type.3.2.2]
/// [Schema1 §3.10.4, §3.2.4 cvc-assess-attr.1, cvc-attribute.3] an
/// attribute an attribute wildcard allows is assessed by its global
/// declaration, if processContents is not skip; strictly, one of no
/// declaration is not valid; laxly or skipped, it is kept as it is.
#[test]
fn attribute_wildcards_assess_by_their_process() {
    each(
        &wildcard(r#" processContents="lax""#),
        &[("<r g='1' h='x'/>".into(), r#"valid <r g="1" h="x"/>"#), ("<r g='x'/>".into(), "[Schema1 §3.2.4 cvc-attribute.3]")],
    );
    each(&wildcard(r#" processContents="skip""#), &[("<r g='x' h='y'/>".into(), r#"valid <r g="x" h="y"/>"#)]);
    each(&wildcard(""), &[("<r g='1'/>".into(), r#"valid <r g="1"/>"#), ("<r h='1'/>".into(), "[Schema1 §3.2.4 cvc-assess-attr.1]")]);
}

/// Verifies: [EXTENSION] [Schema1 §3.2.7, §3.3.4 cvc-elt.3, cvc-elt.4,
/// cvc-type.3.1.1] the attributes of the XML Schema instance namespace:
/// xsi:nil a boolean, of a nillable element, which then holds no element
/// or character; xsi:type a QName in scope naming a type derived from
/// the declared one; xsi:schemaLocation and xsi:noNamespaceSchemaLocation
/// allowed on an element of a simple type, any other attribute not.
#[test]
fn instance_attributes_are_assessed() {
    let body = r#"<xs:element name="n" type="xs:int" nillable="true"/><xs:element name="m" type="xs:int"/>"#;
    each(
        body,
        &[
            (format!("<n {XSI} xsi:nil='maybe'/>"), "[Schema1 §3.3.4 cvc-elt.3.2] xsi:nil 'maybe' is no boolean"),
            (format!("<n {XSI} xsi:nil='true'/>"), "valid"),
            (format!("<n {XSI} xsi:nil='true'>1</n>"), "[Schema1 §3.3.4 cvc-elt.3.2.1]"),
            (format!("<m {XSI} xsi:nil='true'/>"), "[Schema1 §3.3.4 cvc-elt.3.1] m is not nillable"),
            (format!("<m {XSI} xsi:noNamespaceSchemaLocation='x.xsd'>1</m>"), "valid"),
            (format!("<m {XSI} xsi:schemaLocation='a b'>1</m>"), "valid"),
            (format!("<m {XSI} {XS_IN} xsi:type='xs:short'>1</m>"), "valid"),
            (format!("<m {XSI} {XS_IN} xsi:type='xs:string'>1</m>"), "[Schema1 §3.3.4 cvc-elt.4.3]"),
            (format!("<m {XSI} xsi:type='q:x'>1</m>"), "[Schema1 §3.3.4 cvc-elt.4.1]"),
            ("<m h='1'>1</m>".into(), "[Schema1 §3.3.4 cvc-type.3.1.1] an element of a simple type has no attribute h"),
        ],
    );
}

/// Verifies: [EXTENSION] [Schema1 §3.3.4 cvc-elt.5, §3.4.4] an empty
/// element of a default or fixed value takes it; one of other content
/// than its fixed value is not valid; an element of `xs:anyType` holds
/// any attributes and elements, laxly assessed, and may be given a type
/// by xsi:type, but is not nillable.
#[test]
fn element_values_and_the_ur_type_are_assessed() {
    let body = r#"<xs:element name="d" type="xs:int" default="5"/><xs:element name="f" type="xs:int" fixed="5"/>
        <xs:element name="a"/><xs:element name="i" type="xs:int"/>"#;
    each(
        body,
        &[
            ("<d/>".into(), "valid <d>5</d>"),
            ("<f/>".into(), "valid <f>5</f>"),
            ("<f>6</f>".into(), "[Schema1 §3.3.4 cvc-elt.5.2.2.2.2]"),
            (format!("<a {XSI} xsi:nil='x'/>"), "[Schema1 §3.3.4 cvc-elt.3.1] a is not nillable"),
            ("<a h='1'><b c='2'>t</b></a>".into(), r#"valid <a h="1"><b c="2">t</b></a>"#),
            ("<a><i>x</i></a>".into(), "[Schema1 §3.3.4 cvc-type.3.1.3]"),
            (format!("<a {XSI} {XS_IN} xsi:type='xs:int'>1</a>"), "valid"),
        ],
    );
}
