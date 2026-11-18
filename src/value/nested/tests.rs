use super::*;
use crate::value::lob::Part;
use crate::value::types::UdtName;
use crate::xml::{Sequence, Syntax, Whitespace, parse};

/// An XML value held as its sequence, of `text` parsed as CONTENT.
fn held(text: &str) -> Value {
    Value::Xml(XmlValue::of(Sequence::document(parse(text, Syntax::Content, Whitespace::Preserve).unwrap())))
}

/// Whether `v` is an XML value held as its stored form inline.
fn stored_inline(v: &Value) -> bool {
    let Value::Xml(x) = v else { return false };
    x.stored_lob().is_some_and(|lob| lob.inline().is_some())
}

/// The parts of `v`, a value made of values.
fn parts(v: &Value) -> &[Value] {
    parts_of(v).unwrap_or_else(|| panic!("{v:?} is made of no values"))
}

/// The one part of the one value of `row`, after its parts are held
/// inline, which it must be.
fn inlined_part(mut row: Vec<Value>) -> Value {
    hold_parts_inline(&mut row).unwrap();
    parts(&row[0])[0].clone()
}

/// Verifies: [DESIGN] RULES.md D79, D106: a value made of values needs
/// keeping exactly when it holds a large object or an XML value at some
/// depth; a large object or an XML value does itself; nothing else does.
#[test]
fn which_values_need_keeping() {
    let xml = held("<a/>");
    assert!(xml.needs_keeping());
    assert!(Value::Lob(Lob::of_octets(&[1])).needs_keeping());
    assert!(Value::Array(Arc::from(vec![Value::Row(Arc::from(vec![Value::Null, xml]))])).needs_keeping());
    assert!(!Value::Array(Arc::from(vec![Value::Row(Arc::from(vec![Value::Bool(true)]))])).needs_keeping());
    assert!(!Value::Char("a".into()).needs_keeping());
}

/// Verifies: [DESIGN] RULES.md D106: each XML value inside an array, a
/// multiset, a row or a structured value is made its stored form held
/// inline, its serialization kept; other parts and top-level values are
/// left as they are.
#[test]
fn xml_parts_are_held_inline() {
    let structured = Structured::new(Arc::new(UdtName { schema: "A".into(), name: "ST".into() }), Arc::from(vec![held("<s/>")]));
    let mut row = vec![
        held("<top/>"),
        Value::Array(Arc::from(vec![held("<a/>"), Value::Null])),
        Value::Multiset(Arc::from(vec![held("<m/>")])),
        Value::Row(Arc::from(vec![Value::Bool(true), held("<r/>")])),
        Value::Structured(Arc::new(structured)),
    ];
    hold_parts_inline(&mut row).unwrap();
    assert!(!stored_inline(&row[0]), "a column's own value is the table's to keep");
    assert!(stored_inline(&parts(&row[1])[0]) && parts(&row[1])[1].is_null());
    assert!(stored_inline(&parts(&row[2])[0]));
    let fields = parts(&row[3]);
    assert!(matches!(fields[0], Value::Bool(true)) && stored_inline(&fields[1]));
    assert!(stored_inline(&parts(&row[4])[0]));
    let Value::Xml(x) = &fields[1] else { panic!("an XML value") };
    assert_eq!(x.text().unwrap(), "<r/>");
}

/// Verifies: [DESIGN] RULES.md D79: a large object inside a value made of
/// values is held inline up to a chunk, its octets and characters kept;
/// one longer is refused, SQLCODE -900, the boundary a chunk exactly.
#[test]
fn large_object_parts_are_held_to_a_chunk() {
    let octets = Lob::of_parts(LobKind::Binary, vec![Part::Inline(Arc::from(&[1_u8, 2][..]), 2), Part::Inline(Arc::from(&[3_u8][..]), 1)]);
    let Value::Lob(binary) = inlined_part(vec![Value::Array(Arc::from(vec![Value::Lob(octets)]))]) else { panic!("a large object") };
    assert_eq!(binary.inline(), Some(&[1_u8, 2, 3][..]));
    let text =
        Lob::of_parts(LobKind::Character, vec![Part::Inline(Arc::from("é".as_bytes()), 1), Part::Inline(Arc::from("b".as_bytes()), 1)]);
    let Value::Lob(character) = inlined_part(vec![Value::Row(Arc::from(vec![Value::Lob(text)]))]) else { panic!("a large object") };
    assert!(character.inline() == Some("éb".as_bytes()) && character.chars() == 2);
    let chunk = vec![0_u8; CHUNK_OCTETS];
    let mut fits = vec![Value::Array(Arc::from(vec![Value::Lob(Lob::of_octets(&chunk))]))];
    assert!(hold_parts_inline(&mut fits).is_ok());
    let mut longer = vec![Value::Array(Arc::from(vec![Value::Lob(Lob::of_octets(&[chunk, vec![0]].concat()))]))];
    assert_eq!(hold_parts_inline(&mut longer).unwrap_err().code, Code::Unsupported);
}

/// Verifies: [DESIGN] R-CF-6: values nested deeper than `MAX_DEPTH` are
/// refused, SQLCODE -900, not walked, whatever they hold; those as deep
/// are held inline.
#[test]
fn nesting_is_bounded() {
    let nest = |inner: Value, levels: usize| (0..levels).fold(inner, |v, _| Value::Array(Arc::from(vec![v])));
    let mut deep = vec![nest(held("<a/>"), MAX_DEPTH)];
    assert!(hold_parts_inline(&mut deep).is_ok());
    let mut deeper = vec![nest(held("<a/>"), MAX_DEPTH.saturating_add(1))];
    assert_eq!(hold_parts_inline(&mut deeper).unwrap_err().code, Code::Unsupported);
    let plain = nest(Value::Bool(true), MAX_DEPTH.saturating_add(1));
    assert!(plain.needs_keeping() && !nest(Value::Bool(true), MAX_DEPTH).needs_keeping());
    assert_eq!(hold_parts_inline(&mut [plain]).unwrap_err().code, Code::Unsupported);
}
