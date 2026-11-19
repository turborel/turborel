//! A replicated group's servers (RULES.md D201, D206, D57): a connection
//! asking for a read-only server lands on a replica following the group,
//! one asking for a read-write server on its primary, the role each
//! session told by its session status, or read from
//! INFORMATION_SCHEMA.REPLICAS without it, and a pool of read-only
//! settings handing out the replica's connections.

use std::net::TcpStream;
use turborel::db::Database;
use turborel::error::Code;
use turborel_client::pool::{Pool, PoolConfig};

use turborel_client::connection::{Connection, Trust};
use turborel_client::role::{Role, SERVER_READ_ONLY, role};
use turborel_client::settings::{Options, Settings};
use turborel_client::target::Target;
use turborel_client::told::Offer;

use turborel_test_support::database::rows as session_rows;

use crate::common::{Running, Scratch, rows, start};
use crate::replication::{self, Node, eventually, on_primary, ports};

/// The settings of the URI `uri`, read with no environment.
fn settings(uri: &str) -> Settings {
    Settings::resolve_in(&Options::from_uri(uri).unwrap(), |_| None).unwrap()
}

/// Makes OWNER the database's owner, if it has none, and gives Y the table
/// T of one row, committed, which OWNER may read and insert into.
fn populate(db: &Database) -> Result<(), Code> {
    if db.owner().map_err(|e| e.code)?.is_none() {
        db.create_user("OWNER", b"x").map_err(|e| e.code)?;
        db.set_owner("OWNER").map_err(|e| e.code)?;
    }
    db.schema("CREATE SCHEMA AUTHORIZATION Y CREATE TABLE T (K INTEGER)").map_err(|e| e.code)?;
    let s = db.session("Y");
    s.run("INSERT INTO T VALUES (1)").map_err(|e| e.code)?;
    s.run("GRANT SELECT, INSERT ON T TO OWNER").map_err(|e| e.code)?;
    s.commit().map_err(|e| e.code)
}

/// Whether a connection that reached `target` reached `server`.
fn reached(target: Option<&Target>, server: &Running) -> bool {
    matches!(target, Some(Target::Tcp(at)) if *at == server.address)
}

/// Asserts what the settings of `uri` reach, the primary at `primary` and
/// the replica at `replica` each listed first once.
fn targets_land(primary: &Running, replica: &Running) {
    let (p, r) = (primary.address, replica.address);
    let mut writer = settings(&format!("turborel://OWNER@{r},{p}/?tls_mode=disable&target=read-write")).connect().unwrap();
    assert!(reached(writer.target(), primary), "{:?}", writer.target());
    assert_eq!(role(&mut writer).unwrap(), Role::ReadWrite);
    let mut reader = settings(&format!("turborel://OWNER@{p},{r}/?tls_mode=disable&target=read-only")).connect().unwrap();
    assert!(reached(reader.target(), replica), "{:?}", reader.target());
    assert_eq!(role(&mut reader).unwrap(), Role::ReadOnly);
    assert_eq!(rows(&reader.query("SELECT K FROM Y.T").unwrap()), ["1"]);
    let refused = reader.query("INSERT INTO Y.T VALUES (2)").unwrap_err();
    assert_eq!(refused.sqlcode, -917, "{}", refused.message);
    let told = settings(&format!("turborel://Y@{p},{r}/?tls_mode=disable&target=read-only")).connect().unwrap();
    assert!(reached(told.target(), replica), "{:?}", told.target());
    assert_eq!(told.setting(SERVER_READ_ONLY), Some("YES"));
    let mut asked = Connection::open_offering(TcpStream::connect(r).unwrap(), ("Y", &mut Trust), Offer::none()).unwrap();
    assert_eq!(role(&mut asked).unwrap(), Role::Unknown, "with no status, the query, which Y cannot see");
}

/// Asserts a pool of read-only settings over both servers hands out the
/// replica's connections, again once they come back reset.
fn pool_reads_the_replica(primary: &Running, replica: &Running) {
    let uri = format!("turborel://OWNER@{},{}/?tls_mode=disable&target=read-only", primary.address, replica.address);
    let pool = Pool::new(settings(&uri), PoolConfig { max: 2, max_idle: 2, ..PoolConfig::default() });
    for _ in 0..2 {
        let (mut one, mut two) = (pool.get().unwrap(), pool.get().unwrap());
        for c in [&mut one, &mut two] {
            assert!(reached(c.target(), replica), "{:?}", c.target());
            assert_eq!(role(c).unwrap(), Role::ReadOnly);
            assert_eq!(rows(&c.query("SELECT K FROM Y.T").unwrap()), ["1"]);
        }
    }
    assert_eq!(pool.counts().unwrap(), (2, 2), "both given back, kept");
}

/// Verifies: [DESIGN] RULES.md D201, D206, D57: of a group of three over
/// TCP, a server of its primary and one of a backup following it. Settings
/// asking for a read-write server pass the replica listed first and
/// connect to the primary, whose role the session is told is read-write;
/// asking for a read-only server they pass the primary listed first and
/// connect to the replica, read-only, which serves the rows committed and
/// refuses a change (SQLCODE -917). A user not the owner is told the
/// replica's role by its session status too, and kept; without status,
/// it asks REPLICAS, whose row it cannot see, so the role is unknown.
/// A pool of read-only settings hands out the replica's connections, and
/// again as they come back reset.
#[test]
fn targets_find_the_primary_and_a_replica() {
    let scratch = Scratch::new("client-replicas");
    let runtime = turborel_server::runtime::runtime(Some(2)).unwrap();
    let ports = ports(3);
    let nodes: Vec<Node> = (1..=3).map(|me| replication::start(&scratch, (me, &ports), None, &runtime, true)).collect();
    let first = on_primary(&nodes, populate);
    let backup = nodes.iter().find(|n| !n.serves()).unwrap();
    assert!(eventually(|| backup.rows("SELECT K FROM T") == Ok(vec!["1".into()])));
    let shown = "SELECT ROLE FROM INFORMATION_SCHEMA.REPLICAS WHERE IS_SELF = 'YES'";
    assert!(eventually(|| session_rows(&backup.db.session("OWNER"), shown) == Ok(vec!["BACKUP".into()])));
    let (primary, replica) = (start(&first.db, &[]), start(&backup.db, &[]));
    targets_land(&primary, &replica);
    pool_reads_the_replica(&primary, &replica);
    primary.stop();
    replica.stop();
    for node in nodes {
        node.stop();
    }
}
