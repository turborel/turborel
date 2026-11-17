//! XML values kept [ISO/IEC 9075-14:2006 draft §4.2.3] (RULES.md D86):
//! in the row when short, else as a table's large objects, through a
//! crash, a clean close, an update, a delete and a rollback; inside an
//! array, a multiset, a row or a structured value, in that value's row;
//! refused by the column store; described in SQL descriptor areas.

use std::fmt::Write;

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;
use turborel::value::xml::{Typing, XmlType, XmlValue};
use turborel::value::{DataType, Value};

use crate::common::{Scratch, code_with, dynamic_host, os, rows, run_with as run, set_text as text, sorted};

/// A document of `n` elements, each with an attribute and text, its
/// stored form longer than `n` times 20 octets.
fn long_document(n: usize) -> String {
    let mut document = String::from("<list>");
    for i in 0..n {
        write!(document, "<item n=\"{i}\">value {i}</item>").unwrap();
    }
    document + "</list>"
}

/// The INSERT of key `k` and the document `document` into T.
fn insert(k: usize, document: &str) -> String {
    format!("INSERT INTO T VALUES ({k}, XMLPARSE(DOCUMENT '{document}' PRESERVE WHITESPACE))")
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.3, §10.2 GR1b] [DESIGN]
/// RULES.md D79, D86: XML values are kept: one short, in its row; one
/// longer than a row holds and one of several chunks, as the table's
/// large objects; each read back whole after a crash and after a clean
/// close; an update replaces one, a delete removes one, and a rollback
/// keeps what was there.
#[test]
fn values_are_kept_in_rows_and_large_objects() {
    let scratch = Scratch::new("xml");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let (short, row, chunks) = ("<a>short</a>".to_owned(), long_document(400), long_document(20_000));
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML)").unwrap();
    let a = db.session("A");
    for (k, document) in [(1, &short), (2, &row), (3, &chunks)] {
        a.run(&insert(k, document)).unwrap();
    }
    a.commit().unwrap();
    drop((a, db));
    let serialized = "SELECT K, XMLSERIALIZE(DOCUMENT X AS CLOB) FROM T";
    let kept = |db: &Database, expected: &[(usize, &String)]| {
        let a = db.session("A");
        let wanted: Vec<String> = expected.iter().map(|(k, d)| format!("{k},{d}")).collect();
        assert_eq!(sorted(&a, serialized), wanted);
        a.rollback().unwrap();
    };
    let db = open();
    kept(&db, &[(1, &short), (2, &row), (3, &chunks)]);
    let a = db.session("A");
    a.run(&format!("UPDATE T SET X = XMLPARSE(DOCUMENT '{short}' PRESERVE WHITESPACE) WHERE K = 3")).unwrap();
    a.run("DELETE FROM T WHERE K = 2").unwrap();
    a.commit().unwrap();
    a.run(&insert(4, &chunks)).unwrap();
    a.run("DELETE FROM T WHERE K = 1").unwrap();
    a.rollback().unwrap();
    drop(a);
    db.close().unwrap();
    drop(db);
    kept(&open(), &[(1, &short), (3, &short)]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.3, §10.2 GR1b] [DESIGN]
