//! Tests of the XML index methods: entries added and taken back, lookups
//! of each method, and the bounds.

use super::*;
use crate::value::lob::Lob;
use crate::value::xml::XmlValue;
use crate::value::xml::{Typing, XmlType};
use crate::xml::{QName, Sequence, Syntax, Whitespace, encode, parse};

mod objects;

/// An empty tree in a pool in memory of its own.
pub(super) fn tree() -> BLinkTree {
    let (pool, file) = MemoryPages::new(MEMORY_POOL_BYTES).file("the tests' pool").unwrap();
    BLinkTree::create(pool, file).unwrap()
}

/// An empty index of `method`.
pub(super) fn index(method: Method) -> Arc<XmlIndex> {
    Arc::new(XmlMethod::in_memory(method).made(tree()))
}

/// The stored form of `text`, a document.
pub(super) fn stored(text: &str) -> Vec<u8> {
    encode(&Sequence::document(parse(text, Syntax::Content, Whitespace::Preserve).unwrap()))
}

/// The XML value of `text`, a document, held in its row.
pub(super) fn document(text: &str) -> Value {
    Value::Xml(XmlValue::stored(Lob::of_octets(&stored(text))))
}

/// The key of `text`, a document.
fn key(text: &str) -> Key {
    Key(vec![document(text)])
}

/// The path of `steps`, each an axis and a name, `*` any, ending at
/// `target`.
pub(super) fn path(steps: &[(Axis, &str)], target: Target) -> PathPattern {
    let steps = steps.iter().map(|&(axis, name)| Step { axis, name: (name != "*").then(|| QName::local(name)) }).collect();
    PathPattern { steps, target }
}

/// The rows `index` gives for `pattern` and `comparisons`, of `row` if
/// given, in order, each once as given.
pub(super) fn asked(index: &Arc<XmlIndex>, pattern: &PathPattern, comparisons: &[Comparison], row: Option<RowId>) -> Vec<RowId> {
    let lookup = XmlLookup { pattern: pattern.clone(), comparisons: comparisons.to_vec(), row };
    let mut rows: Vec<RowId> = Arc::clone(index).xml_rows(&lookup).unwrap().map(Result::unwrap).collect();
    let given = rows.len();
    rows.sort_unstable();
    rows.dedup();
    assert_eq!(rows.len(), given, "each row once");
    rows
}

/// The documents of rows 1, 2 and 3.
const DOCUMENTS: [&str; 3] = [
    r#"<book isbn="1"><title>Tree Frogs</title><price>12.5</price></book>"#,
    r#"<book isbn="2"><section><title>Bad Bugs</title></section><price>abc</price></book>"#,
    r"<magazine><title>Tree Frogs</title><price>4</price></magazine>",
];

/// An index of `method` holding `DOCUMENTS`, of rows 1 to 3.
fn filled(method: Method) -> Arc<XmlIndex> {
    let index = index(method);
    for (row, text) in (1..).zip(DOCUMENTS) {
        index.insert(key(text), row).unwrap();
    }
    index
}

/// A comparison by `op` with literal `literal`.
pub(super) fn compared(op: CompOp, literal: Literal) -> Comparison {
    Comparison { op, literal }
}

/// The lookups of `DOCUMENTS` and the rows each selects, as candidates.
fn lookups() -> Vec<(PathPattern, Vec<Comparison>, Vec<RowId>)> {
    use Axis::{Child, Descendant};
    let (e, a) = (Target::Element, Target::Attribute);
    let s = |t: &str| Literal::String(t.to_owned());
    vec![
        (path(&[(Child, "book"), (Child, "title")], e), vec![], vec![1]),
        (path(&[(Descendant, "title")], e), vec![], vec![1, 2, 3]),
        (path(&[(Child, "book"), (Descendant, "title")], e), vec![], vec![1, 2]),
        (path(&[(Child, "*"), (Child, "title")], e), vec![], vec![1, 3]),
        (path(&[(Child, "book"), (Child, "isbn")], a), vec![compared(CompOp::Eq, s("2"))], vec![2]),
        (path(&[(Descendant, "title")], e), vec![compared(CompOp::Eq, s("Tree Frogs"))], vec![1, 3]),
        (path(&[(Descendant, "price")], e), vec![compared(CompOp::Gt, Literal::Number(10.0))], vec![1, 2]),
        (path(&[(Descendant, "price")], e), vec![compared(CompOp::Lt, Literal::Number(5.0))], vec![2, 3]),
        (path(&[(Child, "book"), (Child, "nope")], e), vec![], vec![]),
        (path(&[(Child, "title")], e), vec![], vec![]),
        (path(&[(Child, "book")], a), vec![], vec![]),
    ]
}

