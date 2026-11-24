//! Exact numbers of up to 38 digits over a connection (RULES.md D695): a
//! value's units go as 128 bits, so every digit reaches the client and
//! comes back as a parameter; a literal or a parameter of more is refused
//! with the state the standard gives it (RULES.md D970, D971).

use turborel::db::Database;
use turborel::value::{DataType, Decimal, Value};
use turborel_client::connection::Outcome;

use crate::common::{done, rows, start, state};

/// The units and scale of `v`, if it is an exact number.
fn exact(v: Option<&Value>) -> Option<(i128, u32)> {
    if let Some(Value::Exact(d)) = v { Some((d.units, d.scale)) } else { None }
}

/// 10^38 - 1, the greatest units of 38 digits.
const MOST: i128 = 99_999_999_999_999_999_999_999_999_999_999_999_999;

/// Verifies: [ISO/IEC 9075-2:2023 §6.1 SR27a] RULES.md D42, D695: a
/// DECIMAL(38,10) column is described with its precision and scale; its
/// values of 38 digits, either sign, reach the client whole, as their
/// text and as their units; a value of 38 digits sent as a prepared
/// statement's parameter is kept as sent, and a sum past 38 digits is
/// SQLCODE -301.
#[test]
fn wide_exact_numbers_go_both_ways() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL, D DECIMAL(38,10))").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let most = "9999999999999999999999999999.9999999999";
    assert_eq!(done(&c.query(&format!("INSERT INTO T VALUES (1, {most}), (2, -{most})")).unwrap()), 0);
    let prepared = c.prepare("INSERT INTO T VALUES (?, ?)").unwrap();
    let sent = Value::Exact(Decimal::new(-MOST, 10));
    assert_eq!(done(&c.execute(&prepared, &[Value::Exact(Decimal::new(3, 0)), sent.clone()]).unwrap()), 0);
    let out = c.query("SELECT K, D FROM T ORDER BY K").unwrap();
    assert_eq!(rows(&out), [format!("1,{most}"), format!("2,-{most}"), format!("3,-{most}")]);
    let Outcome::Rows { columns, rows: received, .. } = &out else { panic!("no rows") };
    assert!(matches!(columns[1].data_type, DataType::Exact { precision: 38, scale: 10, .. }), "{:?}", columns[1].data_type);
    assert_eq!(exact(received.get(2).and_then(|r| r.get(1))), exact(Some(&sent)));
    assert_eq!(exact(received.first().and_then(|r| r.get(1))), Some((MOST, 10)));
    assert_eq!(state(&c.query("SELECT D + D FROM T").unwrap_err()), ("22003", -301));
    c.terminate().unwrap();
}

/// Verifies: [ISO/IEC 9075-2:2023 §5.3 SR29, SR30, §6.13 GR8a ii, §20.7
/// GR6c, §22.1 GR6a] RULES.md D970, D971: over a connection, a numeric
/// literal no type holds is SQLSTATE 42000 and SQLCODE -103, sent as a
/// query and prepared alike, and the text of one sent as a parameter is
/// 22003, as an exact value past the type is; a query naming no table is
/// 42000 sent and prepared alike, as a regression test: it was 37000
/// prepared, and the literal 0A000, before the fix.
#[test]
fn the_states_of_rules_broken_and_numbers_too_wide_cross_the_wire() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL, D DECIMAL(38,0))").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let wide = format!("{MOST}9");
    for sql in [format!("SELECT {wide} FROM T"), "SELECT 1E400 FROM T".to_owned(), "SELECT K FROM NOWHERE".to_owned()] {
        assert_eq!(state(&c.query(&sql).unwrap_err()), ("42000", -103), "{sql} sent");
        assert_eq!(state(&c.prepare(&sql).unwrap_err()), ("42000", -103), "{sql} prepared");
    }
    let insert = c.prepare("INSERT INTO T VALUES (1, ?)").unwrap();
    assert_eq!(state(&c.execute(&insert, &[Value::Char(wide.as_str().into())]).unwrap_err()), ("22003", -301));
    assert_eq!(state(&c.execute(&insert, &[Value::Exact(Decimal::new(i128::MAX, 0))]).unwrap_err()), ("22003", -301));
    assert_eq!(done(&c.execute(&insert, &[Value::Char(MOST.to_string().as_str().into())]).unwrap()), 0);
    c.terminate().unwrap();
}
