//! Parameter modes over a real connection (RULES.md D977, D978): a client
//! granted `parameter-modes` is told each `?` of a prepared CALL with its
//! parameter's mode and ordinal position and the procedure's specific
//! name, and reads the output ones' values as a row; one not granted it
//! is told the input ones alone, as ever.

use std::net::TcpStream;

use turborel::catalog::ParameterMode;
use turborel::db::Database;
use turborel::value::{Decimal, Value};
use turborel_client::connection::{Connection, Outcome, Trust};
use turborel_client::statements::Statement;
use turborel_client::told::Offer;
use turborel_protocol::capability::PARAMETER_MODES;

use crate::common::{Running, start};

/// A database whose schema X holds T (K) of rows 1 to 3 and MIXED (OUT,
/// IN, INOUT).
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let x = db.session("X");
    x.run("INSERT INTO T VALUES (1), (2), (3)").unwrap();
    x.run(
        "CREATE PROCEDURE MIXED (OUT TOTAL INTEGER, IN TIMES INTEGER, INOUT TAG VARCHAR(10)) SPECIFIC MIXED READS SQL DATA \
         SELECT SUM(K) * TIMES, TAG || 'X' INTO TOTAL, TAG FROM T",
    )
    .unwrap();
    x.commit().unwrap();
    db
}

/// A connection to `server` as X, asking what `offer` asks.
fn offering(server: &Running, offer: Offer) -> Connection<TcpStream> {
    let stream = TcpStream::connect(server.address).unwrap();
    Connection::open_offering(stream, ("X", &mut Trust), offer).unwrap()
}

/// Each of `s`'s `?`s: its parameter's mode and ordinal position.
fn modes(s: &Statement) -> Vec<(ParameterMode, Option<u32>)> {
    s.parameters().iter().map(|p| (p.mode, p.ordinal)).collect()
}

/// Verifies: [DESIGN] [ISO/IEC 9075-2:2023 §20.10 GR7e, §20.7 GR7a]
/// RULES.md D977, D978: granted `parameter-modes`, a client is told each
/// `?` of a prepared CALL with its mode and ordinal position, the OUT
/// one's too, and the procedure's specific name; executed with values for
/// the IN and INOUT ones, the OUT and INOUT ones' come back as a row; a
/// query's `?`s are IN, of no procedure. Not granted it, a client is told
/// the input `?`s alone, each IN, and reads the outputs as ever.
#[test]
fn a_prepared_call_tells_its_parameters_modes() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    assert!(c.granted().has(PARAMETER_MODES));
    let mixed = c.prepare("CALL MIXED(?, ?, ?)").unwrap();
    let (in_, out, both) = (ParameterMode::In, ParameterMode::Out, ParameterMode::InOut);
    assert_eq!(modes(&mixed), [(out, Some(1)), (in_, Some(2)), (both, Some(3))]);
    assert_eq!((mixed.inputs().len(), mixed.procedure().map(|n| n.ident.as_str())), (2, Some("MIXED")));
    let given = [Value::Exact(Decimal::new(2, 0)), Value::Char("A".into())];
    let Outcome::Rows { rows, .. } = c.execute(&mixed, &given).unwrap() else { panic!("rows") };
    assert_eq!(rows[0].iter().map(ToString::to_string).collect::<Vec<_>>(), ["12", "AX"]);
    let query = c.prepare("SELECT K FROM T WHERE K = ?").unwrap();
    assert_eq!((modes(&query), query.procedure()), (vec![(in_, None)], None));
    let mut plain = offering(&server, Offer::none());
    assert!(!plain.granted().has(PARAMETER_MODES));
    let untold = plain.prepare("CALL MIXED(?, ?, ?)").unwrap();
    assert_eq!((modes(&untold), untold.procedure()), (vec![(in_, None), (in_, None)], None));
    let Outcome::Rows { rows, .. } = plain.execute(&untold, &given).unwrap() else { panic!("rows") };
    assert_eq!(rows[0].len(), 2, "the outputs as ever");
}
