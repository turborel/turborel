//! XML values over a connection [ISO/IEC 9075-14:2006 draft §4.2.3]
//! (RULES.md D86): sent as their serialization, read back as XML values,
//! and sent to a prepared statement.

use turborel::db::Database;
use turborel::value::xml::{Typing, XmlType, XmlValue};
use turborel::value::{DataType, Value};
use turborel_client::connection::Outcome;

use crate::common::{done, rows, start};

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.2, §10.15] [DESIGN] RULES.md
/// D42, D86: a column of an XML type is described as its type, with its
/// modifiers; its values reach the client as their serialization, which
/// the client holds as XML values; an XML value sent as a prepared
/// statement's parameter is kept as sent; the null value is null.
#[test]
fn xml_values_go_both_ways() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL, D XML(DOCUMENT(UNTYPED)))").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let insert = "INSERT INTO T VALUES (1, XMLPARSE(DOCUMENT '<a x=\"&amp;\">t&#x2028;</a>' PRESERVE WHITESPACE)), (2, NULL)";
    assert_eq!(done(&c.query(insert).unwrap()), 0);
    let out = c.query("SELECT K, D FROM T ORDER BY K").unwrap();
    assert_eq!(rows(&out), ["1,<a x=\"&amp;\">t&#x2028;</a>", "2,NULL"]);
    let Outcome::Rows { columns, rows: received, .. } = &out else { panic!("no rows") };
    assert_eq!(columns[1].data_type, DataType::Xml(XmlType::Document(Typing::Untyped)));
    let Some(Value::Xml(sent)) = received.first().and_then(|r| r.get(1)) else { panic!("{received:?}") };
    assert!(sent.sequence().unwrap().is_document(), "the client parses what it is sent");
    let prepared = c.prepare("INSERT INTO T VALUES (?, ?)").unwrap();
    assert_eq!(prepared.parameters()[1].clone().data_type(), DataType::Xml(XmlType::Document(Typing::Untyped)));
    let value = Value::Xml(XmlValue::serialized("<b>sent</b>"));
    assert_eq!(done(&c.execute(&prepared, &[Value::Exact(turborel::value::Decimal::new(3, 0)), value]).unwrap()), 0);
    assert_eq!(rows(&c.query("SELECT XMLSERIALIZE(D AS VARCHAR(20)) FROM T WHERE K = 3").unwrap()), ["<b>sent</b>"]);
    c.terminate().unwrap();
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.3, §6.1 CR2, §6.2 CR1,
/// §12.5 CR1] [DESIGN] RULES.md D45, D69, D86: an array and a row of XML
/// values, and a value of a distinct type of XML, reach the client with
/// each XML value its serialization; the distinct type is described as
/// its source; an array of XML values sent as a prepared statement's
/// parameter is kept as sent.
#[test]
fn xml_inside_other_values_goes_both_ways() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION X CREATE TYPE DX AS XML(CONTENT(ANY)) FINAL \
         CREATE TABLE T (K INTEGER, A XML(CONTENT(ANY)) ARRAY[2], R ROW(F XML(CONTENT(ANY)), N INTEGER), D DX)",
    )
    .unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let parsed = "XMLPARSE(CONTENT '<a>&amp;</a>' PRESERVE WHITESPACE)";
    let insert = format!("INSERT INTO T VALUES (1, ARRAY[{parsed}], ROW({parsed}, 2), CAST({parsed} AS DX))");
    assert_eq!(done(&c.query(&insert).unwrap()), 0);
    let out = c.query("SELECT A, R, D FROM T").unwrap();
    let Outcome::Rows { columns, rows: received, .. } = &out else { panic!("no rows") };
    assert_eq!(columns[2].data_type, DataType::Xml(XmlType::Content(Typing::Any)));
    let Some([Value::Array(array), Value::Row(row), distinct]) = received.first().map(Vec::as_slice) else { panic!("{received:?}") };
    for value in [&array[0], &row[0], distinct] {
        let Value::Xml(xml) = value else { panic!("{value:?}") };
        assert_eq!(xml.text().unwrap(), "<a>&amp;</a>");
    }
    let prepared = c.prepare("INSERT INTO T (K, A) VALUES (2, ?)").unwrap();
    let sent = Value::Array(vec![Value::Xml(XmlValue::serialized("<b>sent</b>")), Value::Null].into());
    assert_eq!(done(&c.execute(&prepared, &[sent]).unwrap()), 0);
    let back = "SELECT XMLSERIALIZE(A[1] AS VARCHAR(20)), A[2] IS NULL FROM T WHERE K = 2";
    assert_eq!(rows(&c.query(back).unwrap()), ["<b>sent</b>,TRUE"]);
    c.terminate().unwrap();
}
