//! Tests of values an XML index holds as large objects: kept by their
//! entries, so taken back unread.

use std::sync::atomic::AtomicBool;

use super::*;
use crate::value::lob::{Chunk, Directory, Kept, LobKind, LobSource};

/// A large object a table keeps, of id `id`, which may be freed, its
/// chunks then refused.
#[derive(Debug)]
struct Object {
    id: u64,
    octets: Arc<[u8]>,
    directory: Directory,
    freed: AtomicBool,
}

impl LobSource for Object {
    fn totals(&self) -> (u64, u64) {
        self.directory.totals()
    }
    fn directory(&self) -> Result<&Directory> {
        Ok(&self.directory)
    }
    fn chunk(&self, _k: usize) -> Result<Arc<[u8]>> {
        if self.freed.load(Ordering::Acquire) {
            return Err(SqlError::new(Code::Api, "[DESIGN] freed"));
        }
        Ok(Arc::clone(&self.octets))
    }
    fn kept(&self) -> Option<Kept> {
        Some(Kept { table: 1, id: self.id })
    }
}

/// The large object of id `id` holding `text`, a document, stored.
fn object(id: u64, text: &str) -> Arc<Object> {
    object_of(id, stored(text))
}

/// The large object of id `id` holding `stored`, a stored form.
fn object_of(id: u64, stored: Vec<u8>) -> Arc<Object> {
    let octets: Arc<[u8]> = Arc::from(stored);
    let size = u32::try_from(octets.len()).unwrap();
    Arc::new(Object { id, octets, directory: Directory::new(vec![Chunk { octets: size, chars: size }]), freed: AtomicBool::new(false) })
}

/// The key of the value `source` holds, read through it.
fn read_through(source: &Arc<Object>) -> Key {
    let source: Arc<dyn LobSource> = Arc::<Object>::clone(source);
    let lob = Lob::of_source(LobKind::Binary, source);
    Key(vec![Value::Xml(XmlValue::stored(lob))])
}

/// The key of the value of object `id`, by its id alone, as the log and a
/// spilled undo name it.
fn by_id(id: u64, source: &Object) -> Key {
    let (octets, chars) = source.totals();
    Key(vec![Value::Xml(XmlValue::stored(Lob::of_reference(LobKind::Binary, id, (octets, chars))))])
}

/// Verifies: [DESIGN] RULES.md D106, D79: a value held as a large object
/// is found as one its row holds is; added again by its id alone, from
/// the entries kept of it; and taken back with its object freed, by id or
/// read through it, unread, leaving no entry.
#[test]
fn an_object_is_taken_back_unread() {
    for method in [Method::Path, Method::Value, Method::Property] {
        let index = index(method);
        let source = object(9, r#"<book isbn="7"><title>Frogs</title></book>"#);
        index.insert(read_through(&source), 4).unwrap();
        index.insert(by_id(9, &source), 4).unwrap();
        let isbn = path(&[(Axis::Child, "book"), (Axis::Child, "isbn")], Target::Attribute);
        let seven = [compared(CompOp::Eq, Literal::Number(7.0))];
        assert_eq!(asked(&index, &isbn, &seven, None), [4], "{method:?}");
        assert_eq!(index.rows(&by_id(9, &source)).unwrap(), [4]);
        source.freed.store(true, Ordering::Release);
        index.remove(&by_id(9, &source), 4).unwrap();
        assert_eq!(asked(&index, &isbn, &seven, None), [4], "{method:?}: added twice");
        index.remove(&read_through(&source), 4).unwrap();
        assert_eq!(asked(&index, &isbn, &[], None), Vec::<RowId>::new(), "{method:?}");
        assert_eq!(held(&index, keys::MARKS), 0, "{method:?}: no marker, document entry or entry left");
    }
}

/// Verifies: [DESIGN] RULES.md D106, R-ERR-5: an object named by its id
/// alone that the index does not hold cannot be read, SQLCODE -910, and
/// leaves nothing; one of a value refused leaves nothing either.
#[test]
fn an_object_not_held_is_refused() {
    let index = index(Method::Path);
    let source = object(3, "<a/>");
    assert_eq!(index.insert(by_id(3, &source), 1).unwrap_err().sqlcode(), -910_i32);
    let deep = object(4, &format!("<r>{}</r>", "<b/>".repeat(MAX_KEYED_NODES)));
    assert_eq!(index.insert(read_through(&deep), 1).unwrap_err().sqlcode(), -900_i32);
    assert_eq!(held(&index, keys::MARKS), 0);
    index.remove(&by_id(3, &source), 1).unwrap();
}

/// Verifies: [DESIGN] RULES.md D106: the rows of a value held as an object
/// are those it was added of, by its id.
#[test]
fn the_rows_of_an_object_are_its_rows() {
    let index = index(Method::Value);
    let source = object(5, "<a>x</a>");
    index.insert(read_through(&source), 8).unwrap();
    assert_eq!(index.rows(&by_id(5, &source)).unwrap(), [8]);
    assert_eq!(index.rows(&by_id(6, &source)).unwrap(), Vec::<RowId>::new());
}

/// Verifies: [DESIGN] RULES.md D106, D79: a value of no one document node
/// held as a large object is given by every lookup, added again by its id
/// alone, and taken back unread, its object freed, leaving no mark.
#[test]
fn an_object_not_one_document_is_given_and_taken_back_unread() {
    let index = index(Method::Value);
    let tree = parse("<a/><b/>", Syntax::Content, Whitespace::Preserve).unwrap();
    let tree = Arc::new(tree);
    let items = tree.children(crate::xml::NodeId::ROOT).map(|c| crate::xml::Item::Node(crate::xml::Node::new(Arc::clone(&tree), c)));
    let source = object_of(12, encode(&Sequence::new(items.collect())));
    index.insert(read_through(&source), 6).unwrap();
    index.insert(by_id(12, &source), 6).unwrap();
    let nothing = path(&[(Axis::Child, "nope")], Target::Element);
    assert_eq!(asked(&index, &nothing, &[], None), [6]);
    source.freed.store(true, Ordering::Release);
    index.remove(&by_id(12, &source), 6).unwrap();
    assert_eq!(asked(&index, &nothing, &[], None), [6], "added twice");
    index.remove(&by_id(12, &source), 6).unwrap();
    assert_eq!(asked(&index, &nothing, &[], None), Vec::<RowId>::new());
    assert_eq!(held(&index, keys::MARKS), 0);
}
