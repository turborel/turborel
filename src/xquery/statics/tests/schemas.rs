//! Tests of the static typing of values of the types of a schema: its
//! content models' particles, compositors and occurrences, simple
//! content, lists and unions, nillable elements and wildcards.

use std::sync::Arc;

use super::super::{FormalKind, Typed};
use super::{formal, outcome};
use crate::xml::schema::{Library, Schema, Source, read};
use crate::xml::{Syntax, Whitespace, parse};
use crate::xquery::ast::Occurrence;

/// A schema of target namespace `urn:s`, of a global element `root` of
/// each kind of particle and content, among them `open`, of a wildcard
/// alone, and a global `other` string.
const SHAPES: &str = r###"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:s="urn:s" targetNamespace="urn:s" elementFormDefault="qualified">
<xs:simpleType name="nums"><xs:list itemType="xs:int"/></xs:simpleType>
<xs:simpleType name="either"><xs:union memberTypes="xs:int xs:boolean"/></xs:simpleType>
<xs:complexType name="pair"><xs:all><xs:element name="x" type="xs:int"/><xs:element name="y" type="xs:int" minOccurs="0"/></xs:all></xs:complexType>
<xs:element name="root"><xs:complexType><xs:sequence>
<xs:element name="tokens" type="xs:NMTOKENS"/>
<xs:element name="any" type="xs:anyType"/>
<xs:element name="simple" type="xs:anySimpleType"/>
<xs:element name="price" nillable="true"><xs:complexType><xs:simpleContent><xs:extension base="xs:decimal"><xs:attribute name="cur" type="xs:string"/></xs:extension></xs:simpleContent></xs:complexType></xs:element>
<xs:choice><xs:element name="c1" type="xs:int"/><xs:element name="c2" type="xs:string"/></xs:choice>
<xs:element name="never" type="xs:int" minOccurs="0" maxOccurs="0"/>
<xs:element name="many" type="xs:int" minOccurs="2" maxOccurs="unbounded"/>
<xs:element name="nums" type="s:nums"/>
<xs:element name="either" type="s:either"/>
<xs:element name="pair" type="s:pair"/>
<xs:element name="empty"><xs:complexType/></xs:element>
<xs:element name="open"><xs:complexType><xs:sequence><xs:any namespace="##any" processContents="skip" maxOccurs="unbounded"/></xs:sequence></xs:complexType></xs:element>
</xs:sequence></xs:complexType></xs:element>
<xs:element name="other" type="xs:string"/>
</xs:schema>"###;

/// SHAPES read.
fn shapes() -> Arc<Schema> {
    let tree = Arc::new(parse(SHAPES, Syntax::Document, Whitespace::Preserve).unwrap());
    Arc::new(read(&Source { location: None, tree }, &Library::default()).unwrap())
}

/// Each of `cases`, a query in which `s` is bound to `urn:s` and its
/// outcome, compiled with a context item of `kind`, one.
fn each_of(kind: FormalKind, cases: &[(&str, &str)]) {
    let context = formal(kind, Occurrence::One);
    for (query, expected) in cases {
        let text = format!("declare namespace s = \"urn:s\"; {query}");
        assert_eq!(outcome(&text, Some(context.clone()), &[]), *expected, "{query}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.21 SR4, SR5c] [XQueryFS
/// §8.1.9, §8.1.10, Appendix C.1.4, C.2] a document of a schema's global
/// element is typed by its content model: a choice's elements optional, a
/// particle of no occurrence none, of many a sequence of them, an all
/// group's elements of their own occurrences; an element only a wildcard
/// admits is of `xs:anyType`, any name it allows; every axis over them.
#[test]
fn content_models_are_typed_by_their_particles() {
    let schema = shapes();
    each_of(
        FormalKind::Document(Typed::Schema(schema, Some("urn:s".into()), Some("root".into()))),
        &[
            ("./s:root/s:c1 + 1", "ok"),
            ("upper-case(./s:root/s:c2)", "ok"),
            ("./s:root/s:never", "err:XPST0005"),
            ("./s:root/s:many + 1", "err:XPTY0004"),
            ("sum(./s:root/s:many)", "ok"),
            ("./s:root/s:pair/s:x + ./s:root/s:pair/s:y", "ok"),
            ("./s:root/s:pair/s:z", "err:XPST0005"),
            ("./s:root/s:other", "err:XPST0005"),
            ("./s:other", "err:XPST0005"),
            ("./s:root/s:open/s:z", "ok"),
            ("./s:root/s:open/s:c1 + 1", "err:XPTY0004"),
            ("./s:root/s:open/*/s:deep", "ok"),
            ("//s:x", "ok"),
            ("./s:root/s:pair/ancestor::s:root", "ok"),
            ("./s:root/s:pair/following-sibling::s:empty", "ok"),
            ("./s:root/s:pair/preceding::s:c1", "ok"),
            ("./s:root/s:pair/(s:x, s:y)", "ok"),
            ("./s:root/element(s:pair)", "ok"),
            ("./s:root/element(*, xs:int)", "ok"),
            ("./s:root/self::element(s:root)", "ok"),
            (". instance of document-node(element(s:root))", "ok"),
            ("./s:root/attribute()", "ok"),
            ("./s:root/@xsi:nil", "ok"),
        ],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.21 SR5c] [XQueryFS §8.1.7,
/// §7.2.6, Appendix C.1.4] typed values of each kind of content: a
/// built-in list's its items', a list's and a union's their items' and
/// members', simple content's its type's with its attributes, nillable
/// optional, `xs:anyType`'s and `xs:anySimpleType`'s any atomic values,
/// empty content's none, so err:XPST0005.
#[test]
fn simple_content_lists_and_unions_are_typed() {
    let schema = shapes();
    each_of(
        FormalKind::Document(Typed::Schema(schema, Some("urn:s".into()), Some("root".into()))),
        &[
            ("sum(./s:root/s:tokens)", "err:XPTY0004"),
            ("data(./s:root/s:tokens) instance of xs:NMTOKEN*", "ok"),
            ("sum(./s:root/s:nums)", "ok"),
            ("./s:root/s:either + 1", "err:XPTY0004"),
            ("./s:root/s:price + 1", "ok"),
            ("./s:root/s:price/@cur", "ok"),
            ("./s:root/s:price/text()", "ok"),
            ("./s:root/s:price/@xsi:nil", "ok"),
            ("./s:root/s:price/s:x", "err:XPST0005"),
            ("./s:root/s:any/s:whatever", "ok"),
            ("./s:root/s:any + 1", "err:XPTY0004"),
            ("data(./s:root/s:simple)", "ok"),
            ("./s:root/s:simple + 1", "err:XPTY0004"),
            ("data(./s:root/s:empty)", "err:XPST0005"),
            ("./s:root/s:empty/node()", "ok"),
        ],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.21 SR4b, SR5f] [XQueryFS
/// §8.1.9] a value of XML(CONTENT(XMLSCHEMA)) of no element is of the
/// schema's global elements, each of its own type.
#[test]
fn content_of_a_schema_is_of_its_global_elements() {
    each_of(
        FormalKind::Content(Typed::Schema(shapes(), None, None)),
        &[("./s:root/s:pair", "ok"), ("./s:other", "ok"), ("./s:other + 1", "err:XPTY0004"), ("./s:none", "err:XPST0005"), ("./*", "ok")],
    );
}