/// RULES.md D79, D86: an XML value of many chunks, copied from a table's
/// column into an array, a multiset, a row and a structured value, is
/// held in its row, so each is whole after its source row is deleted, and
/// after a crash; an update rolled back leaves it. A regression test: each
/// named the source's large object, and failed to be read, SQLCODE -916.
#[test]
fn values_inside_other_values_are_kept_with_their_rows() {
    let scratch = Scratch::new("xml-inside");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let document = long_document(20_000);
    let crashed = open();
    crashed
        .schema(
            "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML) \
             CREATE TYPE ST AS (X XML(CONTENT(ANY)), N INTEGER) NOT FINAL \
             CREATE TABLE U (K INTEGER, A XML ARRAY[2], M XML MULTISET, R ROW(F XML, N INTEGER), S ST)",
        )
        .unwrap();
    let a = crashed.session("A");
    a.run(&insert(1, &document)).unwrap();
    a.commit().unwrap();
    a.run("INSERT INTO U SELECT K, ARRAY[X, CAST(NULL AS XML)], MULTISET[X], ROW(X, K), NEW ST().X(X) FROM T").unwrap();
    a.run("DELETE FROM T").unwrap();
    a.commit().unwrap();
    a.run("UPDATE U SET A = NULL").unwrap();
    a.rollback().unwrap();
    drop((a, crashed));
    let db = open();
    let a = db.session("A");
    let whole = |x: &str| format!("XMLSERIALIZE(DOCUMENT {x} AS CLOB)");
    let read = format!("SELECT {}, A[2] IS NULL, {}, {} FROM U", whole("A[1]"), whole("U.R.F"), whole("U.S.X"));
    assert_eq!(rows(&a, &read).unwrap(), [format!("{document},TRUE,{document},{document}")]);
    let elements = format!("SELECT {} FROM U, UNNEST(U.M) AS E(X)", whole("E.X"));
    assert_eq!(rows(&a, &elements).unwrap(), [document]);
}

/// Verifies: [DESIGN] RULES.md D31, D86: the column store keeps no XML
/// value, as it keeps no large object, nor one inside an array, a
/// multiset or a row; nor a large object inside one.
#[test]
fn the_column_store_keeps_no_xml() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    for t in ["XML", "XML ARRAY[2]", "XML MULTISET", "ROW(F INTEGER, G XML)", "BLOB ARRAY[2]"] {
        let error = a.run(&format!("CREATE TABLE C (K INTEGER, V {t}) USING columnstore")).unwrap_err();
        assert_eq!(error.code, Code::Unsupported, "{t}: {error}");
        assert!(error.to_string().contains("XML value"), "{error}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §17.1 GR1, Table 12] [DESIGN]
/// RULES.md D86: an item of XML type is described by the code 137; one
/// set to 137 describes XML, XML(SEQUENCE).
#[test]
fn descriptors_describe_xml_by_137() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML(DOCUMENT))").unwrap();
    let s = db.session("A");
    let mut p = dynamic_host();
    text(&mut p, "SELECT X FROM T");
    run(&s, "PREPARE Q FROM TEXT", &mut p).unwrap();
    run(&s, "ALLOCATE DESCRIPTOR 'D' WITH MAX 2", &mut p).unwrap();
    run(&s, "DESCRIBE Q USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    s.execute("GET DESCRIPTOR 'D' VALUE 1 N = TYPE", &mut p).unwrap();
    assert_eq!(p.value("N").to_string(), "137");
    let document = Value::Xml(XmlValue::serialized("<a/>"));
    let mut q = dynamic_host().declare("H", DataType::Xml(XmlType::Document(Typing::Any)), document);
    text(&mut q, "INSERT INTO T (X) VALUES (?)");
    run(&s, "PREPARE I FROM TEXT", &mut q).unwrap();
    run(&s, "ALLOCATE DESCRIPTOR 'E' WITH MAX 1", &mut q).unwrap();
    run(&s, "SET DESCRIPTOR 'E' COUNT = 1", &mut q).unwrap();
    run(&s, "SET DESCRIPTOR 'E' VALUE 1 TYPE = 137, DATA = H", &mut q).unwrap();
    run(&s, "EXECUTE I USING SQL DESCRIPTOR 'E'", &mut q).unwrap();
    assert_eq!(rows(&s, "SELECT XMLSERIALIZE(X AS VARCHAR(9)) FROM T").unwrap(), ["<a/>"]);
    assert_eq!(code_with(&s, "SET DESCRIPTOR 'E' VALUE 1 TYPE = 4, DATA = H", &mut q), Code::Data);
}
