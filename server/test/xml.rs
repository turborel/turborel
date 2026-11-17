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
