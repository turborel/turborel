//! Conditions that can raise an error read through an index below their
//! guard (RULES.md D15): a conjunct of ON, or of WHERE over a product, of
//! one operand's columns, moved onto it over a guard of the others, finds
//! its rows by an index, and is still evaluated on every row the index
//! gives only when the others have rows, so the results, and the errors,
//! are those of a full read.

use turborel::db::{Database, Session};
use turborel::error::SqlState;
use turborel::params::Params;

use crate::common::{fetched, rows};

/// The XML methods, as CREATE INDEX names them.
const METHODS: [&str; 4] = ["XML_PATH", "XML_VALUE", "XML_PROPERTY", "XML_PRIMARY"];

/// How many rows table T has.
const ROWS: usize = 200;

/// Schema A: table T (K INTEGER, X XML(DOCUMENT)) of `ROWS` rows, each an
/// order of an item of a price; table E (K INTEGER), empty; table O (K
/// INTEGER) of the keys 31 to 50.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML(DOCUMENT)) CREATE TABLE E (K INTEGER) CREATE TABLE O (K INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    for k in 1..=ROWS {
        a.run(&format!("INSERT INTO T VALUES ({k}, XMLPARSE(DOCUMENT '<order><item price=\"{}\"/></order>' STRIP WHITESPACE))", k % 50))
            .unwrap();
    }
    for k in 31..=50 {
        a.run(&format!("INSERT INTO O VALUES ({k})")).unwrap();
    }
    a.commit().unwrap();
    db
}

