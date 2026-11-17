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

fn content(text: &str) -> Value {
    Value::Xml(XmlValue::of(Sequence::document(parse(text, Syntax::Content, Whitespace::Preserve).unwrap())))
}

fn atomic(lexical: &str) -> Value {
    Value::Xml(XmlValue::of(Sequence::new(vec![Item::Atomic(Atomic { kind: AtomicType::String, lexical: lexical.into() })])))
}

/// An attribute standing alone, which no text serializes.
fn lone_attribute() -> Value {
    let mut b = Builder::parentless();
    b.attribute(&Specified { name: QName::local("a"), value: "v".into(), is_id: false, is_idrefs: false }).unwrap();
    let tree = Arc::new(b.finish().unwrap());
    Value::Xml(XmlValue::of(Sequence::new(vec![Item::Node(Node::new(tree, crate::xml::NodeId::ROOT))])))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.2, §6.1] [DESIGN] RULES.md
/// D86: each XML type is written with its modifiers, its parameters read
/// back as written, others corrupt; it is comparable with XML types
/// alone, described as XML, of no width and no key.
#[test]
fn the_types_6_1() {
    let written: Vec<String> = TYPES.iter().map(|&t| XML.written(&DataType::Xml(t))).collect();
    assert_eq!(written, ["XML(SEQUENCE)", "XML(DOCUMENT(UNTYPED))", "XML(DOCUMENT(ANY))", "XML(CONTENT(UNTYPED))", "XML(CONTENT(ANY))"]);
    for t in TYPES {
        let mut params = Vec::new();
        XML.put_params(&DataType::Xml(t), &mut params);
        assert_eq!(XML.params(&mut Reader::new(&params, "parameters")).unwrap(), DataType::Xml(t));
        assert!(XML.comparable(&DataType::Xml(t)));
        assert_eq!(XML.descriptor(&DataType::Xml(t)).name, "XML");
        assert_eq!(XML.record_width(&DataType::Xml(t)), None);
    }
    for bad in [[3_u8, 0], [0, 2], [2, 1]] {
        assert_eq!(XML.params(&mut Reader::new(&bad, "parameters")).unwrap_err().sqlcode(), -916_i32, "{bad:?}");
    }
    assert!(!XML.comparable(&DataType::varying(9)));
    assert_eq!(XML.name(), "XML");
    assert_eq!(DataType::Xml(XmlType::Sequence).lob_kind(), Some(LobKind::Binary));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.4, §10.2 SR1b] which XML
/// types a value of each is assignable to: an UNTYPED one, of an UNTYPED
/// type alone; any other, of any.
#[test]
fn assignability_4_2_4() {
    let untyped = [XmlType::Document(Typing::Untyped), XmlType::Content(Typing::Untyped)];
    for target in TYPES {
        for source in TYPES {
            let expected = !untyped.contains(&target) || untyped.contains(&source);
            assert_eq!(target.takes(source), expected, "{} from {}", target.written(), source.written());
        }
    }
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

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.3] [DESIGN] RULES.md D86: a
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

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.15] [EXTENSION] RULES.md D86:
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

/// Verifies: [DESIGN] RULES.md D18, D86: a value whose stored form is
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
    let huge = XmlValue::of(Sequence::new(vec![Item::Atomic(Atomic { kind: AtomicType::String, lexical: "x".repeat(MAX_STORED).into() })]));
    assert_eq!(huge.lob().unwrap_err().sqlcode(), -900_i32);
}
