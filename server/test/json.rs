//! SQL/JSON over the wire: what a path's exception condition reaches a
//! client as.

use turborel::db::Database;
use turborel_server::settings::WORKERS;

use crate::common::start;

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 B; §9.48 GR5c; §23.1]
/// the SQLSTATE of a path's exception condition, raised ERROR ON ERROR,
/// reaches the client with its SQLCODE: 22039 of a strict array accessor
/// of an object, data exception -301; NULL ON ERROR gives the null value
/// instead.
#[test]
fn a_path_exception_reaches_the_client() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X").unwrap();
    let server = start(&db, &[(WORKERS, 1)]);
    let mut c = server.connect("X");
    let failed = c.query("VALUES JSON_VALUE('{}', 'strict $[0]' ERROR ON ERROR)").unwrap_err();
    assert_eq!((failed.sqlstate.as_str(), failed.sqlcode), ("22039", -301_i32), "{failed}");
    let shown = c.query("VALUES JSON_VALUE('{}', 'strict $[0]' NULL ON ERROR)").unwrap();
    assert_eq!(crate::common::rows(&shown), ["NULL"]);
}
