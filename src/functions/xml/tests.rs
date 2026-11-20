//! Unit tests of the XML functions (`xml.rs`): the types they report,
//! and their limits and refusals.

use super::*;
use crate::value::xml::{IMPLICIT, Typing, XmlType};

fn parser(syntax: Syntax) -> XmlParse {
    XmlParse { syntax, whitespace: Whitespace::Strip, binary: false }
}

fn serializer(target: DataType) -> XmlSerialize {
    XmlSerialize(Serialization { syntax: Syntax::Content, target, binary: false, utf16: false, version: Version::V10, declaration: None })
}

/// Verifies: [DESIGN] RULES.md D68, each XML function, bound as its
/// syntax is read, reports its
/// name and type: XMLPARSE an untyped XML(DOCUMENT) or XML(CONTENT) as
/// its syntax is, IS DOCUMENT and IS CONTENT Boolean, XMLDOCUMENT the type
/// bound, XMLCOPY its argument's or the implicit type, XMLSERIALIZE its
/// target.
#[test]
fn each_function_reports_its_bound_type() {
    let document = DataType::Xml(XmlType::Document(Typing::Untyped));
    let content = DataType::Xml(XmlType::Content(Typing::Untyped));
    assert_eq!(parser(Syntax::Document).return_type(&[]).unwrap(), document);
    assert_eq!(parser(Syntax::Content).return_type(&[]).unwrap(), content);
    assert_eq!((XmlIs(Syntax::Document).name(), XmlIs(Syntax::Content).name()), ("IS DOCUMENT", "IS CONTENT"));
    assert_eq!(XmlIs(Syntax::Content).return_type(&[]).unwrap(), DataType::Boolean);
    assert_eq!(XmlDocument(document.clone()).return_type(&[]).unwrap(), document);
    assert_eq!(XmlCopy.return_type(std::slice::from_ref(&content)).unwrap(), content);
    assert_eq!(XmlCopy.return_type(&[]).unwrap(), DataType::Xml(IMPLICIT));
    assert_eq!(serializer(DataType::varying(9)).return_type(&[]).unwrap(), DataType::varying(9));
}

/// Verifies: [DESIGN] RULES.md D15, XMLPARSE takes text of at most
/// turborel's limit of 64 MiB: longer text is refused with SQLCODE -900
/// before it is parsed; the text of an octet string is none, as it is
/// parsed by its encoding.
#[test]
fn text_past_the_limit_is_refused() {
    let long = Value::Char("a".repeat(MAX_INPUT.saturating_add(1)));
    let e = parser(Syntax::Content).call(&[long]).unwrap_err();
    assert!(e.code == Code::Unsupported && e.to_string().contains("beyond turborel's limits"), "{e}");
    assert_eq!(xml_text(&Value::Binary(Binary::new(b"<a/>".to_vec()))).unwrap(), None);
    assert_eq!(xml_text(&Value::Char("<a/>".to_owned())).unwrap().as_deref(), Some("<a/>"));
}

/// Verifies: [DESIGN] RULES.md D68, the XML functions are given only
/// what binding checked: XMLPARSE of a value not a string and
/// XMLSERIALIZE of a value not XML are refused with SQLCODE -910; of the
/// null value each gives the null value.
#[test]
fn a_value_binding_never_gives_is_refused() {
    assert_eq!(parser(Syntax::Content).call(&[Value::Bool(true)]).unwrap_err().code, Code::Api);
    let s = serializer(DataType::varying(9));
    assert_eq!(s.call(&[Value::Bool(true)]).unwrap_err().code, Code::Api);
    assert!(s.call(&[Value::Null]).unwrap().is_null() && parser(Syntax::Document).call(&[]).unwrap().is_null());
}
