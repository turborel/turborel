//! Tests of the scalar functions of SQL/XML as functions: the name each
//! is called by, the type it declares, and what it refuses of arguments
//! no statement the binder accepts gives it.

use std::sync::Arc;

use super::*;
use crate::value::xml::{IMPLICIT, Typing, XmlType};
use crate::xml::AtomicType;
use crate::xml::schema::SimpleType;

/// The name `f` is called by and the type it declares of `args`.
fn declared(f: &dyn ScalarFunction, args: &[DataType]) -> (String, DataType) {
    (f.name().to_owned(), f.return_type(args).unwrap())
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6, §6.11, §6.15, §6.16,
/// §6.17, §6.18, §8.2] [DESIGN] RULES.md R-SAFE-3: each scalar function
/// of SQL/XML is called by its key words and declares the type of its
/// result: XMLPARSE an untyped document or content, IS DOCUMENT and IS
/// CONTENT a boolean, XMLDOCUMENT and XMLCAST their target, XMLCOPY its
/// argument's, the publishing functions XML(SEQUENCE).
#[test]
fn xml_functions_name_themselves_and_their_types() {
    let parse_of = |syntax| XmlParse { syntax, whitespace: Whitespace::Strip, binary: false };
    assert_eq!(declared(&parse_of(Syntax::Document), &[]), ("XMLPARSE".to_owned(), DataType::Xml(XmlType::Document(Typing::Untyped))));
    assert_eq!(declared(&parse_of(Syntax::Content), &[]), ("XMLPARSE".to_owned(), DataType::Xml(XmlType::Content(Typing::Untyped))));
    assert_eq!(declared(&XmlIs(Syntax::Document), &[]), ("IS DOCUMENT".to_owned(), DataType::Boolean));
    assert_eq!(declared(&XmlIs(Syntax::Content), &[]), ("IS CONTENT".to_owned(), DataType::Boolean));
    let content = DataType::Xml(XmlType::Content(Typing::Any));
    assert_eq!(declared(&XmlDocument(content.clone()), &[]), ("XMLDOCUMENT".to_owned(), content.clone()));
    assert_eq!(declared(&XmlCopy, std::slice::from_ref(&content)), ("XMLCOPY".to_owned(), content.clone()));
    assert_eq!(declared(&XmlCopy, &[]), ("XMLCOPY".to_owned(), DataType::Xml(IMPLICIT)));
    let cast_in = XmlCastIn { target: XmlType::Sequence, base: AtomicType::Int, collection: None, encoding: BinaryEncoding::Base64 };
    assert_eq!(declared(&cast_in, &[]), ("XMLCAST".to_owned(), DataType::Xml(XmlType::Sequence)));
    let xmlt = Arc::new(SimpleType::built_in(AtomicType::Int));
    let cast_out = XmlCastOut { xmlt, adjust: None, fraction: None, given: DataType::Boolean };
    assert_eq!(declared(&cast_out, &[]), ("XMLCAST".to_owned(), DataType::Boolean));
    let publishing: [(&dyn ScalarFunction, &str); 4] =
        [(&XmlComment, "XMLCOMMENT"), (&XmlText(BinaryEncoding::Hex), "XMLTEXT"), (&XmlPi("p".into()), "XMLPI"), (&XmlConcat, "XMLCONCAT")];
    for (f, name) in publishing {
        assert_eq!(declared(f, &[]), (name.to_owned(), DataType::Xml(XmlType::Sequence)));
    }
}

/// Verifies: [DESIGN] RULES.md R-ERR-2: XMLPARSE given a boolean, and
/// XMLSERIALIZE given a value not XML, which the binder never gives them,
/// refuse it as a misuse of the interface, SQLCODE -910, never a panic;
/// a null argument is the null value.
#[test]
fn xml_functions_refuse_arguments_of_other_types() {
    let parse = XmlParse { syntax: Syntax::Content, whitespace: Whitespace::Preserve, binary: false };
    let number = Value::Bool(true);
    assert_eq!(parse.call(std::slice::from_ref(&number)).unwrap_err().sqlcode(), -910_i32);
    assert!(matches!(parse.call(&[Value::Null]).unwrap(), Value::Null));
    let serialization = Serialization {
        syntax: Syntax::Content,
        target: DataType::Boolean,
        binary: false,
        utf16: false,
        version: Version::V10,
        declaration: None,
    };
    assert_eq!(XmlSerialize(serialization).call(&[number]).unwrap_err().sqlcode(), -910_i32);
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