/// The physical plan of `query`, as EXPLAIN shows it.
fn plan(a: &Session, query: &str) -> String {
    let explained = a.explain(query, &Params::new()).unwrap();
    explained.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// Queries whose condition on T may raise an error, a numeric
/// comparison's [XQuery §3.5.2], of T's column alone: of ON, of an inner
/// and of a LEFT join, and of WHERE over a product.
const QUERIES: [&str; 4] = [
    "SELECT O.K, T.K FROM O JOIN T ON O.K = T.K AND XMLEXISTS('//item[@price > 45]' PASSING BY REF T.X) ORDER BY O.K",
    "SELECT O.K, T.K FROM O LEFT JOIN T ON O.K = T.K AND XMLEXISTS('//item[@price > 44]' PASSING BY REF T.X) ORDER BY O.K",
    "SELECT O.K, T.K FROM O, T WHERE XMLEXISTS('//item[@price >= 47]' PASSING BY REF T.X) ORDER BY O.K, T.K",
    "SELECT O.K, T.K FROM O JOIN T ON XMLEXISTS('//item[@price = 48]' PASSING BY REF T.X) ORDER BY O.K, T.K",
];

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.5 GR1c] RULES.md D15,
/// D106: a condition of T's column that may raise an error, of ON or of
/// WHERE over a product, finds T's rows by each XML method's index, read
/// below its guard, EXPLAIN showing the index's read under the product
/// with the guard and the condition over it; the rows are a full read's.
#[test]
fn raising_conditions_read_by_an_index_below_their_guard() {
    let db = database();
    let a = db.session("A");
    let full: Vec<Vec<String>> = QUERIES.iter().map(|q| rows(&a, q).unwrap()).collect();
    assert!(full.iter().all(|r| !r.is_empty()), "{full:?}");
    assert!(plan(&a, QUERIES[0]).contains("SeqScan A.T"), "{}", plan(&a, QUERIES[0]));
    a.commit().unwrap();
    for method in METHODS {
        a.run(&format!("CREATE INDEX I ON T (X) USING {method}")).unwrap();
        for (query, expected) in QUERIES.iter().zip(&full) {
            assert_eq!(&rows(&a, query).unwrap(), expected, "{method}: {query}");
        }
        a.commit().unwrap();
        a.run("DROP INDEX I").unwrap();
    }
    a.run("CREATE INDEX I ON T (X) USING XML_PATH").unwrap();
    for query in QUERIES {
        let shown = plan(&a, query);
        assert!(shown.contains("IndexScan A.T using I [xml_path]"), "{query}: {shown}");
        assert!(shown.contains("Cartesian product") && shown.contains("EXISTS"), "below the guard: {query}: {shown}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.5 GR1c] [XQuery §3.5.2]
/// RULES.md D15, D106: a row of a value the condition cannot cast fails
/// the query, err:FORG0001, of an index read below the guard as of a full
/// read, while the other operand has a row; while it has none the
/// condition is evaluated on no row, and nothing fails, though the index
/// gives the row.
#[test]
fn errors_below_a_guard_are_a_full_reads() {
    let db = database();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1000, XMLPARSE(DOCUMENT '<order><item price=\"n/a\"/></order>' STRIP WHITESPACE))").unwrap();
    a.commit().unwrap();
    let joined = |other: &str| format!("SELECT {other}.K, T.K FROM {other} JOIN T ON XMLEXISTS('//item[@price = 48]' PASSING BY REF T.X)");
    for indexed in [false, true] {
        if indexed {
            a.run("CREATE INDEX I ON T (X) USING XML_VALUE").unwrap();
            assert!(plan(&a, &joined("E")).contains("IndexScan A.T using I"), "{}", plan(&a, &joined("E")));
        }
        let error = fetched(&a, &joined("O")).unwrap_err();
        assert_eq!(error.sqlstate(), SqlState::XQUERY_ERROR, "indexed {indexed}: {error}");
        assert!(error.msg.contains("err:FORG0001"), "{error}");
        a.rollback().unwrap();
        assert_eq!(fetched(&a, &joined("E")).unwrap(), 0, "indexed {indexed}: of no row of E, none evaluated");
        let left = "SELECT E.K, T.K FROM E LEFT JOIN T ON XMLEXISTS('//item[@price = 48]' PASSING BY REF T.X)";
        assert_eq!(fetched(&a, left).unwrap(), 0, "indexed {indexed}");
        let right = "SELECT O.K, T.K FROM O LEFT JOIN T ON O.K = T.K AND XMLEXISTS('//item[@price = 48]' PASSING BY REF T.X)";
        assert_eq!(fetched(&a, right).unwrap_err().sqlstate(), SqlState::XQUERY_ERROR, "indexed {indexed}");
        a.rollback().unwrap();
    }
}

/// Schema A: table DOCS (ID INTEGER, BODY FULLTEXT) of 300 documents, a
/// fox in every fiftieth, and table O of the keys 1 to 20.
fn documents() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE DOCS (ID INTEGER, BODY FULLTEXT) CREATE TABLE O (K INTEGER)").unwrap();
    let a = db.session("A");
    for id in 1..=300 {
        let animal = if id % 50 == 7 { "fox" } else { "dog" };
        a.run(&format!("INSERT INTO DOCS VALUES ({id}, FULLTEXT('a brown {animal} number {id}', 'NONE'))")).unwrap();
    }
    for k in 1..=20 {
        a.run(&format!("INSERT INTO O VALUES ({k})")).unwrap();
    }
    a.commit().unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 13249-2:2002 §5.1.2] RULES.md D15,
/// D34, D87: the guard is read below for any index method: a full-text
/// search of ON, which may raise an error of its pattern, finds its rows
/// by a full-text index below the guard, its rows a full read's; a
/// pattern that fails, fails alike while the other operand has a row.
#[test]
fn any_method_is_read_below_a_guard() {
    let db = documents();
    let a = db.session("A");
    let query = "SELECT O.K, DOCS.ID FROM O JOIN DOCS ON CONTAINS(DOCS.BODY, '\"fox\"') = 1 ORDER BY O.K, DOCS.ID";
    let full = rows(&a, query).unwrap();
    assert_eq!(full.len(), 20 * 6, "{full:?}");
    a.commit().unwrap();
    a.run("CREATE INDEX DOCS_BODY ON DOCS (BODY) USING fulltext").unwrap();
    assert_eq!(rows(&a, query).unwrap(), full);
    let shown = plan(&a, query);
    assert!(shown.contains("IndexScan A.DOCS using DOCS_BODY") && shown.contains("Cartesian product"), "{shown}");
    let bad = "SELECT O.K, DOCS.ID FROM O JOIN DOCS ON CONTAINS(DOCS.BODY, '\"fox') = 1";
    let unindexed = {
        a.commit().unwrap();
        a.run("DROP INDEX DOCS_BODY").unwrap();
        let e = fetched(&a, bad).unwrap_err().sqlstate();
        a.rollback().unwrap();
        a.run("CREATE INDEX DOCS_BODY ON DOCS (BODY) USING fulltext").unwrap();
        e
    };
    assert_eq!(fetched(&a, bad).unwrap_err().sqlstate(), unindexed);
}
