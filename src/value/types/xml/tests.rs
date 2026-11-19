//! Tests of the XML types' kind and of XML values.

use std::sync::Arc;

use super::*;
use crate::bytes::Reader;
use crate::error::SqlState;
use crate::value::lob::Lob;
use crate::xml::{Atomic, AtomicType, Builder, Item, MAX_STORED, Node, QName, Sequence, Specified, Syntax, Whitespace, parse};

/// The XML types, each.
const TYPES: [XmlType; 5] = [
    XmlType::Sequence,
    XmlType::Document(Typing::Untyped),
    XmlType::Document(Typing::Any),
    XmlType::Content(Typing::Untyped),
    XmlType::Content(Typing::Any),
];

/// What a type of XMLSCHEMA of registered XML Schema A.`name` indicates,
/// of `namespace` and `element` if given.
fn named(name: &str, namespace: Option<&str>, element: Option<&str>) -> Typing {
    Typing::Schema(Arc::new(XmlSchemaNamed {
        schema: "A".into(),
        name: name.into(),
        namespace: namespace.map(Into::into),
        element: element.map(Into::into),
    }))
}

/// Types of XMLSCHEMA: of A.S, of its namespace urn:s, of its element e,
/// of no namespace, and of A.T.
fn schema_types() -> Vec<XmlType> {
    vec![
        XmlType::Document(named("S", None, None)),
        XmlType::Content(named("S", Some("urn:s"), None)),
        XmlType::Document(named("S", Some("urn:s"), Some("e"))),
        XmlType::Content(named("S", Some(""), None)),
        XmlType::Content(named("T", None, None)),
    ]
}

fn content(text: &str) -> Value {
    Value::Xml(XmlValue::of(Sequence::document(parse(text, Syntax::Content, Whitespace::Preserve).unwrap())))
}

fn atomic(lexical: &str) -> Value {
    Value::Xml(XmlValue::of(Sequence::new(vec![Item::Atomic(Atomic::new(AtomicType::String, lexical))])))
}

/// An attribute standing alone, which no text serializes.
fn lone_attribute() -> Value {
    let mut b = Builder::parentless();
    b.attribute(&Specified { name: QName::local("a"), value: "v".into(), is_id: false, is_idrefs: false }).unwrap();
    let tree = Arc::new(b.finish().unwrap());
    Value::Xml(XmlValue::of(Sequence::new(vec![Item::Node(Node::new(tree, crate::xml::NodeId::ROOT))])))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.2, §6.1] [DESIGN] RULES.md
/// D106: each XML type is written with its modifiers, its parameters read
/// back as written, others corrupt; it is comparable with XML types
/// alone, described as XML, of no width and no key.
#[test]
fn the_types_6_1() {
    let written: Vec<String> = TYPES.iter().map(|t| XML.written(&DataType::Xml(t.clone()))).collect();
    assert_eq!(written, ["XML(SEQUENCE)", "XML(DOCUMENT(UNTYPED))", "XML(DOCUMENT(ANY))", "XML(CONTENT(UNTYPED))", "XML(CONTENT(ANY))"]);
    for t in TYPES.into_iter().chain(schema_types()) {
        let mut params = Vec::new();
        XML.put_params(&DataType::Xml(t.clone()), &mut params);
        assert_eq!(XML.params(&mut Reader::new(&params, "parameters")).unwrap(), DataType::Xml(t.clone()));
        assert!(XML.comparable(&DataType::Xml(t.clone())));
        assert_eq!(XML.descriptor(&DataType::Xml(t.clone())).name, "XML");
        assert_eq!(XML.record_width(&DataType::Xml(t)), None);
    }
    for bad in [&[3_u8, 0][..], &[0, 3], &[2, 1], &[0, 2, 1, b'A', 1, b'S', 2], &[0, 2, 1, b'A']] {
        assert_eq!(XML.params(&mut Reader::new(bad, "parameters")).unwrap_err().sqlcode(), -916_i32, "{bad:?}");
    }
    assert!(!XML.comparable(&DataType::varying(9)));
    assert_eq!(XML.name(), "XML");
    assert_eq!(DataType::Xml(XmlType::Sequence).lob_kind(), Some(LobKind::Binary));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.4, §10.2 SR1b] which XML