/// Verifies: [EXTENSION] RULES.md D86 [Pal et al. §3.2.1-§3.2.3] [XQuery
/// §3.2, §3.5.2]: each method gives the rows whose value has a node a
/// path selects, with a value a comparison may hold of, each once: child
/// and descendant steps, a name or any, an element or an attribute, a
/// string or a number; a row whose number does not cast a candidate of a
/// numeric comparison; none of a name no value has.
#[test]
fn each_method_finds_the_rows_of_a_path() {
    for method in [Method::Path, Method::Value, Method::Property] {
        let index = filled(method);
        for (pattern, comparisons, rows) in lookups() {
            assert_eq!(asked(&index, &pattern, &comparisons, None), rows, "{method:?} {pattern:?} {comparisons:?}");
        }
    }
}

/// Verifies: [EXTENSION] RULES.md D86, D34, D15 [XQuery §3.5.2]: the rows
/// a lookup gives are a superset of those whose value it holds of, and of
/// those whose comparison raises; checked again (`XmlLookup::matches`),
/// they are exactly the rows it holds of.
#[test]
fn a_lookups_rows_checked_are_its_true_rows() {
    let index = filled(Method::Path);
    for (pattern, comparisons, _) in lookups() {
        let candidates = asked(&index, &pattern, &comparisons, None);
        let lookup = XmlLookup { pattern: pattern.clone(), comparisons: comparisons.clone(), row: None };
        let answers: Vec<(RowId, Result<bool>)> = (1..).zip(DOCUMENTS).map(|(r, t)| (r, lookup.matches(&xml_of(t)))).collect();
        let wanted: Vec<RowId> = answers.iter().filter(|(_, a)| !matches!(a, Ok(false))).map(|&(r, _)| r).collect();
        assert!(wanted.iter().all(|r| candidates.contains(r)), "{pattern:?}: {wanted:?} within {candidates:?}");
        let checked: Vec<RowId> =
            candidates.iter().copied().filter(|&r| answers.iter().any(|(at, a)| *at == r && matches!(a, Ok(true)))).collect();
        let true_rows: Vec<RowId> = answers.iter().filter(|(_, a)| matches!(a, Ok(true))).map(|&(r, _)| r).collect();
        assert_eq!(checked, true_rows, "{pattern:?} {comparisons:?}");
    }
    let raising = XmlLookup {
        pattern: path(&[(Axis::Descendant, "price")], Target::Element),
        comparisons: vec![compared(CompOp::Gt, Literal::Number(1.0))],
        row: None,
    };
    assert_eq!(raising.matches(&xml_of(DOCUMENTS[1])).unwrap_err().sqlcode(), -301_i32, "err:FORG0001");
}

/// The XML value of `text`, a document.
fn xml_of(text: &str) -> XmlValue {
    XmlValue::stored(Lob::of_octets(&stored(text)))
}

/// Verifies: [EXTENSION] RULES.md D86 [Pal et al. §3.2.2]: of a row asked,
/// a property index gives that row alone, if its value has the path; any
/// method does.
#[test]
fn a_row_asked_is_given_alone() {
    let titles = path(&[(Axis::Descendant, "title")], Target::Element);
    for method in [Method::Path, Method::Value, Method::Property] {
        let index = filled(method);
        assert_eq!(asked(&index, &titles, &[], Some(2)), [2], "{method:?}");
        assert_eq!(asked(&index, &path(&[(Axis::Child, "book")], Target::Element), &[], Some(3)), Vec::<RowId>::new());
    }
}

/// Every key of `index` from space `from` on: its marks and entries.
fn held(index: &XmlIndex, from: u8) -> usize {
    index.tree.scan(std::ops::Bound::Included(&[from][..]), std::ops::Bound::Unbounded, Direction::Forward).count()
}

