//! Tests of values an XML index holds that are not one document node, as
//! XML(SEQUENCE) holds: every lookup gives their rows.

use super::*;
use crate::xml::{Atomic, AtomicType, Item, Node, NodeId};

/// The XML value of `sequence`, held in its row.
fn of(sequence: &Sequence) -> Key {
    Key(vec![Value::Xml(XmlValue::stored(Lob::of_octets(&encode(sequence))))])
}

/// Values of no one document node: two documents, an element, an atomic
/// value, and none; with the rows each is of, from 11.
fn others() -> Vec<(RowId, Key)> {
    let document = Sequence::document(parse("<book><title>Frogs</title></book>", Syntax::Content, Whitespace::Preserve).unwrap());
    let twice = Sequence::new(document.items().iter().chain(document.items()).cloned().collect());
    let tree = Arc::new(parse("<book><title>Frogs</title></book>", Syntax::Content, Whitespace::Preserve).unwrap());
    let element = Sequence::new(tree.children(NodeId::ROOT).map(|c| Item::Node(Node::new(Arc::clone(&tree), c))).collect());
    let atomic = Sequence::new(vec![Item::Atomic(Atomic::new(AtomicType::Integer, "7"))]);
    (11..).zip([of(&twice), of(&element), of(&atomic), of(&Sequence::default())]).collect()
}

/// Verifies: [EXTENSION] RULES.md D106, D15 [ISO/IEC 9075-14:2006 draft
/// §6.18 GR1a ii 2]: of each method, a value of several items, an element,
/// an atomic value or none is given by every lookup, a path no value has
/// too, and of a row asked if it is that row, and counted by its cost, as
/// a query of it may raise an error its entries do not show; the rows of
/// such a value are its own; taken back, nothing of it is left.
#[test]
fn a_value_not_one_document_is_given_by_every_lookup() {
    let titles = path(&[(Axis::Descendant, "title")], Target::Element);
    let nothing = path(&[(Axis::Child, "nope")], Target::Element);
    for method in [Method::Path, Method::Value, Method::Property, Method::Primary] {
        let index = filled(method);
        for (row, key) in others() {
            index.insert(key, row).unwrap();
        }
        assert_eq!(asked(&index, &titles, &[], None), [1, 2, 3, 11, 12, 13, 14], "{method:?}");
        assert_eq!(asked(&index, &nothing, &[], None), [11, 12, 13, 14], "{method:?}");
        assert_eq!(asked(&index, &nothing, &[], Some(12)), [12], "{method:?}");
        assert_eq!(asked(&index, &nothing, &[], Some(2)), Vec::<RowId>::new(), "{method:?}");
        let cost = Arc::clone(&index).xml_cost(&XmlLookup { pattern: nothing.clone(), comparisons: Vec::new(), row: None }).unwrap();
        assert!(cost.entries.is_some_and(|n| n >= 4), "{method:?}: {cost:?}");
        for (row, key) in others() {
            assert_eq!(index.rows(&key).unwrap(), [row], "{method:?}");
            index.remove(&key, row).unwrap();
        }
        assert_eq!(asked(&index, &nothing, &[], None), Vec::<RowId>::new(), "{method:?}");
        assert_eq!(held(&index, keys::OTHERS), 0, "{method:?}: no mark left");
    }
}

/// Verifies: [EXTENSION] RULES.md D106: a value added twice, by two
/// versions of its row, is given until both are taken back; one of
/// another row of the same octets apart.
#[test]
fn a_value_not_one_document_is_counted() {
    let index = index(Method::Path);
    let (row, key) = others().swap_remove(2);
    index.insert(key.clone(), row).unwrap();
    index.insert(key.clone(), row).unwrap();
    index.insert(key.clone(), 40).unwrap();
    let nothing = path(&[(Axis::Child, "nope")], Target::Element);
    index.remove(&key, row).unwrap();
    assert_eq!(asked(&index, &nothing, &[], None), [row, 40]);
    index.remove(&key, row).unwrap();
    assert_eq!(asked(&index, &nothing, &[], None), [40]);
}
