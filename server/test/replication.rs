//! A replica group of nodes in one process (RULES.md D57): each a
//! database kept in files, prepared as its settings say, opened to follow
//! and started on the server's runtime, linked to the others over TCP
//! with mutual TLS. The first member serves view 0; what it commits the
//! backups hold; INFORMATION_SCHEMA.REPLICAS shows the group; its node
//! stopped, another serves; opened again from its files, it rejoins.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use turborel::db::{Database, OpenOptions};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::Code;
use turborel::value::{Decimal, Value};
use turborel_replication::{ReplicaId, Transport};
use turborel_server::replication::{self, Replication};
use turborel_server::settings::{self, MEMBERS, REPLICA, REPLICATION_CA, REPLICATION_TLS, TICK_MS};
use turborel_test_support::database::rows;
use turborel_test_support::replication::contract::check_transport;

use crate::common::{Kept, Pki, Scratch, pki, secured};

/// How long a node is given to serve, or a backup to hold what the primary
/// does.
const PATIENCE: Duration = Duration::from_secs(30);

/// Whether `done` comes true within `PATIENCE`.
fn eventually(mut done: impl FnMut() -> bool) -> bool {
    let began = Instant::now();
    while began.elapsed() < PATIENCE {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

/// Ports the system has free now, one for each member.
fn ports(count: usize) -> Vec<u16> {
    let held: Vec<TcpListener> = (0..count).map(|_| TcpListener::bind("127.0.0.1:0").unwrap()).collect();
    held.iter().map(|l| l.local_addr().unwrap().port()).collect()
}

/// A node of a group, started.
struct Node {
    db: Database,
    replica: Replication,
    kept: Kept,
}

/// Member `me` of the group whose members are at `ports`, its database in
/// `scratch` under its own name, secured by `pki` (or in the clear if
/// none), started on `runtime`; told the group is new, as `--owner`
/// tells a node, if `new_group`.
fn start(scratch: &Scratch, (me, ports): (u32, &[u16]), pki: Option<&Pki>, runtime: &tokio::runtime::Runtime, new_group: bool) -> Node {
    let db = Database::new();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone())).unwrap();
    settings::declare_replication(&db).unwrap();
    let text = |s: &str| Value::Char(s.to_owned());
    let number = |n: u32| Value::Exact(Decimal::new(i128::from(n), 0));
    match pki {
        Some(pki) => {
            secured(&db, pki, "OFF");
            db.set_setting(REPLICATION_CA, &text(&pki.authority.display().to_string())).unwrap();
        }
        None => db.set_setting(REPLICATION_TLS, &text("OFF")).unwrap(),
    }
    let members: Vec<String> = ports.iter().enumerate().map(|(n, p)| format!("{}=localhost:{p}", n + 1)).collect();
    db.set_setting(REPLICA, &number(me)).unwrap();
    db.set_setting(MEMBERS, &text(&members.join(","))).unwrap();
    // Ticks slow enough that a backup under the test suite's load does not
    // give its primary up for want of a heartbeat.
    db.set_setting(TICK_MS, &number(50)).unwrap();
    let dir = scratch.path(&format!("node{me}"));
    let prepared = replication::prepare(&db, &dir, new_group).unwrap().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(OsFileSystem);
    let holds = db.open_following(&fs, &dir, &OpenOptions::default()).unwrap();
    let node = prepared.start(&db, runtime.handle(), holds).unwrap();
    Node { db, replica: node, kept }
}

impl Node {
    /// Whether the node serves as primary now.
    fn serves(&self) -> bool {
        self.replica.driver().serving().is_some()
    }

    /// The rows of `sql`, as the user Y.
    fn rows(&self, sql: &str) -> Result<Vec<String>, Code> {
        rows(&self.db.session("Y"), sql)
    }

    /// Stops the node and closes its database.
    fn stop(self) {
        self.replica.stop(&self.db).unwrap();
        self.db.close().unwrap();
    }
}

/// The node of `nodes` serving as primary, once one does and its writes
/// commit: a schema of `name` and a number made there.
fn primary<'a>(nodes: &'a [Node], name: &str) -> &'a Node {
    let (mut found, mut tried) = (None, 0_u32);
    assert!(
        eventually(|| {
            tried += 1;
            found = nodes.iter().find(|n| n.serves());
            found.is_some_and(|n| n.db.schema(&format!("CREATE SCHEMA AUTHORIZATION {name}{tried}")).is_ok())
        }),
        "no node serves and commits: {:?}",
        nodes.iter().map(|n| n.kept.all()).collect::<Vec<_>>()
    );
    found.unwrap()
}