/// Verifies: [EXTENSION] RULES.md D86, D34: an entry is counted, a pair
/// added twice taken back twice before it goes; every entry and marker
/// taken back leaves none, the dictionary staying; a pair never added
/// taken back changes nothing.
#[test]
fn entries_are_counted_and_taken_back() {
    for method in [Method::Path, Method::Value, Method::Property] {
        let index = filled(method);
        let titles = path(&[(Axis::Descendant, "title")], Target::Element);
        index.insert(key(DOCUMENTS[0]), 1).unwrap();
        index.remove(&key(DOCUMENTS[0]), 1).unwrap();
        assert_eq!(asked(&index, &titles, &[], None), [1, 2, 3], "{method:?}: once more");
        index.remove(&key(DOCUMENTS[0]), 7).unwrap();
        for (row, text) in (1..).zip(DOCUMENTS) {
            index.remove(&key(text), row).unwrap();
        }
        assert_eq!(asked(&index, &titles, &[], None), Vec::<RowId>::new());
        assert_eq!(held(&index, keys::MARKS), 0, "{method:?}: no marker or entry left");
        assert!(held(&index, keys::NAMES) > 0, "the dictionary stays");
        index.remove(&key(DOCUMENTS[0]), 1).unwrap();
    }
}

/// Verifies: [DESIGN] RULES.md D86, D34: the rows holding a value are
/// those it was added of, each once, until taken back; every entry,
/// `Probe::All`, is each pair held, of a null key; no comparison probes.
#[test]
fn the_rows_of_a_value_and_every_entry() {
    let index = filled(Method::Value);
    index.insert(key(DOCUMENTS[0]), 5).unwrap();
    assert_eq!(index.rows(&key(DOCUMENTS[0])).unwrap(), [1, 5]);
    assert_eq!(index.rows(&key("<other/>")).unwrap(), Vec::<RowId>::new());
    let all: Vec<RowId> = Arc::clone(&index).search(&Probe::All, KeyOrder::Ascending).map(|e| e.unwrap().1).collect();
    assert_eq!(all.len(), 4);
    let compare = Probe::Compare { op: CompOp::Eq, value: Value::Null };
    assert_eq!(Arc::clone(&index).search(&compare, KeyOrder::Ascending).next().unwrap().unwrap_err().sqlcode(), -900_i32);
    assert_eq!(index.estimate(&Probe::All), None);
    index.remove(&key(DOCUMENTS[0]), 5).unwrap();
    assert_eq!(index.rows(&key(DOCUMENTS[0])).unwrap(), [1]);
}