/// types a value of each is assignable to: an UNTYPED one, of an UNTYPED
/// type alone; one of XMLSCHEMA, of no other; any other, of any.
#[test]
fn assignability_4_2_4() {
    let untyped = [XmlType::Document(Typing::Untyped), XmlType::Content(Typing::Untyped)];
    for target in TYPES {
        for source in TYPES.into_iter().chain(schema_types()) {
            let expected = !untyped.contains(&target) || untyped.contains(&source);
            assert_eq!(target.takes(&source), expected, "{} from {}", target.written(), source.written());
        }
    }
    for target in schema_types() {
        assert!(TYPES.iter().all(|source| !target.takes(source)), "{}", target.written());
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.2 SR1b ii, §4.2.4, §4.2.6] a
/// type of XMLSCHEMA takes a value of one of the same registered XML
/// Schema, identified by its name, of the namespace it indicates if any
/// [SR1b ii 2], and of its element if any [SR1b ii 1]: so DOCUMENT or
/// CONTENT alike; no other.
#[test]
fn assignability_of_xmlschema_types_10_2() {
    let [whole, namespace, element, no_namespace, other] = schema_types().try_into().unwrap();
    let takes = |t: &XmlType, s: &XmlType| t.takes(s);
    assert!(takes(&whole, &namespace) && takes(&whole, &element) && takes(&whole, &no_namespace));
    assert!(takes(&namespace, &element) && !takes(&namespace, &whole) && !takes(&namespace, &no_namespace));
    assert!(takes(&element, &element) && !takes(&element, &namespace));
    assert!(!takes(&whole, &other) && !takes(&other, &whole));
    let content = XmlType::Content(named("S", None, None));
    assert!(takes(&content, &whole) && takes(&whole, &content));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.1 GR1, §11.6, §21.1
/// Description 1a] a type of XMLSCHEMA is written as an `<XML valid
/// according to clause>` indicates its registered XML Schema by ID, its
/// namespace and element as an element clause does, NO NAMESPACE of the
/// zero-length one; its descriptor holds them.
#[test]
fn xmlschema_types_written_and_described_6_1() {
    let written: Vec<String> = schema_types().into_iter().map(|t| t.written()).collect();
    assert_eq!(
        written,
        [
            "XML(DOCUMENT(XMLSCHEMA ID A.S))",
            "XML(CONTENT(XMLSCHEMA ID A.S NAMESPACE 'urn:s'))",
            "XML(DOCUMENT(XMLSCHEMA ID A.S NAMESPACE 'urn:s' ELEMENT \"e\"))",
            "XML(CONTENT(XMLSCHEMA ID A.S NO NAMESPACE))",
            "XML(CONTENT(XMLSCHEMA ID A.T))",
        ]
    );
    let d = XML.descriptor(&DataType::Xml(schema_types()[2].clone()));
    assert_eq!(d.xml, Some(("DOCUMENT", Some("XMLSCHEMA"))));
    let named = d.xml_schema.unwrap();
    assert_eq!(
        (named.schema, named.name, named.namespace, named.element),
        ("A".into(), "S".into(), Some("urn:s".into()), Some("e".into()))
    );
    assert!(XML.descriptor(&DataType::Xml(XmlType::Sequence)).xml_schema.is_none());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.3] [DESIGN] RULES.md D106: a
/// site of a type of XMLSCHEMA takes a value each of whose elements a
/// validation typed, or one a table holds stored, and the null value; a
/// value untyped, as one sent as text, is SQLSTATE 2201R.
#[test]
fn a_site_of_xmlschema_takes_what_validation_made() {
    let t = DataType::Xml(XmlType::Content(named("S", None, None)));
    assert_eq!(XML.assign(content("<a/>"), &t, "test").unwrap_err().sqlstate(), SqlState::VALIDATION_FAILURE);
    assert_eq!(XML.cast(Value::Xml(XmlValue::serialized("<a/>")), &t).unwrap_err().sqlstate(), SqlState::VALIDATION_FAILURE);
    assert!(XML.assign(content("<!--no element-->"), &t, "test").is_ok(), "nothing to type");
    let stored = XmlValue::stored(Lob::of_octets(&crate::xml::encode(&Sequence::document(
        parse("<a/>", Syntax::Content, Whitespace::Preserve).unwrap(),
    ))));
    assert!(XML.assign(Value::Xml(stored), &t, "test").is_ok());
    assert!(XML.assign(Value::Null, &t, "test").unwrap().is_null());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.5 GR1a, §10.2 GR1a, §4.2.4]
/// assignment and CAST to an XML type: a DOCUMENT type takes a document
/// alone, else 2200L; a CONTENT type one document node alone, else 2200U;
/// SEQUENCE any; the null value is null; no other value; XML values do
/// not compare and are in no bucket or key.
#[test]
fn assignment_and_cast_10_2() {
    let document = DataType::Xml(XmlType::Document(Typing::Any));
    let content_type = DataType::Xml(XmlType::Content(Typing::Any));
    assert!(XML.assign(content("<a/>"), &document, "test").is_ok());
    assert_eq!(XML.assign(content("<a/><b/>"), &document, "test").unwrap_err().sqlstate(), SqlState::NOT_AN_XML_DOCUMENT);
    assert_eq!(XML.cast(content("t"), &document).unwrap_err().sqlstate(), SqlState::NOT_AN_XML_DOCUMENT);
    assert_eq!(XML.assign(atomic("1"), &content_type, "test").unwrap_err().sqlstate(), SqlState::NOT_AN_XQUERY_DOCUMENT_NODE);
    assert!(XML.assign(atomic("1"), &DataType::Xml(XmlType::Sequence), "test").is_ok());
    assert!(XML.assign(Value::Null, &document, "test").unwrap().is_null());
    assert!(XML.cast(Value::Null, &document).unwrap().is_null());
    assert!(XML.assign(Value::Char("<a/>".into()), &document, "test").is_err());
    assert!(XML.cast(Value::Char("<a/>".into()), &document).is_err());
    let v = content("<a/>");
    assert!(XML.compare(&v, &v).is_none() && XML.bucket(&v).is_none());
    assert!(XML.position(&v).is_none() && XML.anchor(&v, &document).is_none());
    assert_eq!(XML.encode_key(&v, &document, &mut Vec::new()).unwrap_err().sqlcode(), -910_i32);
    assert_eq!(XML.decode_key(&mut &[][..], &document).unwrap_err().sqlcode(), -910_i32);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.3] [DESIGN] RULES.md D106: a
/// value's bytes and record are its stored form as a binary large
/// object's, read back as the same value; a record of another value is
/// refused, and bytes that are none read as none.
#[test]
fn stored_forms_read_back() {
    let t = DataType::Xml(XmlType::Sequence);
    for v in [content("<a x=\"1\">t</a>"), atomic("x y"), Value::Xml(XmlValue::serialized("s<b/>"))] {
        let mut bytes = Vec::new();
        XML.put_value(&v, &mut bytes);
        let read = XML.value(&mut Reader::new(&bytes, "value")).unwrap();
        assert_eq!(XML.text(&read), XML.text(&v));
        let record = XML.encode_record(&v, &t).unwrap();
        assert_eq!(XML.text(&XML.decode_record(&record, &t).unwrap()), XML.text(&v));
    }
    assert!(XML.encode_record(&Value::Char("x".into()), &t).is_err());
    assert!(XML.decode_record(&[9], &t).is_none());
    let mut null = Vec::new();
    XML.put_value(&Value::Null, &mut null);
    assert!(XML.value(&mut Reader::new(&null, "value")).is_ok());
    let stored = XmlValue::stored(Lob::of_octets(&[1, 0, 0, 0, 0]));
    assert_eq!(stored.sequence().unwrap().items().len(), 0);
    assert!(XmlValue::stored(Lob::of_octets(&[7])).sequence().is_err());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.15] [EXTENSION] RULES.md D106:
/// a value is shown, and is a literal, as its serialization as CONTENT,
/// in XML 1.0, else XML 1.1 after a declaration; one with none says so,
/// and its text is 2200W.
#[test]
fn values_as_text_10_15() {
    assert_eq!(XML.text(&content("<a/>it's")), "<a/>it's");
    assert_eq!(XML.literal(&content("<a/>it's")), "XMLPARSE(CONTENT '<a/>it''s' PRESERVE WHITESPACE)");
    let v11 = Value::Xml(XmlValue::of(Sequence::document(
        parse("<?xml version='1.1'?><a>&#1;</a>", Syntax::Content, Whitespace::Preserve).unwrap(),
    )));
    assert_eq!(XML.text(&v11), "<?xml version=\"1.1\" encoding=\"UTF-8\"?><a>&#x1;</a>");
    let Value::Xml(back) =
        XML.cast(Value::Xml(XmlValue::serialized(&XML.text(&v11))), &DataType::Xml(XmlType::Content(Typing::Any))).unwrap()
    else {
        panic!("an XML value")
    };
    assert!(back.sequence().unwrap().is_content(), "its text parses back as CONTENT");
    let lone = lone_attribute();
    assert_eq!(XML.text(&lone), "(an XML value with no serialization)");
    assert_eq!(XML.literal(&lone), "XMLPARSE(CONTENT '' PRESERVE WHITESPACE)");
    let Value::Xml(x) = lone else { panic!("an XML value") };
    assert_eq!(x.text().unwrap_err().sqlstate(), SqlState::XQUERY_SERIALIZATION_ERROR);
    assert_eq!(content("<a/>").to_string(), "<a/>");
}

/// Verifies: [DESIGN] RULES.md D18, D106: a value whose stored form is
/// longer than `MAX_STORED` octets is refused as it is stored, SQLCODE
/// -900; its forms say what they hold, a table's large object or not.
#[test]
fn forms_and_bounds() {
    let held = XmlValue::of(Sequence::default());
    assert!(held.stored_lob().is_none() && held.held_bytes() == 0);
    let text = XmlValue::serialized("<a/>");
    assert_eq!((text.octets(), text.held_bytes()), (b"<a/>".to_vec(), 4));
    let mut stored = XmlValue::stored(Lob::of_octets(&crate::xml::encode(&Sequence::default())));
    assert!(stored.stored_lob().is_some() && stored.stored_lob_mut().is_some());
    assert_eq!(stored.octets(), crate::xml::encode(&Sequence::default()));
    let mut value = Value::Xml(stored.clone());
    assert!(value.as_lob().is_some() && value.as_lob_mut().is_some() && value.is_lob_stored());
    assert!(Value::Xml(held).as_lob().is_none());
    let huge = XmlValue::of(Sequence::new(vec![Item::Atomic(Atomic::new(AtomicType::String, "x".repeat(MAX_STORED)))]));
    assert_eq!(huge.lob().unwrap_err().sqlcode(), -900_i32);
}
