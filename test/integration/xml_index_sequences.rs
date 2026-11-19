//! XML indexes of a column of XML(SEQUENCE) (RULES.md D106): a value of
//! other than one document node, of several items, an element, an atomic
//! value or none, is given by every lookup, so a query of it is checked,
//! and fails, as a full read fails it.

use turborel::db::{Database, Session};
use turborel::error::SqlState;
use turborel::params::Params;

use crate::common::{fetched, rows};

/// The methods, as CREATE INDEX names them.
const METHODS: [&str; 4] = ["XML_PATH", "XML_VALUE", "XML_PROPERTY", "XML_PRIMARY"];

/// How many rows table U has, after its null row.
const ROWS: usize = 240;

/// Of every how many rows one is an element alone.
const ELEMENTS: usize = 8;

/// The document of row `k`: an order of an item of a name and a price.
fn document(k: usize) -> String {
    let name = ["pen", "ink", "cap"][k % 3];
    format!("<order id=\"{k}\"><item price=\"{}\"><name>{name}</name></item></order>", k % 50)
}

/// Schema A: table U (K INTEGER, S XML(SEQUENCE)), of `ROWS` rows, each
/// row's value its document, but, `with_elements`, every `ELEMENTS`th
/// row's the document's order element alone, of no document node; and
/// its null row 0.
fn database(with_elements: bool) -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE U (K INTEGER, S XML(SEQUENCE))").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO U VALUES (0, NULL)").unwrap();
    for k in 1..=ROWS {
        let parsed = format!("XMLPARSE(DOCUMENT '{}' STRIP WHITESPACE)", document(k));
        let value = if !with_elements || k % ELEMENTS != 0 {
            parsed
        } else {
            format!("XMLQUERY('/*' PASSING BY VALUE {parsed} RETURNING SEQUENCE EMPTY ON EMPTY)")
        };
        a.run(&format!("INSERT INTO U VALUES ({k}, {value})")).unwrap();
    }
    a.commit().unwrap();
    db
}

/// The physical plan of `query`, as EXPLAIN shows it.
fn plan(a: &Session, query: &str) -> String {
    let explained = a.explain(query, &Params::new()).unwrap();
    explained.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// Conditions of U's rows that raise no error of a document or an
/// element: relative paths, which select the element's children and the
/// document's; each with whether it is selective enough that an index of
/// each secondary method is read for it.
const CONDITIONS: [(&str, bool); 6] = [
    ("XMLEXISTS('item[name = \"pen\"]' PASSING BY REF S)", true),
    ("XMLEXISTS('.//item[@price > 45]' PASSING BY REF S)", true),
    ("XMLEXISTS('order/item' PASSING BY REF S)", false),
    ("XMLEXISTS('.//name[. = \"cap\"]' PASSING BY VALUE S)", false),
    ("XMLQUERY('item/name' PASSING BY REF S NULL ON EMPTY) IS NOT NULL", true),
    ("XMLEXISTS('order/item/@price[. = \"7\"]' PASSING BY REF S)", true),
];

/// The keys of U's rows `condition` holds of, in order.
fn keys(a: &Session, condition: &str) -> Vec<String> {
    rows(a, &format!("SELECT K FROM U WHERE {condition} ORDER BY K")).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §8.4, §6.18 GR1a ii]
/// RULES.md D106, D34: of each method, an index of a column of
/// XML(SEQUENCE) is read for a query of it, EXPLAIN showing so and never
/// answering it from the index alone, and every
/// condition gives the rows of a full read, the values that are an
/// element and no document node among them.
#[test]
fn a_sequence_column_is_read_by_an_index() {
    let db = database(true);
    let a = db.session("A");
    let full: Vec<Vec<String>> = CONDITIONS.iter().map(|(c, _)| keys(&a, c)).collect();
    assert!(full.iter().all(|k| !k.is_empty()), "{full:?}");
    assert!(full[0].iter().any(|k| k.parse::<usize>().unwrap() % ELEMENTS == 0), "an element's children: {:?}", full[0]);
    a.commit().unwrap();
    for method in METHODS {
        a.run(&format!("CREATE INDEX I ON U (S) USING {method}")).unwrap();
        for ((condition, selective), expected) in CONDITIONS.iter().zip(&full) {
            assert_eq!(&keys(&a, condition), expected, "{method}: {condition}");
            let shown = plan(&a, &format!("SELECT K FROM U WHERE {condition}"));
            let read = shown.contains("IndexScan A.U using I");
            assert!(!selective || method == "XML_PRIMARY" || read, "{method}: {condition}: {shown}");
            assert!(!shown.contains("answered"), "a sequence's query is checked: {shown}");
        }
        a.commit().unwrap();
        a.run("DROP INDEX I").unwrap();
    }
}

/// The values of no one node that fail a query as its context item, each
/// with the SQLSTATE they fail with: several items [§6.18 GR1a ii 2 B
/// III]; an atomic value, of which a path's step is a type error [XQuery
/// §3.2]; none, of no context item [XQuery §2.1.2]; an element, whose
/// root is no document a path from `/` starts at [XQuery §3.2].
const FAILING: [(&str, SqlState); 4] = [
    ("XMLQUERY('<a/>, <b/>' RETURNING SEQUENCE BY REF EMPTY ON EMPTY)", SqlState::INVALID_XQUERY_CONTEXT_ITEM),
    ("XMLQUERY('7' RETURNING SEQUENCE BY REF EMPTY ON EMPTY)", SqlState::XQUERY_ERROR),
    ("XMLQUERY('()' RETURNING SEQUENCE BY REF EMPTY ON EMPTY)", SqlState::XQUERY_ERROR),
    ("XMLQUERY('<order><item/></order>' RETURNING SEQUENCE BY REF EMPTY ON EMPTY)", SqlState::XQUERY_ERROR),
];

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §6.18 GR1a ii 2 B
/// III, GR3] RULES.md D15, D106: a row of a value a query fails of, as
/// its context item, is given by each method's lookup of a path no value
/// has, so the query fails as a full read fails it: of several items
/// SQLSTATE 2200V, of an atomic value, of none, and of an element a path
/// from `/` is of, 10000.
#[test]
fn values_a_query_fails_of_are_given() {
    let db = database(false);
    let a = db.session("A");
    let query = "SELECT K FROM U WHERE XMLEXISTS('/order/nothing' PASSING BY REF S)";
    assert_eq!(fetched(&a, query).unwrap(), 0, "documents alone hold no such node");
    for (value, state) in FAILING {
        a.run(&format!("INSERT INTO U VALUES (1000, {value})")).unwrap();
        a.commit().unwrap();
        assert_eq!(fetched(&a, query).unwrap_err().sqlstate(), state, "a full read: {value}");
        a.rollback().unwrap();
        for method in METHODS {
            a.run(&format!("CREATE INDEX I ON U (S) USING {method}")).unwrap();
            assert!(plan(&a, query).contains("IndexScan"), "{method}: {}", plan(&a, query));
            assert_eq!(fetched(&a, query).unwrap_err().sqlstate(), state, "{method}: {value}");
            a.rollback().unwrap();
            a.run("DROP INDEX I").unwrap();
        }
        a.run("DELETE FROM U WHERE K = 1000").unwrap();
        a.commit().unwrap();
    }
}