/// Verifies: [EXTENSION] RULES.md D86 [XQueryDM §2.1]: a name is an
/// expanded QName: a path finds a name in a namespace by its URI, and
/// not without it.
#[test]
fn names_are_expanded_qnames() {
    let index = index(Method::Path);
    index.insert(key(r#"<p:a xmlns:p="urn:x"><b/></p:a>"#), 1).unwrap();
    let mut a = QName::local("a");
    a.uri = Some("urn:x".into());
    let named = |name: QName| PathPattern { steps: vec![Step { axis: Axis::Child, name: Some(name) }], target: Target::Element };
    assert_eq!(asked(&index, &named(a), &[], None), [1]);
    assert_eq!(asked(&index, &named(QName::local("a")), &[], None), Vec::<RowId>::new());
}

/// Verifies: [EXTENSION] RULES.md D86: a value longer than its prefix is
/// keyed by it, so another of the same first octets is a candidate too,
/// which the check tells apart.
#[test]
fn values_alike_to_their_cut_are_candidates() {
    let index = index(Method::Value);
    let (one, two) = (format!("{}1", "t".repeat(VALUE_PREFIX_BYTES)), format!("{}2", "t".repeat(VALUE_PREFIX_BYTES)));
    index.insert(key(&format!("<r>{one}</r>")), 1).unwrap();
    index.insert(key(&format!("<r>{two}</r>")), 2).unwrap();
    let lookup = XmlLookup {
        pattern: path(&[(Axis::Child, "r")], Target::Element),
        comparisons: vec![compared(CompOp::Eq, Literal::String(one.clone()))],
        row: None,
    };
    assert_eq!(asked(&index, &lookup.pattern, &lookup.comparisons, None), [1, 2]);
    assert!(lookup.matches(&xml_of(&format!("<r>{one}</r>"))).unwrap());
    assert!(!lookup.matches(&xml_of(&format!("<r>{two}</r>"))).unwrap());
}

/// Verifies: [DESIGN] RULES.md D86, D35: a lookup's cost counts its
/// entries and prices its seeks by its tree's height; of a name no value
/// has, nothing.
#[test]
fn a_lookup_is_costed() {
    let index = filled(Method::Path);
    let ask = |pattern| XmlLookup { pattern, comparisons: Vec::new(), row: None };
    let titles = Arc::clone(&index).xml_cost(&ask(path(&[(Axis::Descendant, "title")], Target::Element))).unwrap();
    assert_eq!(titles.entries, Some(6), "two values of each of three titles");
    assert!(titles.pages.is_some_and(|p| p >= 2.0_f64), "the dictionary's seek and a path's");
    let none = Arc::clone(&index).xml_cost(&ask(path(&[(Axis::Child, "nope")], Target::Element))).unwrap();
    assert_eq!((none.entries, none.pages), (Some(0), Some(0.0_f64)));
}

/// Verifies: [DESIGN] RULES.md D86, D18: a lookup of no step, or of more
/// than `MAX_STEPS`, is refused.
#[test]
fn a_lookups_steps_are_bounded() {
    let index = filled(Method::Path);
    let refused = |steps: usize| {
        let pattern = path(&vec![(Axis::Child, "book"); steps], Target::Element);
        let lookup = XmlLookup { pattern, comparisons: Vec::new(), row: None };
        Arc::clone(&index).xml_rows(&lookup).unwrap().next().and_then(Result::err).map(|e| e.sqlcode())
    };
    assert_eq!(refused(0), Some(-910_i32));
    assert_eq!(refused(MAX_STEPS), None);
    assert_eq!(refused(MAX_STEPS + 1), Some(-900_i32));
}

/// Verifies: [EXTENSION] RULES.md D86, D34: a method takes one column of
/// an XML type alone, SQLCODE -103, and answers no comparison; an index
/// reopened by its locator holds what it held.
#[test]
fn a_method_takes_one_xml_column_and_reopens() {
    let method = XmlMethod::in_memory(Method::Property);
    let xml = DataType::Xml(XmlType::Document(Typing::Untyped));
    assert!(method.check_columns(std::slice::from_ref(&xml), "index I").is_ok());
    assert_eq!(method.check_columns(&[DataType::Boolean], "index I").unwrap_err().sqlcode(), -103_i32);
    assert_eq!(method.check_columns(&[xml.clone(), xml.clone()], "index I").unwrap_err().sqlcode(), -103_i32);
    assert!(!method.supports(CompOp::Eq) && method.name() == XML_PROPERTY_METHOD);
    let made: Arc<dyn Index> = Arc::from(method.create(std::slice::from_ref(&xml)).unwrap());
    made.insert(key(DOCUMENTS[0]), 1).unwrap();
    let again: Arc<dyn Index> = Arc::from(method.open(std::slice::from_ref(&xml), &made.locator()).unwrap().unwrap());
    let lookup = XmlLookup { pattern: path(&[(Axis::Child, "book")], Target::Element), comparisons: Vec::new(), row: None };
    assert_eq!(again.xml_rows(&lookup).unwrap().map(Result::unwrap).collect::<Vec<_>>(), [1]);
    assert_eq!(method.open(&[xml], &[1, 2]).err().map(|e| e.sqlcode()), Some(-916_i32));
    assert!(method.checkpoint().is_ok());
}

/// Verifies: [DESIGN] RULES.md D86, D18, R-ERR-5: a value an index cannot
/// key, of a key past the tree's longest or of more nodes than it keys,
/// is refused, SQLCODE -900, and leaves no entry or marker.
#[test]
fn a_value_refused_leaves_nothing() {
    let index = index(Method::Path);
    let long = "n".repeat(index.tree.max_key());
    assert_eq!(index.insert(key(&format!("<r><{long}/></r>")), 1).unwrap_err().sqlcode(), -900_i32);
    let many = format!("<r>{}</r>", "<b/>".repeat(MAX_KEYED_NODES));
    assert_eq!(index.insert(key(&many), 2).unwrap_err().sqlcode(), -900_i32);
    assert_eq!(held(&index, keys::MARKS), 0);
    assert_eq!(index.insert(Key(vec![Value::Bool(true)]), 3).unwrap_err().sqlcode(), -910_i32, "not XML");
    assert_eq!(index.insert(Key(Vec::new()), 3).unwrap_err().sqlcode(), -910_i32, "no column");
}
