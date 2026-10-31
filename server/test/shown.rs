//! What a server shows of itself (RULES.md D42, D55): its connections, in
//! INFORMATION_SCHEMA.CONNECTIONS, and its counts, in
//! INFORMATION_SCHEMA.SERVER, to the database's owner.

use turborel::db::Database;
use turborel::value::Value;
use turborel_client::connection::{self, Trust};
use turborel_server::settings::{MAX_CONNECTIONS, SOCKET};

use crate::common::{pki, rows, secured, start, state};

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

/// Times a client connects and ends its connection.
const ENDED: u32 = 300;

/// Verifies: [DESIGN] RULES.md D42, D55: a connection's row of
/// INFORMATION_SCHEMA.CONNECTIONS goes before its client sees it closed:
/// once a client has ended its connection, the owner, reading embedded,
/// never finds it listed, nor counted in INFORMATION_SCHEMA.SERVER.
///
/// Regression test (R-TEST-7): the row went only once the connection's
/// task ended, after the socket closed, so it was listed a moment after.
#[test]
fn a_connection_its_client_saw_closed_is_not_listed() {
    let db = Database::new();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    let server = start(&db, &[]);
    let owner = db.session("OWNER");
    let listed = "SELECT (SELECT COUNT(*) FROM INFORMATION_SCHEMA.CONNECTIONS), CONNECTIONS FROM INFORMATION_SCHEMA.SERVER";
    for _ in 0..ENDED {
        server.connect("X").terminate().unwrap();
        let mut cursor = owner.open_cursor(listed).unwrap();
        let row = cursor.fetch().unwrap().unwrap();
        drop(cursor);
        assert_eq!((row[0].to_string(), row[1].to_string()), ("0".to_owned(), "0".to_owned()), "none listed nor counted");
    }
    owner.rollback().unwrap();
}

/// Verifies: [DESIGN] RULES.md D56, D43: a server's own files are shown to
/// the database's owner in INFORMATION_SCHEMA.FILES, of KIND SERVER and
/// no CLASS, not being the database's: its
/// TLS certificate chain and key, read once as it took TLS and closed;
/// its Unix socket or, on Windows, named pipe, open while it listens. Another user sees none.
#[test]
fn a_server_shows_its_files_to_the_owner() {
    let pki = pki("shown-files");
    let db = Database::new();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    secured(&db, &pki, "ON");
    let mut expected = vec![format!("{},NO,1", pki.chain.display()), format!("{},NO,1", pki.key.display())];
    if cfg!(any(unix, windows)) {
        let socket = pki.scratch.socket("s");
        db.set_setting(SOCKET, &Value::Char(socket.display().to_string())).unwrap();
        expected.push(format!("{},YES,0", socket.display()));
    }
    let server = start(&db, &[]);
    let files = "SELECT PATH, IS_OPEN, READS FROM INFORMATION_SCHEMA.FILES WHERE KIND = 'SERVER' AND CLASS IS NULL ORDER BY PATH";
    assert_eq!(rows(&server.connect("OWNER").query(files).unwrap()), expected);
    assert_eq!(rows(&server.connect("X").query(files).unwrap()), Vec::<String>::new(), "not the owner");
}
