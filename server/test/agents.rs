//! The client library's SQL-agent [ISO/IEC 9075:1992 §15] (RULES.md D46):
//! its connections to servers, made, chosen and ended by the connection
//! statements, which it runs itself.

use std::collections::HashMap;
use std::net::SocketAddr;

use turborel::db::Database;
use turborel_client::agent::{Agent, Connected, Connector};
use turborel_client::connection::{Outcome, Result, Trust};
use turborel_client::target::{Target, connect_to};
use turborel_protocol::Failure;

use crate::common::{Running, start};

/// A connector to the servers it names, by name; a server it does not
/// know refuses, 28000.
struct Named(HashMap<&'static str, SocketAddr>);

impl Connector for Named {
    fn connect(&mut self, server: Option<&str>, user: Option<&str>) -> Result<Connected> {
        let Some(address) = self.0.get(server.unwrap_or("default")) else {
            return Err(Failure {
                sqlcode: -919,
                sqlstate: "28000".into(),
                message: "no".into(),
                rule: None,
                position: None,
                active: false,
            });
        };
        connect_to(&Target::Tcp(*address), user.unwrap_or("X"), &mut Trust)
    }
}

/// A server whose X.T holds one row, which PUBLIC may read.
fn served() -> (Database, Running) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER) GRANT SELECT ON T TO PUBLIC").unwrap();
    db.session("X").run("INSERT INTO T VALUES (1)").unwrap();
    db.session("X").commit().ok();
    let server = start(&db, &[]);
    (db, server)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §15.1 GR8-GR11, §15.3 GR8,
/// GR9]; [DESIGN] RULES.md D46: an agent begins with no connection, so a
/// statement has none current (08003); CONNECT makes one current, named,
/// and a statement runs on it; a server that refuses gives 08004; a
/// connection name or server not a literal is refused (SQLCODE -103).
/// Ending a connection whose server is gone is a warning, 01002, and
/// ending the agent tells the first failure, each connection ended still.
#[test]
fn an_agent_holds_connections_to_servers() {
    let (_one, first) = served();
    let (_two, second) = served();
    let named = HashMap::from([("default", first.address), ("second", second.address)]);
    let mut agent = Agent::new(Named(named));
    assert_eq!((agent.current_name(), agent.canceller().is_some(), agent.is_lost()), (None, false, false));
    assert_eq!(agent.query("SELECT K FROM X.T").unwrap_err().sqlstate, "08003");
    assert!(matches!(agent.query("CONNECT TO DEFAULT"), Ok(Outcome::Done(_))));
    assert_eq!((agent.current_name(), agent.canceller().is_some()), (None, true), "the default connection");
    assert!(matches!(agent.query("SELECT K FROM X.T"), Ok(Outcome::Rows { rows, .. }) if rows.len() == 1));
    agent.query("COMMIT").unwrap();
    assert_eq!(agent.query("CONNECT TO 'elsewhere' AS 'E'").unwrap_err().sqlstate, "08004", "refused");
    assert_eq!(agent.query("CONNECT TO 1 AS 'E'").unwrap_err().sqlcode, -103);
    assert_eq!(agent.query("SET CONNECTION 1").unwrap_err().sqlcode, -103);
    for name in ["S", "S2"] {
        agent.query(&format!("CONNECT TO 'second' AS '{name}'")).unwrap();
    }
    assert_eq!(agent.current_name(), Some("S2"));
    second.stop();
    assert_eq!(agent.query("SELECT K FROM X.T").unwrap_err().sqlstate, "08006");
    assert!(agent.is_lost());
    agent.query("SET CONNECTION 'S'").unwrap();
    assert_eq!(agent.query("SELECT K FROM X.T").unwrap_err().sqlstate, "08006");
    assert!(matches!(agent.query("DISCONNECT CURRENT"), Ok(Outcome::Done(c)) if c.sqlstate == "01002"), "a warning");
    assert_eq!((agent.current_name(), agent.is_lost()), (None, false), "none current");
    let failed = agent.terminate().unwrap_err();
    assert_eq!(failed.sqlstate, "08006", "S2's");
}
