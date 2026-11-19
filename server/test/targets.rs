//! Where a server is reached, as a client names it and shows it
//! (RULES.md D42, D43).

use turborel::db::Database;
use turborel_client::connection::{self, Connection, Trust};
use turborel_client::target::Target;

use crate::common::{rows, start};

/// Verifies: [DESIGN] RULES.md D42: a target over TCP gives a stream to
/// its server, over which a connection opens and runs statements; a
/// target and a connection show where they reach, never a secret, and
/// whether the connection is lost; a local socket's target shows its
/// path.
#[test]
fn a_target_streams_and_shows_where_it_reaches() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let server = start(&db, &[]);
    let target = Target::Tcp(server.address);
    assert_eq!(format!("{target:?}"), format!("Tcp({})", server.address));
    let mut opened = Connection::open(target.stream().unwrap(), "X", &mut Trust).unwrap();
    assert_eq!(rows(&opened.query("SELECT COUNT(*) FROM T").unwrap()), ["0"]);
    opened.terminate().unwrap();
    let reached = connection::connect(server.address, "X", &mut Trust).unwrap();
    let shown = format!("{reached:?}");
    assert!(shown.starts_with("Connection { target: ") && shown.contains("authorization: \"X\", lost: false"), "{shown}");
    reached.terminate().unwrap();
    #[cfg(any(unix, windows))]
    {
        let local = Target::local(std::path::Path::new("turborel.sock")).unwrap();
        assert_eq!(format!("{local:?}"), "Local(turborel.sock)");
        assert!(!local.is_tls() && !local.is_plain_tcp());
    }
}