/// Does `work` on the node of `nodes` serving as primary, again on the
/// one serving if a view change deposed it meanwhile (SQLCODE -917),
/// until it is done: the node it was done on.
fn on_primary(nodes: &[Node], work: impl Fn(&Database) -> Result<(), Code>) -> &Node {
    let mut done = None;
    assert!(
        eventually(|| {
            let Some(node) = nodes.iter().find(|n| n.serves()) else { return false };
            match work(&node.db) {
                Ok(()) => {
                    done = Some(node);
                    true
                }
                Err(Code::TransactionState) => false,
                Err(e) => panic!("{e:?}"),
            }
        }),
        "no node serves and commits: {:?}",
        nodes.iter().map(|n| n.kept.all()).collect::<Vec<_>>()
    );
    done.unwrap()
}

/// Verifies: [EXTENSION] RULES.md D57: three nodes over TCP with mutual
/// TLS: one serves, its view told `serving` and `view_changed`, its
/// links to the others up, told `connected`; the rows it commits reach
/// the backups, which serve reads and refuse writes (SQLCODE -917);
/// INFORMATION_SCHEMA.REPLICAS shows the owner each member, this node,
/// the primary, the view and the links; the transport keeps its
/// contract. The primary stopped, a backup serves the next view and
/// commits; the one stopped, opened again from its files, follows and
/// holds every row.
#[test]
fn nodes_replicate_over_tcp_with_mutual_tls() {
    let scratch = Scratch::new("replication-nodes");
    let pki = pki("replication-nodes-pki");
    let runtime = turborel_server::runtime::runtime(Some(2)).unwrap();
    let ports = ports(3);
    let mut nodes: Vec<Node> = (1..=3).map(|me| start(&scratch, (me, &ports), Some(&pki), &runtime, true)).collect();
    check_transport(&**primary(&nodes, "X").replica.transport(), ReplicaId(9));
    let first = on_primary(&nodes, |db| {
        if db.owner().map_err(|e| e.code)?.is_none() {
            db.create_user("OWNER", b"x").map_err(|e| e.code)?;
            db.set_owner("OWNER").map_err(|e| e.code)?;
        }
        db.schema("CREATE SCHEMA AUTHORIZATION Y CREATE TABLE T (K INTEGER NOT NULL UNIQUE)").map_err(|e| e.code)?;
        let s = db.session("Y");
        s.run("INSERT INTO T VALUES (1), (2)").map_err(|e| e.code)?;
        s.commit().map_err(|e| e.code)
    });
    let me = usize::try_from(first.replica.transport().me().0).unwrap() - 1;
    for backup in nodes.iter().filter(|n| !n.serves()) {
        assert!(eventually(|| backup.rows("SELECT K FROM T ORDER BY K") == Ok(vec!["1".into(), "2".into()])), "{:?}", backup.kept.all());
        let writer = backup.db.session("Y");
        assert_eq!(writer.run("INSERT INTO T VALUES (9)").err().map(|e| e.code), Some(Code::TransactionState), "a backup takes no change");
    }
    let shown = "SELECT MEMBER, IS_SELF, ROLE, IS_CONNECTED FROM INFORMATION_SCHEMA.REPLICAS ORDER BY MEMBER";
    let expected: Vec<String> = (1..=3)
        .map(|m| format!("{m},{},{},YES", if m == me + 1 { "YES" } else { "NO" }, if m == me + 1 { "PRIMARY" } else { "BACKUP" }))
        .collect();
    let owner = first.db.session("OWNER");
    assert!(eventually(|| rows(&owner, shown) == Ok(expected.clone())), "{:?}", rows(&owner, shown));
    assert_eq!(rows(&first.db.session("Y"), shown), Ok(Vec::new()), "the owner's alone");
    let events = first.kept.all();
    assert!(events.iter().any(|e| e.contains("Serving { view:")) && events.iter().any(|e| e.contains("ViewChanged")), "{events:?}");
    assert!(events.iter().any(|e| e.contains("Connected { member:")), "{events:?}");
    drop(owner);

    nodes.remove(me).stop();
    let promoted = eventually(|| nodes.iter().any(Node::serves));
    assert!(promoted, "a backup serves the next view: {:?}", nodes[0].kept.all());
    let serving = nodes.iter().find(|n| n.serves()).unwrap();
    assert!(eventually(|| {
        let s = serving.db.session("Y");
        s.run("INSERT INTO T VALUES (3)").is_ok() && s.commit().is_ok()
    }));
    let again = start(&scratch, (u32::try_from(me).unwrap() + 1, &ports), Some(&pki), &runtime, false);
    let heard = "SELECT MEMBER, ROLE, VIEW_NUMBER, IS_CONNECTED, LAST_HEARD_MS FROM INFORMATION_SCHEMA.REPLICAS ORDER BY MEMBER";
    assert!(
        eventually(|| again.rows("SELECT K FROM T ORDER BY K") == Ok(vec!["1".into(), "2".into(), "3".into()])),
        "{:?} {:?} {:?}",
        rows(&again.db.session("OWNER"), heard),
        rows(&serving.db.session("OWNER"), heard),
        again.kept.all()
    );
    assert!(!again.serves(), "a backup now");
    again.stop();
    for node in nodes {
        node.stop();
    }
}

