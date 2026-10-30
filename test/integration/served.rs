//! END CONNECTION and CANCEL CONNECTION (RULES.md D42): the database's
//! owner reaches the connections of the server that serves it, through
//! the `ServedConnections` it registered.

use std::sync::{Arc, Mutex};

use turborel::db::{Database, ServedConnections};
use turborel::error::Code;

use crate::common::exec;

/// A test-local `ServedConnections` (RULES.md D6) serving the connections
/// 1 and 2, and keeping what it was asked.
#[derive(Clone, Default)]
struct Asked(Arc<Mutex<Vec<String>>>);

impl Asked {
    fn served(&self, what: &str, id: u64) -> bool {
        self.0.lock().unwrap().push(format!("{what} {id}"));
        id <= 2
    }
}

impl ServedConnections for Asked {
    fn name(&self) -> &'static str {
        "asked"
    }
    fn end(&self, id: u64) -> bool {
        self.served("end", id)
    }
    fn cancel(&self, id: u64) -> bool {
        self.served("cancel", id)
    }
}

/// The SQLSTATE and the position of `sql`'s failure in `s`.
fn refused(s: &turborel::db::Session, sql: &str) -> (String, Option<usize>) {
    let e = s.run(sql).unwrap_err();
    (e.sqlstate().as_str().to_owned(), e.pos)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §22.1, Table 23]; [DESIGN]
/// RULES.md D42, D43: the owner ends a served connection, or cancels what
/// it runs, by its ID, through what the server registered; in a
/// transaction too, which goes on. Refused: by any but the owner (SQLCODE
/// -104), nothing asked; an ID no connection has, or any with no server
/// registered (SQLSTATE 08003), its position given.
#[test]
fn the_owner_ends_and_cancels_served_connections() {
    let db = Database::new();
    db.create_user("ANN", b"").unwrap();
    let ann = db.session("ANN");
    assert_eq!(refused(&ann, "END CONNECTION 1"), ("08003".to_owned(), Some(15)), "no server");
    let asked = Asked::default();
    db.set_served_connections(Box::new(asked.clone()));
    assert_eq!(exec(&db.session("BOB"), "CANCEL CONNECTION 1"), Err(Code::Privilege), "not the owner");
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE T (K INTEGER)").unwrap();
    exec(&ann, "INSERT INTO T VALUES (1)").unwrap();
    exec(&ann, "END CONNECTION 1").unwrap();
    exec(&ann, "cancel connection 2;").unwrap();
    assert!(ann.transaction().is_some(), "in a transaction, which goes on");
    assert_eq!(refused(&ann, "CANCEL CONNECTION 3").0, "08003");
    assert_eq!(*asked.0.lock().unwrap(), ["end 1", "cancel 2", "cancel 3"]);
    ann.commit().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D42, R-ERR-1: END CONNECTION and CANCEL
/// CONNECTION are refused as a Format broken (SQLCODE -102) without their
/// `<connection identifier>`, with one not an `<unsigned integer>`, or with
/// more after it; one past the largest is refused (-900). CANCEL is no
/// key word: a column may be named so.
#[test]
fn connection_statements_are_refused_saying_why() {
    let db = Database::new();
    let s = db.session("ANN");
    for wrong in ["END CONNECTION", "END CONNECTION X", "CANCEL CONNECTION 1.5", "END 1", "END CONNECTION 1 2"] {
        assert_eq!(exec(&s, wrong), Err(Code::Syntax), "{wrong}");
    }
    assert_eq!(exec(&s, "CANCEL CONNECTION 99999999999999999999"), Err(Code::Unsupported));
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE T (CANCEL INTEGER)").unwrap();
    exec(&s, "INSERT INTO T (CANCEL) VALUES (1)").unwrap();
    assert_eq!(exec(&s, "CANCEL T"), Err(Code::Syntax), "a word alone");
}
