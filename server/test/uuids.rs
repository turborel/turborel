//! UUIDs over a connection (RULES.md D184): described and sent under
//! their own tag, held by the client as UUIDs, and sent to a prepared
//! statement.

use turborel::db::Database;
use turborel::value::types::uuid::{UuidType, octets_of, parse, uuid_value};
use turborel::value::{Decimal, Value};
use turborel_client::connection::Outcome;

use crate::common::{done, rows, start};

/// A UUID of version 4 [RFC 9562 Appendix B.2].
const ONE: &str = "919108f7-52d1-4320-9bac-f847db4148a8";

/// Verifies: [EXTENSION] RULES.md D42, D184: a UUID column is described
/// as UUID; its values reach the client as UUIDs of their octets, shown
/// as their text; a UUID sent as a prepared statement's parameter, which
/// is described as UUID, is kept as sent; the null value is null.
#[test]
fn uuids_go_both_ways() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL, U UUID)").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    assert_eq!(done(&c.query(&format!("INSERT INTO T VALUES (1, UUID '{ONE}'), (2, NULL)")).unwrap()), 0);
    let out = c.query("SELECT K, U FROM T ORDER BY K").unwrap();
    assert_eq!(rows(&out), [format!("1,{ONE}"), "2,NULL".to_owned()]);
    let Outcome::Rows { columns, rows: received, .. } = &out else { panic!("no rows") };
    assert_eq!(columns[1].data_type, UuidType::data_type());
    assert_eq!(received.first().and_then(|r| r.get(1)).and_then(octets_of), parse(ONE));
    let prepared = c.prepare("INSERT INTO T VALUES (?, ?)").unwrap();
    assert_eq!(prepared.parameters()[1].clone().data_type(), UuidType::data_type());
    let sent = uuid_value([0xab; 16]);
    assert_eq!(done(&c.execute(&prepared, &[Value::Exact(Decimal::new(3, 0)), sent]).unwrap()), 0);
    let back = c.query("SELECT CAST(U AS VARCHAR(36)) FROM T WHERE K = 3").unwrap();
    assert_eq!(rows(&back), ["abababab-abab-abab-abab-abababababab"]);
    c.terminate().unwrap();
}