/// Verifies: [EXTENSION] RULES.md D57: a member whose certificate another
/// authority signed is refused by the other's listener, told
/// `connection_refused` for TLS, and its own listener refuses the other,
/// whose link is told `disconnected` for TLS: no link comes up between
/// them. The group in the clear (REPLICATION.TLS OFF) links all the same,
/// one of its nodes serves, and REPLICAS shows both links up.
#[test]
fn a_member_of_another_authority_is_refused() {
    let scratch = Scratch::new("replication-authority");
    let (pki, other) = (pki("replication-authority-pki"), pki("replication-authority-other"));
    let runtime = turborel_server::runtime::runtime(Some(2)).unwrap();
    let secured_ports = ports(2);
    let first = start(&scratch, (1, &secured_ports), Some(&pki), &runtime, true);
    let stranger = start(&scratch, (2, &secured_ports), Some(&other), &runtime, true);
    let told = |node: &Node, change: &str| node.kept.all().iter().any(|e| e.contains(change) && e.contains("TLS"));
    assert!(eventually(|| told(&first, "Disconnected") && told(&first, "ConnectionRefused")), "{:?}", first.kept.all());
    assert!(eventually(|| told(&stranger, "Disconnected") && told(&stranger, "ConnectionRefused")), "{:?}", stranger.kept.all());
    assert!(!first.kept.all().iter().any(|e| e.contains("Connected { member")), "no link came up: {:?}", first.kept.all());
    stranger.stop();
    first.stop();

    let plain = Scratch::new("replication-plain");
    let plain_ports = ports(2);
    let nodes: Vec<Node> = (1..=2).map(|me| start(&plain, (me, &plain_ports), None, &runtime, true)).collect();
    let serving = primary(&nodes, "X");
    serving.db.create_user("OWNER", b"x").unwrap();
    serving.db.set_owner("OWNER").unwrap();
    let shown = "SELECT MEMBER, IS_CONNECTED FROM INFORMATION_SCHEMA.REPLICAS ORDER BY MEMBER";
    assert!(eventually(|| rows(&serving.db.session("OWNER"), shown) == Ok(vec!["1,YES".into(), "2,YES".into()])));
    for node in nodes {
        node.stop();
    }
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §7]: a group of three nodes in
/// the clear reconfigured by the owner on the primary, ALTER DATABASE
/// REPLICATION MEMBERS, to four: the statement returns once the group
/// commits it, told `epoch_entered`; a fourth node started with settings
/// naming the four joins by recovery, holds every row, and REPLICAS on
/// every node shows the four members in epoch 1; reconfigured to three
/// again without the second node, that node is told `removed`, its
/// database serving nothing (SQLCODE -917, a read too), REPLICAS shows
/// epoch 2 and three members, and a row committed after reaches the
/// fourth node; a member list malformed is refused (-103), and a backup
/// refuses the statement (-917).
#[test]
fn nodes_are_added_and_removed_by_reconfiguration() {
    let scratch = Scratch::new("replication-reconfigure");
    let runtime = turborel_server::runtime::runtime(Some(2)).unwrap();
    let ports = ports(4);
    let members =
        |which: &[usize]| -> String { which.iter().map(|&i| format!("{}=localhost:{}", i + 1, ports[i])).collect::<Vec<_>>().join(",") };
    let mut nodes: Vec<Node> = (1..=3).map(|me| start(&scratch, (me, &ports[..3]), None, &runtime, true)).collect();
    let first = primary(&nodes, "X");
    first.db.create_user("OWNER", b"x").unwrap();
    first.db.set_owner("OWNER").unwrap();
    first.db.schema("CREATE SCHEMA AUTHORIZATION Y CREATE TABLE T (K INTEGER NOT NULL UNIQUE)").unwrap();
    let s = first.db.session("Y");
    s.run("INSERT INTO T VALUES (1), (2)").unwrap();
    s.commit().unwrap();
    drop(s);
    let owner = first.db.session("OWNER");
    assert_eq!(owner.run("ALTER DATABASE REPLICATION MEMBERS '1=localhost'").err().map(|e| e.code), Some(Code::SyntaxRule), "malformed");
    let backup = nodes.iter().position(|n| !n.serves()).unwrap();
    let refused = nodes[backup].db.session("OWNER").run(&format!("ALTER DATABASE REPLICATION MEMBERS '{}'", members(&[0, 1, 2, 3])));
    assert_eq!(refused.err().map(|e| e.code), Some(Code::TransactionState), "a backup takes no change");
    owner.run(&format!("ALTER DATABASE REPLICATION MEMBERS '{}'", members(&[0, 1, 2, 3]))).unwrap();
    assert!(eventually(|| first.kept.all().iter().any(|e| e.contains("EpochEntered { epoch: 1"))), "{:?}", first.kept.all());
    drop(owner);
    let fourth = start(&scratch, (4, &ports), None, &runtime, false);
    assert!(eventually(|| fourth.rows("SELECT K FROM T ORDER BY K") == Ok(vec!["1".into(), "2".into()])), "{:?}", fourth.kept.all());
    nodes.push(fourth);
    let shown = "SELECT MEMBER, EPOCH FROM INFORMATION_SCHEMA.REPLICAS ORDER BY MEMBER";
    let four: Vec<String> = (1..=4).map(|m| format!("{m},1")).collect();
    for node in &nodes {
        let why = || node.db.session("OWNER").open_cursor(shown).err().map(|e| e.to_string());
        assert!(
            eventually(|| rows(&node.db.session("OWNER"), shown) == Ok(four.clone())),
            "{:?} {:?}",
            rows(&node.db.session("OWNER"), shown),
            why()
        );
    }

    let kept: Vec<usize> = (0..4).filter(|&i| i != 1).collect();
    let serving = nodes.iter().find(|n| n.serves()).expect("a primary of epoch 1");
    eprintln!(
        "SERVING me={:?} members={:?} events={:?}",
        serving.replica.transport().me(),
        serving.replica.transport().members(),
        serving.kept.all()
    );
    serving.db.session("OWNER").run(&format!("ALTER DATABASE REPLICATION MEMBERS '{}'", members(&kept))).unwrap();
    let second = nodes.iter().find(|n| n.replica.transport().me() == ReplicaId(2)).unwrap();
    assert!(eventually(|| second.kept.all().iter().any(|e| e.contains("Removed"))), "{:?}", second.kept.all());
    assert_eq!(second.rows("SELECT K FROM T").err(), Some(Code::TransactionState), "retired: not even a read");
    let three: Vec<String> = [1, 3, 4].iter().map(|m| format!("{m},2")).collect();
    for node in nodes.iter().filter(|n| n.replica.transport().me() != ReplicaId(2)) {
        assert!(eventually(|| rows(&node.db.session("OWNER"), shown) == Ok(three.clone())), "{:?}", rows(&node.db.session("OWNER"), shown));
    }
    on_primary(&nodes, |db| {
        let s = db.session("Y");
        s.run("INSERT INTO T VALUES (3)").map_err(|e| e.code)?;
        s.commit().map_err(|e| e.code)
    });
    let fourth = nodes.iter().find(|n| n.replica.transport().me() == ReplicaId(4)).unwrap();
    assert!(
        eventually(|| fourth.rows("SELECT K FROM T ORDER BY K") == Ok(vec!["1".into(), "2".into(), "3".into()])),
        "{:?}",
        fourth.kept.all()
    );
    for node in nodes {
        node.stop();
    }
}
