//! XML indexes (RULES.md D86): the methods XML_PATH, XML_VALUE,
//! XML_PROPERTY and XML_PRIMARY of CREATE INDEX, kept with a table's writes, transactions
//! and rollbacks, reopened and recovered; their lookups give a superset of
//! the rows a path selects, the right ones once checked; and their bounds.

use std::fmt::Write;
use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::FileSystem;
use turborel::error::Code;
use turborel::index::xml::{Axis, Comparison, Literal, MAX_KEYED_NODES, PathPattern, Step, Target, XmlFilter, XmlLookup, XmlQuery};
use turborel::value::{CompOp, Value};
use turborel::xml::QName;

use crate::common::{Scratch, after_crash, os, rows, sorted};

/// The methods, as CREATE INDEX names them.
const METHODS: [&str; 4] = ["XML_PATH", "XML_VALUE", "XML_PROPERTY", "XML_PRIMARY"];

/// A database of table A.T (K INTEGER, X XML), empty.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML)").unwrap();
    db
}

/// Inserts row `k` of document `document`.
fn insert(a: &Session, k: usize, document: &str) {
    a.run(&format!("INSERT INTO T VALUES ({k}, XMLPARSE(DOCUMENT '{document}' PRESERVE WHITESPACE))")).unwrap();
}

/// Makes index `name` of `method` on T (X), outside a transaction.
fn index(a: &Session, name: &str, method: &str) {
    a.run(&format!("CREATE INDEX {name} ON T (X) USING {method}")).unwrap();
}

/// The lookup of `steps`, each an axis and a local name, ending at
/// `target`, with `comparisons`.
fn lookup(steps: &[(Axis, &str)], target: Target, comparisons: Vec<Comparison>) -> XmlLookup {
    let steps = steps.iter().map(|&(axis, name)| Step { axis, name: Some(QName::local(name)) }).collect();
    XmlLookup { pattern: PathPattern { steps, target }, comparisons, row: None }
}

/// The keys of the rows index `name` gives for `lookup`, as committed,
/// and of those its check holds of, each sorted.
fn found(db: &Database, name: &str, query: &XmlQuery) -> (Vec<String>, Vec<String>) {
    let candidates = db.xml_candidates(name, &query.lookup).unwrap();
    let key = |row: &Vec<Value>| row[0].to_string();
    let mut all: Vec<String> = candidates.iter().map(key).collect();
    let mut held: Vec<String> = candidates
        .iter()
        .filter(|row| match &row[1] {
            Value::Xml(x) => query.matches(x).unwrap_or(true),
            _ => false,
        })
        .map(key)
        .collect();
    all.sort();
    held.sort();
    (all, held)
}

/// The keys of the rows whose document a lookup holds of, by SQL alone,
/// read whole: the answer an index must give once checked.
fn by_scan(a: &Session, query: &XmlQuery) -> Vec<String> {
    let mut keys = Vec::new();
    let mut cursor = a.open_cursor("SELECT K, X FROM T").unwrap();
    while let Some(row) = cursor.fetch().unwrap() {
        if let Value::Xml(x) = &row[1]
            && query.matches(x).unwrap_or(true)
        {
            keys.push(row[0].to_string());
        }
    }
    keys.sort();
    keys
}

/// A comparison by `op` with the string `s`.
fn string(op: CompOp, s: &str) -> Comparison {
    Comparison { op, literal: Literal::String(s.to_owned()) }
}

/// A comparison by `op` with the number `n`.
fn number(op: CompOp, n: f64) -> Comparison {
    Comparison { op, literal: Literal::Number(n) }
}

/// The lookups the tests ask.
fn lookups() -> Vec<XmlQuery> {
    use Axis::{Child, Descendant};
    vec![
        alone(lookup(&[(Child, "order"), (Child, "item")], Target::Element, vec![])),
        alone(lookup(&[(Descendant, "item")], Target::Element, vec![string(CompOp::Eq, "tea")])),
        alone(lookup(&[(Child, "order"), (Child, "no")], Target::Attribute, vec![number(CompOp::Ge, 17.0)])),
        alone(lookup(&[(Descendant, "qty")], Target::Element, vec![number(CompOp::Lt, 3.0)])),
        filtered(lookup(&[(Child, "order"), (Child, "item")], Target::Element, vec![string(CompOp::Eq, "tea")]), number(CompOp::Ge, 17.0)),
    ]
}

