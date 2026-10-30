//! What a server shows of itself (RULES.md D42, D55): its connections, in
//! INFORMATION_SCHEMA.CONNECTIONS, and its counts, in
//! INFORMATION_SCHEMA.SERVER, to the database's owner.

use turborel::db::Database;
use turborel_client::connection::{self, Trust};
use turborel_server::settings::MAX_CONNECTIONS;

use crate::common::{rows, start, state};

/// Verifies: [DESIGN] RULES.md D42, D55, D43: a server shows the
/// database's owner each connection it serves, its user, whether it is
/// secure, and whether it runs a statement; and its counts: workers
/// running a statement, statements waiting, connections, and those
/// refused past SERVER.MAX_CONNECTIONS. Another user sees none of them.
#[test]
fn a_server_shows_its_connections_to_the_owner() {
    let db = Database::new();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    let server = start(&db, &[(MAX_CONNECTIONS, 2)]);
    let mut owner = server.connect("OWNER");
    let mut other = server.connect("X");
    let connections = "SELECT ID, USER_NAME, SECURE, STATE FROM INFORMATION_SCHEMA.CONNECTIONS ORDER BY ID";
    assert_eq!(rows(&owner.query(connections).unwrap()), ["1,OWNER,NO,RUNNING", "2,X,NO,IDLE"]);
    let refused = connection::connect(server.address, "Y", &mut Trust).err().unwrap();
    assert_eq!(state(&refused).0, "08004");
    let counts = "SELECT WORKERS_BUSY, STATEMENTS_WAITING, CONNECTIONS, REFUSED, CLOSED_UNTOLD FROM INFORMATION_SCHEMA.SERVER";
    assert_eq!(rows(&owner.query(counts).unwrap()), ["1,0,2,1,0"], "the owner's statement runs");
    assert_eq!(rows(&other.query(connections).unwrap()), Vec::<String>::new(), "not the owner");
    assert_eq!(rows(&other.query(counts).unwrap()), Vec::<String>::new());
}
