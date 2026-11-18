//! What a path predicate costs over XML values' stored form, of documents
//! of several sizes, against the XML indexes, the primary's of Pal et al.
//! (§3.1) among them (RULES.md D106).
//!
//! For each size, a predicate `/order/item[. = "thing N"]` asked of every
//! value of a table's worth of documents:
//!
//! * decoded: each value's stored form decoded into a tree and the path
//!   walked, as an evaluator reading values whole does;
//! * streamed: one pass over each value's events (`xml::Events`), the
//!   path matched as elements open, no tree built, the least a reader of
//!   the stored form can do;
//! * checked: each value answered by the lookup's own check
//!   (`XmlLookup::matches`), one pass over its events;
//! * rows found: the rows an `xml_path` and an `xml_value` index give for
//!   it (`Index::xml_rows`), of an index of the first of those documents,
//!   about 200,000 items';
//! * looked up: those rows, each row's value then checked
//!   (`XmlLookup::matches`); and decoded: each decoded and walked instead;
//! * answered: the primary's answer, from its entries alone
//!   (`Index::xml_answers`), of the one row that holds, asked of it, and of
//!   every row; and of the same, filtered by its order's number,
//!   `/order[@no = 0]/item[. = "thing N"]`, beside the check of each value
//!   of that.
//!
//! And statements of a table of 2,000 orders of 10 items, read whole and
//! by each method's index, of the shapes the planner reads an index for:
//! a predicate of two steps, of each step, of `and`; queries joined by
//! AND; a parent step; a numeric predicate in ON, read below its guard;
//! and a query of a column of XML(SEQUENCE).
//!
//! Index building is outside the timer. Run with `cargo xtask bench
//! xml_index`, on the harness (RULES.md R-BENCH-1). When the XQuery
//! evaluator is there, its XMLEXISTS of the same predicate is a case of
//! its own here.

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use std::fmt::Write;
use std::sync::Arc;

use harness::{Bench, Case, note};
use turborel::db::{Database, Session};
use turborel::index::xml::{
    Axis, Comparison, Literal, Method, PathPattern, Step, Target, XmlAnswer, XmlFilter, XmlLookup, XmlMethod, XmlQuery,
};
use turborel::index::{Index, IndexMethod, Key};
use turborel::value::lob::Lob;
use turborel::value::xml::{Typing, XmlType, XmlValue};
use turborel::value::{CompOp, DataType, Value};
use turborel::xml::{Event, Events, Item, NodeId, QName, Sequence, Syntax, Whitespace, decode, encode, parse};

/// Items of each order: a small, a middling and a large document.
const SIZES: [usize; 3] = [10, 1_000, 50_000];
/// The stored forms read in each run, about: fewer of larger documents.
const OCTETS_READ: usize = 64 * 1024 * 1024;
/// The documents read, at most.
const INDEXED: usize = 2_000;
/// The items of the documents an index holds, about: fewer of larger
/// documents, so building it stays short.
const INDEXED_ITEMS: usize = 200_000;

fn main() {
    let bench = Bench::new("xml_index");
    for items in SIZES {
        let document = stored(&order(items, 0));
        let count = (OCTETS_READ / document.len()).clamp(1, INDEXED);
        let values: Vec<Vec<u8>> = (0..count).map(|n| stored(&order(items, n))).collect();
        note(&format!("{items} items: {} octets stored, {count} documents", document.len()));
        let lookup = predicate(items / 2);
        let bytes = document.len();
        bench.measure(Case::new(format!("decoded, {items} items"), "values").bytes(bytes), || {
            let held = values.iter().filter(|v| decoded(v, &format!("thing {}", items / 2))).count();
            assert_eq!(held, 1);
            count as u64
        });
        bench.measure(Case::new(format!("checked, {items} items"), "values").bytes(bytes), || {
            let held = values.iter().filter(|v| lookup.matches(&xml(v)).unwrap()).count();
            assert_eq!(held, 1);
            count as u64
        });
        bench.measure(Case::new(format!("streamed, {items} items"), "values").bytes(bytes), || {
            let held = values.iter().filter(|v| streamed(v, &format!("thing {}", items / 2))).count();
            assert_eq!(held, 1);
            count as u64
        });
        let indexed_values = &values[..(INDEXED_ITEMS / items).clamp(1, count)];
        for method in [Method::Path, Method::Value] {
            let index = indexed(method, indexed_values);
            bench.measure(Case::new(format!("rows found, {}, {items} items", method.name()), "lookups"), || {
                assert_eq!(Arc::clone(&index).xml_rows(&lookup).unwrap().count(), 1);
                1
            });
            bench.measure(Case::new(format!("looked up, {}, {items} items", method.name()), "lookups"), || {
                let rows: Vec<u64> = Arc::clone(&index).xml_rows(&lookup).unwrap().map(Result::unwrap).collect();
                let held = rows.iter().filter(|&&r| lookup.matches(&xml(&values[r as usize])).unwrap()).count();
                assert_eq!(held, 1);
                1
            });
            bench.measure(Case::new(format!("looked up and decoded, {}, {items} items", method.name()), "lookups"), || {
                let rows: Vec<u64> = Arc::clone(&index).xml_rows(&lookup).unwrap().map(Result::unwrap).collect();
                let wanted = format!("thing {}", items / 2);
                let held = rows.iter().filter(|&&r| decoded(&values[r as usize], &wanted)).count();
                assert_eq!(held, 1);
                1
            });
        }
        primary(&bench, items, (&values, indexed_values), &lookup);
    }
    statements(&bench);
}

