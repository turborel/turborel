//! The protocol mode's terminals: each a connection over loopback TCP to
//! a server of the case's database, started in this process, logged in by
//! trust as a client of this machine. A statement that ends its
//! transaction is sent with the request's ends flag, so the server commits
//! it, or rolls it back, in the same request.

use std::net::TcpStream;

use turborel::db::Database;
use turborel::value::Value;
use turborel_client::connection::{Connection, Outcome};
use turborel_client::statements::Statement;
use turborel_server::settings::MAX_CONNECTIONS;
use turborel_test_support::server::{Running, start};

use crate::cases::Serve;
use crate::client::{Client, Connect, Failed};

/// Connections the server takes: every terminal, and the case's own.
const CONNECTIONS: i128 = 256;

/// Terminals through the protocol.
pub struct Protocol;

impl Serve for Protocol {
    type Served = Served;

    fn serve(&self, db: Database) -> Served {
        Served(start(&db, &[(MAX_CONNECTIONS, CONNECTIONS)]))
    }
}

/// A server of a case's database, stopped when dropped.
pub struct Served(Running);

impl Connect for Served {
    type Client = Remote;

    fn connect(&self, user: &str) -> Remote {
        Remote(self.0.connect(user))
    }
}

/// A connection to the server.
pub struct Remote(Connection<TcpStream>);

impl Client for Remote {
    type Statement = Statement;

    fn prepare(&mut self, sql: &str) -> Result<Statement, Failed> {
        self.0.prepare(sql).map_err(|e| Failed(e.sqlcode))
    }

    fn execute(&mut self, statement: &Statement, values: &[Value], ends: bool) -> Result<u64, Failed> {
        self.0.set_autocommit(ends);
        match self.0.execute(statement, values).map_err(|e| Failed(e.sqlcode))? {
            Outcome::Rows { rows, .. } => Ok(rows.len() as u64),
            Outcome::Done(_) => Ok(0),
        }
    }

    fn direct(&mut self, sql: &str, ends: bool) -> Result<Vec<Vec<Value>>, Failed> {
        self.0.set_autocommit(ends);
        match self.0.query(sql).map_err(|e| Failed(e.sqlcode))? {
            Outcome::Rows { rows, .. } => Ok(rows),
            Outcome::Done(_) => Ok(Vec::new()),
        }
    }

    fn rollback(&mut self) -> Result<(), Failed> {
        self.0.set_autocommit(false);
        self.0.query("ROLLBACK").map(drop).map_err(|e| Failed(e.sqlcode))
    }
}
