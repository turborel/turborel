//! Declared types over a real connection (RULES.md D396): a client
//! granted `declared-types` is told each column's declared type in a
//! Description and a Prepared, and one not granted it reads the bytes it
//! always read.

use std::io::Read;

use turborel::db::Database;
use turborel_client::connection::{Outcome, Trust};
use turborel_client::told::Offer;
use turborel_protocol::capability::DECLARED_TYPES;
use turborel_protocol::{Declared, HEADER, Reply, Request, header};
use turborel_test_support::server::Raw;

use crate::common::{Running, start};

/// The query of every column of T.
const EVERY: &str = "SELECT I, S, B, D, N, R, P, F, M FROM T";

/// A database whose schema Z holds T, a column of each name an exact or
/// approximate number is declared with, and one of a distinct type, and a
/// row of it.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION Z CREATE TYPE MONEY AS DECIMAL(9,2) CREATE TABLE T (I INTEGER, S SMALLINT, B BIGINT, \
         D DECIMAL(9,2), N NUMERIC(5), R REAL, P DOUBLE PRECISION, F FLOAT(20), M MONEY)",
    )
    .unwrap();
    db
}

/// The name and the text of each declared type T's columns have; FLOAT(20)
/// is held as FLOAT(24), the precision of its values.
fn expected() -> Vec<(&'static str, &'static str)> {
    vec![
        ("INTEGER", "INTEGER"),
        ("SMALLINT", "SMALLINT"),
        ("BIGINT", "BIGINT"),
        ("DECIMAL", "DECIMAL(9,2)"),
        ("NUMERIC", "NUMERIC(5,0)"),
        ("REAL", "REAL"),
        ("DOUBLE PRECISION", "DOUBLE PRECISION"),
        ("FLOAT", "FLOAT(24)"),
        ("USER-DEFINED", "Z.MONEY"),
    ]
}

/// The next frame `raw` reads, whole, its header too.
fn frame(raw: &mut Raw) -> Vec<u8> {
    let mut head = [0_u8; HEADER];
    raw.0.read_exact(&mut head).unwrap();
    let (_, length) = header(head, u32::MAX).unwrap();
    let mut body = vec![0_u8; length];
    raw.0.read_exact(&mut body).unwrap();
    [&head[..], &body[..]].concat()
}

/// The reply `bytes` frame.
fn decoded(bytes: &[u8]) -> Reply {
    Reply::decode(bytes[0], &bytes[HEADER..]).unwrap()
}

/// The frames of `sql`'s Description and Rows, over a raw connection as
/// Z, asking `capabilities`.
fn described(server: &Running, capabilities: &[&str], sql: &str) -> (Vec<u8>, Vec<u8>) {
    let mut raw = server.raw();
    let capabilities = capabilities.iter().map(|c| (*c).to_owned()).collect();
    let hello =
        Request::Hello { version: 1, user: "Z".into(), mechanism: "trust".into(), initial: vec![], capabilities, attributes: vec![] };
    raw.send(&hello);
    assert!(matches!(raw.receive(), Some(Reply::Ready { .. })));
    raw.send(&Request::Query { sql: sql.into(), ends: true, attributes: vec![], multi: false });
    (frame(&mut raw), frame(&mut raw))
}

/// Verifies: [DESIGN] RULES.md D396: a client not granted
/// `declared-types` is sent a Description with no tail, its columns of
/// no declared type, and a client granted it the same bytes, then tag 19
/// and the declared type of each column; the rows are the same bytes for
/// both, as values are written as ever.
#[test]
fn declared_types_are_told_only_to_a_client_that_asks() {
    let db = database();
    db.session("Z").direct("INSERT INTO T VALUES (1, 2, 3, 4.5, 6, 7.5, 8.5, 9.5, 10.25)").unwrap();
    let server = start(&db, &[]);
    let (plain, plain_rows) = described(&server, &[], EVERY);
    let (told, told_rows) = described(&server, &[DECLARED_TYPES], EVERY);
    let Reply::Description(columns) = decoded(&plain) else { panic!("no Description") };
    assert!(columns.iter().all(|c| c.declared.is_none()), "none told");
    assert_eq!(columns[0].data_type.to_string(), "NUMERIC(10,0)", "INTEGER is an exact numeric type of its precision");
    assert_eq!(told[HEADER..plain.len()], plain[HEADER..], "the same columns first");
    assert_eq!(told[plain.len()], 19, "then the tail of declared types");
    let Reply::Description(columns) = decoded(&told) else { panic!("no Description") };
    let declared: Vec<(String, String)> = columns.iter().map(|c| c.declared.clone().map(|d| (d.name, d.written)).unwrap()).collect();
    let expected: Vec<(String, String)> = expected().into_iter().map(|(n, w)| (n.to_owned(), w.to_owned())).collect();
    assert_eq!(declared, expected);
    assert_eq!(told_rows, plain_rows, "values as ever");
}

/// Verifies: [DESIGN] RULES.md D396: a statement prepared, and described
/// again by Describe, carries the declared type of each column to a
/// client granted `declared-types`, as the client asks by default; a
/// column's type is written as declared then, and as its tag says to a
/// client that asked for nothing; a statement with no columns carries no
/// tail.
#[test]
fn the_client_asks_for_declared_types_and_shows_them() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = turborel_client::connection::connect(server.address, "Z", &mut Trust).unwrap();
    let statement = c.prepare(EVERY).unwrap();
    let written: Vec<String> = statement.columns().iter().map(turborel_protocol::Column::type_written).collect();
    assert_eq!(written, expected().into_iter().map(|(_, w)| w.to_owned()).collect::<Vec<_>>());
    let again = c.describe(&statement).unwrap();
    assert_eq!(again.columns()[0].declared, Some(Declared { name: "INTEGER".into(), written: "INTEGER".into() }));
    let Outcome::Rows { columns, .. } = c.query("SELECT S, M FROM T").unwrap() else { panic!("no rows") };
    assert_eq!(columns[1].declared.as_ref().map(|d| d.name.as_str()), Some("USER-DEFINED"));
    let none = c.prepare("DELETE FROM T").unwrap();
    assert_eq!(none.columns().len(), 0);
    let mut first = offering(&server, Offer::none());
    let plain = first.prepare(EVERY).unwrap();
    assert_eq!(plain.columns()[0].declared, None);
    assert_eq!(plain.columns()[0].type_written(), "NUMERIC(10,0)");
}

/// A connection to `server` as Z, asking what `offer` asks.
fn offering(server: &Running, offer: Offer) -> turborel_client::connection::Connection<std::net::TcpStream> {
    let stream = std::net::TcpStream::connect(server.address).unwrap();
    turborel_client::connection::Connection::open_offering(stream, ("Z", &mut Trust), offer).unwrap()
}