/// The primary's cases of `items` items, of an index of `indexed`: its
/// answer of the one row that holds, asked of it, as `lookup` and as
/// filtered; and of every row; with the check of each of `values`,
/// filtered.
fn primary(bench: &Bench, items: usize, (values, indexed_values): (&[Vec<u8>], &[Vec<u8>]), lookup: &XmlLookup) {
    let index = indexed(Method::Primary, indexed_values);
    let filtered = filtered(lookup);
    bench.measure(Case::new(format!("checked, filtered, {items} items"), "values").bytes(values[0].len()), || {
        let held = values.iter().filter(|v| filtered.matches(&xml(v)).unwrap()).count();
        assert_eq!(held, 1);
        values.len() as u64
    });
    let alone = XmlQuery::of(lookup.clone());
    for (case, asked) in [("", &alone), ("filtered, ", &filtered)] {
        let one = XmlQuery { lookup: XmlLookup { row: Some(0), ..asked.lookup.clone() }, filters: asked.filters.clone() };
        bench.measure(Case::new(format!("answered, {case}one row, xml_primary, {items} items"), "lookups"), || {
            let answers: Vec<(u64, XmlAnswer)> = Arc::clone(&index).xml_answers(&one).unwrap().map(Result::unwrap).collect();
            assert_eq!(answers, [(0, XmlAnswer::Holds)]);
            1
        });
        bench.measure(Case::new(format!("answered, {case}every row, xml_primary, {items} items"), "lookups"), || {
            assert_eq!(Arc::clone(&index).xml_answers(asked).unwrap().count(), 1);
            1
        });
    }
}

/// `lookup` of the items of an order of number 0 alone, as
/// `/order[@no = 0]/item[. = "thing N"]`.
fn filtered(lookup: &XmlLookup) -> XmlQuery {
    let no = PathPattern { steps: vec![Step { axis: Axis::Child, name: Some(QName::local("no")) }], target: Target::Attribute };
    let zero = Comparison { op: CompOp::Eq, literal: Literal::Number(0.0) };
    XmlQuery { lookup: lookup.clone(), filters: vec![XmlFilter { at: 1, pattern: no, comparisons: vec![zero] }] }
}

/// An order of `items` items, the `n`th of a table: each document's
/// items but one named alike, so one document holds the item asked.
fn order(items: usize, n: usize) -> String {
    let mut text = format!("<order no=\"{n}\">");
    for i in 0..items {
        let name = if n == 0 { format!("thing {i}") } else { format!("other {i}") };
        write!(text, "<item sku=\"{i}\">{name}</item><qty>{}</qty>", i % 7).unwrap();
    }
    text + "</order>"
}

/// The stored form of `text`.
fn stored(text: &str) -> Vec<u8> {
    encode(&Sequence::document(parse(text, Syntax::Content, Whitespace::Preserve).unwrap()))
}

/// The value of stored form `bytes`, held in memory.
fn xml(bytes: &[u8]) -> XmlValue {
    XmlValue::stored(Lob::of_octets(bytes))
}

/// `/order/item[. = "thing n"]`.
fn predicate(n: usize) -> XmlLookup {
    let step = |name: &str| Step { axis: Axis::Child, name: Some(QName::local(name)) };
    XmlLookup {
        pattern: PathPattern { steps: vec![step("order"), step("item")], target: Target::Element },
        comparisons: vec![Comparison { op: CompOp::Eq, literal: Literal::String(format!("thing {n}")) }],
        row: None,
    }
}

/// Whether the value stored as `bytes` has an `/order/item` of text
/// `wanted`, read in one pass over its events.
fn streamed(bytes: &[u8], wanted: &str) -> bool {
    let mut events = Events::new(bytes).unwrap();
    let (mut depth, mut within, mut text) = (0_usize, false, String::new());
    while let Some(event) = events.next_event().unwrap() {
        match event {
            Event::Element { name, .. } => {
                depth += 1;
                within = depth == 2 && &*events.names()[name as usize].local == "item";
                text.clear();
            }
            Event::Text(t) => {
                if within {
                    text.push_str(t);
                }
            }
            Event::End => {
                if within && text == wanted {
                    return true;
                }
                within = false;
                depth = depth.saturating_sub(1);
            }
            Event::Node | Event::Atomic | Event::Document | Event::Other | Event::Attribute { .. } => {}
        }
    }
    false
}