/// `lookup`, of the nodes whose parent has an attribute `no` of which
/// `comparison` holds, as `/order[@no >= 17]/item` [XQuery §3.2.2].
fn filtered(lookup: XmlLookup, comparison: Comparison) -> XmlQuery {
    let no = PathPattern { steps: vec![Step { axis: Axis::Child, name: Some(QName::local("no")) }], target: Target::Attribute };
    XmlQuery { lookup, filter: Some(XmlFilter { above: 1, pattern: no, comparisons: vec![comparison] }) }
}

/// `lookup` alone.
fn alone(lookup: XmlLookup) -> XmlQuery {
    XmlQuery { lookup, filter: None }
}

/// A document of `n` items, its stored form longer than a row holds.
fn long_order(no: usize, n: usize) -> String {
    let mut order = format!("<order no=\"{no}\">");
    for i in 0..n {
        write!(order, "<item>thing {i}</item><qty>{i}</qty>").unwrap();
    }
    order + "</order>"
}

/// Every lookup asked of every index of `db`: each gives every row it
/// holds of, by a scan, and checked, exactly those.
fn every_lookup_answers(db: &Database, names: &[&str]) {
    let a = db.session("A");
    for lookup in lookups() {
        let wanted = by_scan(&a, &lookup);
        for name in names {
            let (all, held) = found(db, name, &lookup);
            assert!(wanted.iter().all(|k| all.contains(k)), "{name} {lookup:?}: {wanted:?} within {all:?}");
            assert_eq!(held, wanted, "{name} {lookup:?}");
        }
    }
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D86, D34 [Pal et al. §3.2]: CREATE INDEX
/// makes an index of each method, its name matched in any case, built of
/// the rows there and kept up to date; INFORMATION_SCHEMA.INDEXES names
/// its method; DROP INDEX drops it.
#[test]
fn each_method_is_made_listed_and_dropped() {
    let db = database();
    let a = db.session("A");
    insert(&a, 1, r#"<order no="17"><item>tea</item><qty>2</qty></order>"#);
    a.commit().unwrap();
    for (n, method) in METHODS.iter().enumerate() {
        index(&a, &format!("I{n}"), &method.to_lowercase());
    }
    insert(&a, 2, r#"<order no="18"><item>cups</item><qty>x</qty></order>"#);
    a.commit().unwrap();
    every_lookup_answers(&db, &["I0", "I1", "I2", "I3"]);
    let listed = sorted(&a, "SELECT INDEX_NAME, METHOD_NAME, COLUMN_NAMES FROM INFORMATION_SCHEMA.INDEXES WHERE TABLE_NAME = 'T'");
    assert_eq!(listed, ["I0,xml_path,X", "I1,xml_value,X", "I2,xml_property,X", "I3,xml_primary,X"]);
    a.rollback().unwrap();
    a.run("DROP INDEX I1").unwrap();
    assert_eq!(db.xml_candidates("I1", &lookups()[0].lookup).unwrap_err().code, Code::Api);
    assert_eq!(db.index_entries("I0").unwrap(), 2, "a pair of each row and its value");
}

/// Verifies: [EXTENSION] RULES.md D86, D34: an XML method keys one column
/// of an XML type: another column, or two, is refused, SQLCODE -103, as
/// an index of another method on an XML column is.
#[test]
fn an_xml_method_keys_one_xml_column() {
    let db = database();
    let a = db.session("A");
    for columns in ["K", "K, X", "X, X"] {
        let error = a.run(&format!("CREATE INDEX I ON T ({columns}) USING XML_PATH")).unwrap_err();
        assert_eq!(error.code, Code::SyntaxRule, "{columns}: {error}");
    }
    assert_eq!(a.run("CREATE INDEX I ON T (X) USING HASH").unwrap_err().code, Code::SyntaxRule);
    assert_eq!(rows(&a, "SELECT INDEX_NAME FROM INFORMATION_SCHEMA.INDEXES WHERE TABLE_NAME = 'T'").unwrap(), Vec::<String>::new());
}

/// Verifies: [EXTENSION] RULES.md D86, D29, D34 [ISO/IEC 9075:1992 §4.28]:
/// each index is kept with inserts, updates and deletes, of values held
/// in their rows and as large objects, each committed or rolled back, and
/// with the versions taken back as they are collected: every lookup
/// checked gives what a scan does.
#[test]
fn indexes_are_kept_with_writes_and_rollbacks() {
    let db = database();
    let a = db.session("A");
    for (n, method) in METHODS.iter().enumerate() {
        index(&a, &format!("I{n}"), method);
    }
    let names = ["I0", "I1", "I2", "I3"];
    insert(&a, 1, r#"<order no="17"><item>tea</item><qty>2</qty></order>"#);
    insert(&a, 2, &long_order(20, 400));
    insert(&a, 3, r#"<order no="9"><item>tea</item><qty>5</qty></order>"#);
    a.commit().unwrap();
    every_lookup_answers(&db, &names);
    a.run(&format!("UPDATE T SET X = XMLPARSE(DOCUMENT '{}' PRESERVE WHITESPACE) WHERE K = 1", long_order(30, 300))).unwrap();
    a.run(r#"UPDATE T SET X = XMLPARSE(DOCUMENT '<order no="1"><qty>1</qty></order>' PRESERVE WHITESPACE) WHERE K = 2"#).unwrap();
    a.run("DELETE FROM T WHERE K = 3").unwrap();
    a.commit().unwrap();
    every_lookup_answers(&db, &names);
    insert(&a, 4, &long_order(40, 200));
    a.run("UPDATE T SET K = 5 WHERE K = 2").unwrap();
    a.run("DELETE FROM T WHERE K = 1").unwrap();
    a.rollback().unwrap();
    every_lookup_answers(&db, &names);
    a.run("DELETE FROM T").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT K FROM T").unwrap(), Vec::<String>::new());
    a.commit().unwrap();
    for name in names {
        assert_eq!(db.index_entries(name).unwrap(), 0, "{name}: every version collected takes its entries back");
    }
}

/// Verifies: [EXTENSION] RULES.md D86, D25: an index is reopened as it
/// was after a clean close, and recovered with what the log holds after a
/// crash, its dictionary with it: values held in rows and as objects.
#[test]
fn indexes_are_reopened_and_recovered() {
    let scratch = Scratch::new("xml-index");
    let dir = scratch.path("db");
    let open = |fs: &Arc<dyn FileSystem>| {
        let db = Database::new();
        db.open_files(fs, &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open(&os());
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML)").unwrap();
    let a = db.session("A");
    for (n, method) in METHODS.iter().enumerate() {
        index(&a, &format!("I{n}"), method);
    }
    insert(&a, 1, r#"<order no="17"><item>tea</item><qty>2</qty></order>"#);
    insert(&a, 2, &long_order(20, 400));
    a.commit().unwrap();
    drop(a);
    db.close().unwrap();
    drop(db);
    let before = open(&os());
    every_lookup_answers(&before, &["I0", "I1", "I2", "I3"]);
    let a = before.session("A");
    insert(&a, 3, r#"<order no="30"><item>tea</item></order>"#);
    a.run(&format!("UPDATE T SET X = XMLPARSE(DOCUMENT '{}' PRESERVE WHITESPACE) WHERE K = 1", long_order(10, 300))).unwrap();
    a.run("DELETE FROM T WHERE K = 2").unwrap();
    a.commit().unwrap();
    insert(&a, 4, r#"<order no="40"><item>tea</item></order>"#);
    // A crash: the database is left, and opened again as it is.
    let db = open(&after_crash());
    every_lookup_answers(&db, &["I0", "I1", "I2", "I3"]);
    let tea = alone(lookup(&[(Axis::Descendant, "item")], Target::Element, vec![string(CompOp::Eq, "tea")]));
    assert_eq!(found(&db, "I0", &tea).1, ["3"], "the write not committed is gone");
    drop(a);
    drop((db, before));
}

/// Verifies: [EXTENSION] RULES.md D86, D18, R-ERR-5: a value an index
/// cannot key, of more elements and attributes than `MAX_KEYED_NODES`, is
/// refused, SQLCODE -900, and its statement has no effect; a table with
/// no XML index takes it.
#[test]
fn a_value_past_the_bounds_is_refused() {
    let db = database();
    let a = db.session("A");
    let many = format!("<r>{}</r>", "<b/>".repeat(MAX_KEYED_NODES));
    insert(&a, 1, &many);
    a.run("DELETE FROM T").unwrap();
    a.commit().unwrap();
    index(&a, "I", "XML_VALUE");
    let error = a.run(&format!("INSERT INTO T VALUES (1, XMLPARSE(DOCUMENT '{many}' PRESERVE WHITESPACE))")).unwrap_err();
    assert_eq!(error.code, Code::Unsupported, "{error}");
    assert_eq!(rows(&a, "SELECT K FROM T").unwrap(), Vec::<String>::new());
    a.commit().unwrap();
    assert_eq!(db.index_entries("I").unwrap(), 0);
}
