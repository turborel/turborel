//! Several servers (RULES.md D201, D202): a list tried in order, past an
//! address no server listens at; a server of the wrong role left; and
//! connection settings read from a URI.

use std::net::{SocketAddr, TcpListener};

use turborel::db::Database;
use turborel_client::role::{Role, role};
use turborel_client::settings::{Options, Settings};
use turborel_client::target::Target;

use crate::common::{Running, rows, start};

/// A server whose user X has the table T.
fn served() -> (Database, Running) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let server = start(&db, &[]);
    (db, server)
}

/// An address no server listens at: one the system gave, then gave up.
fn dead() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap()
}

/// The settings of the URI `uri`, read with no environment.
fn settings(uri: &str) -> Settings {
    Settings::resolve_in(&Options::from_uri(uri).unwrap(), |_| None).unwrap()
}

/// Verifies: [DESIGN] RULES.md D201: of a list of servers, the first that
/// can be reached is connected to, past an address no server listens at;
/// the connection remembers where it reached; none reached, the failure
/// (SQLSTATE 08001) names each address and why.
#[test]
fn a_dead_address_is_passed_for_the_next() {
    let (_db, server) = served();
    let (first, second) = (dead(), dead());
    let mut c = settings(&format!("turborel://X@{first},{}/?tls_mode=disable", server.address)).connect().unwrap();
    assert!(matches!(c.target(), Some(Target::Tcp(at)) if *at == server.address), "{:?}", c.target());
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["0"]);
    let none = settings(&format!("turborel://X@{first},{second}/?tls_mode=disable&connect_timeout=2")).connect().unwrap_err();
    assert_eq!(none.sqlstate, "08001");
    assert!(none.message.contains(&first.to_string()) && none.message.contains(&second.to_string()), "{}", none.message);
}

/// Verifies: [DESIGN] RULES.md D201, D57: a server alone takes changes,
/// so its role is read-write, to any session; a connection asking for a
/// read-write server keeps it, its session left in no transaction; one
/// asking for a read-only server leaves it, and of no other is refused;
/// past it, a list goes on to the next.
#[test]
fn a_server_of_the_wrong_role_is_left() {
    let (_db, server) = served();
    let mut c = server.connect("X");
    assert_eq!(role(&mut c).unwrap(), Role::ReadWrite);
    assert!(!c.in_transaction());
    let mut kept = settings(&format!("turborel://X@{}/?tls_mode=disable&target=read-write", server.address)).connect().unwrap();
    assert!(!kept.in_transaction());
    assert_eq!(rows(&kept.query("SELECT COUNT(*) FROM T").unwrap()), ["0"]);
    let left = settings(&format!("turborel://X@{}/?tls_mode=disable&target=read-only", server.address)).connect().unwrap_err();
    assert!(left.message.contains("the server is read-write, and read-only is asked"), "{}", left.message);
    assert_eq!(left.sqlstate, "08001");
    let (_other, another) = served();
    let uri = format!("turborel://X@{},{}/?tls_mode=disable&target=read-only", server.address, another.address);
    let both = settings(&uri).connect().unwrap_err();
    assert_eq!(both.sqlstate, "08001");
}

/// Verifies: [DESIGN] RULES.md D201: servers ordered at random are each
/// connected to as any is; every connection reaches one of them.
#[test]
fn servers_ordered_at_random_are_each_reached() {
    let (_a, one) = served();
    let (_b, two) = served();
    let s = settings(&format!("turborel://X@{},{}/?tls_mode=disable&load_balance=random", one.address, two.address));
    for _ in 0..8 {
        let c = s.connect().unwrap();
        assert!(matches!(c.target(), Some(Target::Tcp(at)) if *at == one.address || *at == two.address));
    }
}
