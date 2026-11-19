//! Tests of an XML index's probes of several lookups: the rows each
//! gives, or any gives.

use super::*;
use crate::index::xml::XmlProbe;

/// The rows `index` gives for a probe of `lookups`, of any or of each,
/// each once, sorted.
fn probed(index: &Arc<XmlIndex>, lookups: &[XmlLookup], any: bool) -> Vec<RowId> {
    let probe = Probe::Xml(Box::new(XmlProbe { queries: Vec::new(), answered: false, lookups: lookups.to_vec(), any }));
    let mut rows: Vec<RowId> = Arc::clone(index).search(&probe, KeyOrder::Ascending).map(|e| e.unwrap().1).collect();
    let given = rows.len();
    rows.sort_unstable();
    rows.dedup();
    assert_eq!(rows.len(), given, "each row once");
    rows
}

/// The lookup of `pattern` and `comparisons`.
fn of(pattern: PathPattern, comparisons: Vec<Comparison>) -> XmlLookup {
    XmlLookup { pattern, comparisons, row: None }
}

/// Verifies: [EXTENSION] RULES.md D106 [Pal et al. §3.3]: of each method,
/// a probe of several lookups gives the rows each gives, a row a lookup
/// gives twice once, of one lookup its rows, of none none; or, of any,
/// the rows any gives.
#[test]
fn a_probe_gives_the_rows_of_each_lookup_or_of_any() {
    use Axis::{Child, Descendant};
    let titles = of(path(&[(Descendant, "title")], Target::Element), Vec::new());
    let cheap = of(path(&[(Descendant, "price")], Target::Element), vec![compared(CompOp::Lt, Literal::Number(5.0))]);
    let books = of(path(&[(Child, "book")], Target::Element), Vec::new());
    let isbn = of(path(&[(Child, "book"), (Child, "isbn")], Target::Attribute), vec![compared(CompOp::Eq, Literal::String("1".into()))]);
    for method in [Method::Path, Method::Value, Method::Property, Method::Primary] {
        let index = filled(method);
        assert_eq!(probed(&index, &[titles.clone(), cheap.clone()], false), [2, 3], "{method:?}");
        assert_eq!(probed(&index, &[cheap.clone(), titles.clone(), books.clone()], false), [2], "{method:?}");
        assert_eq!(probed(&index, &[books.clone(), isbn.clone()], false), [1], "{method:?}");
        assert_eq!(probed(&index, &[isbn.clone(), cheap.clone()], false), Vec::<RowId>::new(), "{method:?}");
        assert_eq!(probed(&index, &[isbn.clone(), cheap.clone()], true), [1, 2, 3], "{method:?}");
        assert_eq!(probed(&index, std::slice::from_ref(&titles), false), [1, 2, 3], "{method:?}");
        assert_eq!(probed(&index, &[], false), Vec::<RowId>::new(), "{method:?}");
    }
}