/// Whether the value stored as `bytes`, decoded into trees, has an
/// `/order/item` of text `wanted`.
fn decoded(bytes: &[u8], wanted: &str) -> bool {
    decode(bytes).unwrap().items().iter().any(|item| {
        let Item::Node(node) = item else { return false };
        let tree = node.tree();
        let named = |id: NodeId, local: &str| tree.name(id).is_some_and(|n| &*n.local == local);
        tree.children(node.id())
            .filter(|&o| named(o, "order"))
            .any(|o| tree.children(o).any(|i| named(i, "item") && tree.string_value(i) == wanted))
    })
}

/// An index of `method` holding `values`, each of the row its place.
fn indexed(method: Method, values: &[Vec<u8>]) -> Arc<dyn Index> {
    let made = XmlMethod::in_memory(method);
    let index: Arc<dyn Index> = Arc::from(made.create(&[DataType::Xml(XmlType::Document(Typing::Untyped))]).unwrap());
    for (row, bytes) in values.iter().enumerate() {
        index.insert(Key(vec![Value::Xml(xml(bytes))]), row as u64).unwrap();
    }
    index
}

/// Orders of the table the statements read.
const ORDERS: usize = 2_000;

/// The statements of each shape the planner reads an index for, each a
/// count of the rows it holds of, with that count.
const SHAPES: [(&str, &str, u64); 7] = [
    ("two predicates", "SELECT COUNT(*) FROM T WHERE XMLEXISTS('/order[@no = \"7\"]/item[@sku = \"3\"]' PASSING BY REF X)", 1),
    (
        "a predicate of each step",
        "SELECT COUNT(*) FROM T WHERE XMLEXISTS('/order[@no = \"7\"][qty]/item[@sku = \"3\"]' PASSING BY REF X)",
        1,
    ),
    ("and", "SELECT COUNT(*) FROM T WHERE XMLEXISTS('/order[@no = \"7\" and item]' PASSING BY REF X)", 1),
    (
        "queries joined by AND",
        "SELECT COUNT(*) FROM T WHERE XMLEXISTS('//item[@sku = \"3\"]' PASSING BY REF X) AND XMLEXISTS('/order[@no = \"7\"]' PASSING BY REF X)",
        1,
    ),
    ("parent", "SELECT COUNT(*) FROM T WHERE XMLEXISTS('/order/item[@sku = \"3\"]/../@no[. = \"7\"]' PASSING BY REF X)", 1),
    (
        "a numeric predicate in ON",
        "SELECT COUNT(*) FROM O JOIN T ON O.K = T.K AND XMLEXISTS('/order[@no > 1990]/item' PASSING BY REF T.X)",
        9,
    ),
    ("a sequence", "SELECT COUNT(*) FROM T WHERE XMLEXISTS('/order[@no = \"7\"]/item' PASSING BY REF S)", 1),
];

/// The statements of `SHAPES` of a table T (K, X, S) of `ORDERS` orders of
/// 10 items each, S of XML(SEQUENCE) the same, and a table O of their
/// numbers: read whole, then by each method's index on X and on S.
fn statements(bench: &Bench) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML(DOCUMENT), S XML) CREATE TABLE O (K INTEGER)").unwrap();
    let a = db.session("A");
    for n in 0..ORDERS {
        let parsed = format!("XMLPARSE(DOCUMENT '{}' STRIP WHITESPACE)", order(10, n));
        a.run(&format!("INSERT INTO T VALUES ({n}, {parsed}, {parsed})")).unwrap();
        a.run(&format!("INSERT INTO O VALUES ({n})")).unwrap();
    }
    a.commit().unwrap();
    for method in ["", "XML_PATH", "XML_VALUE", "XML_PRIMARY"] {
        if !method.is_empty() {
            a.run(&format!("CREATE INDEX I ON T (X) USING {method}")).unwrap();
            a.run(&format!("CREATE INDEX J ON T (S) USING {method}")).unwrap();
        }
        let read = if method.is_empty() { "read whole" } else { method };
        for (shape, sql, held) in SHAPES {
            bench.measure(Case::new(format!("{shape}, {read}"), "statements"), || {
                assert_eq!(count(&a, sql), held, "{sql}");
                1
            });
        }
        a.commit().unwrap();
        if !method.is_empty() {
            a.run("DROP INDEX I").unwrap();
            a.run("DROP INDEX J").unwrap();
        }
    }
}

/// The count `sql`, a SELECT COUNT(*), gives.
fn count(a: &Session, sql: &str) -> u64 {
    let mut cursor = a.open_cursor(sql).unwrap();
    let row = cursor.fetch().unwrap().unwrap();
    row[0].to_string().parse().unwrap()
}
