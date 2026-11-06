//! The program as a node of a replica group (RULES.md D57): three
//! `turboreld`s on one machine, each its database's replica, linked over
//! TCP with mutual TLS, each told the group is new by `--owner`; the
//! first serves, the others hold what it commits; `--owner` makes the
//! owner once a node serves; stopped, a
//! backup takes over; run again, the first follows; its diagnostics tell
//! its replica's life; a command on a replicated database's directory is
//! refused.

use std::net::TcpListener;
use std::time::{Duration, Instant};

use turborel_client::connection::{self, Trust};
use turborel_test_support::Scratch;
use turborel_test_support::client::{done, rows};
use turborel_test_support::tls::pki;
use turboreld::{Command, UserCommand};

use crate::program::{Running, running};

/// How long a node is given to serve, or a backup to hold what the primary
/// does.
const PATIENCE: Duration = Duration::from_secs(30);

/// Whether `done` comes true within `PATIENCE`.
fn eventually(done: impl FnMut() -> bool) -> bool {
    let mut done = done;
    let began = Instant::now();
    while began.elapsed() < PATIENCE {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

/// The command line of member `me` of the group at `ports`, its database
/// under `scratch`, secured by `pki`, its diagnostics in its own log.
fn line(scratch: &Scratch, (me, ports): (usize, &[u16]), pki: &turborel_test_support::tls::Pki) -> Vec<String> {
    let members: Vec<String> = ports.iter().enumerate().map(|(n, p)| format!("{}=localhost:{p}", n + 1)).collect();
    let set = |name: &str, value: String| vec!["--set".to_owned(), format!("{name}={value}")];
    let mut line = vec!["--dir".to_owned(), scratch.path(&format!("node{me}")).display().to_string()];
    line.extend(set("SERVER.PORT", "0".into()));
    line.extend(set("SERVER.TRUST_LOCAL", "ON".into()));
    line.extend(set("SERVER.WORKERS", "2".into()));
    line.extend(set("SERVER.TLS_CERTIFICATE", pki.chain.display().to_string()));
    line.extend(set("SERVER.TLS_KEY", pki.key.display().to_string()));
    line.extend(set("REPLICATION.REPLICA", me.to_string()));
    line.extend(set("REPLICATION.MEMBERS", members.join(",")));
    line.extend(set("REPLICATION.CA", pki.authority.display().to_string()));
    // Ticks slow enough that a backup under a test runner's load, three
    // programs on one machine, does not give its primary up for want of a
    // heartbeat: a view change would move the service, and the owner,
    // off the first node.
    line.extend(set("REPLICATION.TICK_MS", "50".into()));
    line.extend(["--log".to_owned(), scratch.path(&format!("node{me}.log")).display().to_string()]);
    line
}

/// The configuration of `line`, a server's.
fn served(line: &[String]) -> turboreld::Config {
    match turboreld::parse(line).unwrap() {
        Command::Serve(config) => config,
        Command::Help | Command::Version | Command::User(..) | Command::Reseal(..) | Command::Rekey(..) => panic!("not a server"),
    }
}

/// A connection to the node at `running`, as `user`, trusted.
fn connect(running: &Running, user: &str) -> connection::Connection<std::net::TcpStream> {
    let at: std::net::SocketAddr = running.0[0].parse().unwrap();
    connection::connect(at, user, &mut Trust).unwrap()
}

/// Runs `sql` at `running` as X until it is done: whether it was within
/// `PATIENCE`; a node not serving refuses it meanwhile (SQLCODE -917).
fn done_at(running: &Running, sql: &str) -> bool {
    eventually(|| {
        let mut c = connect(running, "X");
        let outcome = c.query(sql);
        drop(c.terminate());
        outcome.is_ok_and(|o| done(&o) == 0)
    })
}

/// What every node's diagnostics log told of its replica, for a test that
/// fails to say why a view changed.
fn told(scratch: &Scratch) -> String {
    let mut lines = Vec::new();
    for n in 1..=3 {
        let log = std::fs::read_to_string(scratch.path(&format!("node{n}.log"))).unwrap_or_default();
        lines.extend(log.lines().filter(|l| l.contains(r#""event":"replica""#)).map(|l| format!("node{n}: {l}")));
    }
    lines.join("\n")
}

/// The rows of `sql` at `running`, as `user`.
fn rows_at(running: &Running, user: &str, sql: &str) -> Vec<String> {
    let mut c = connect(running, user);
    let found = c.query(sql).map(|o| rows(&o)).unwrap_or_default();
    drop(c.terminate());
    found
}

/// Verifies: [EXTENSION] RULES.md D57: three programs, each a member,
/// each given `--owner` as the group is new, link over mutual TLS; the
/// first serves, and `--owner` makes the owner once it does, once only;
/// the rows it commits are read at the backups, which
/// refuse writes; the owner sees INFORMATION_SCHEMA.REPLICAS; stopped, a
/// backup serves the next view; run again, the first holds every row;
/// the diagnostics log tells the replica's life; `user list` on a
/// replicated database's directory is refused.
#[test]
fn programs_replicate_a_database() {
    let scratch = Scratch::new("turboreld-replication");
    let pki = pki("turboreld-replication-pki");
    let held: Vec<TcpListener> = (0..3).map(|_| TcpListener::bind("127.0.0.1:0").unwrap()).collect();
    let ports: Vec<u16> = held.iter().map(|l| l.local_addr().unwrap().port()).collect();
    drop(held);
    let lines: Vec<Vec<String>> = (1..=3).map(|me| line(&scratch, (me, &ports), &pki)).collect();
    // A new group: each node is told so by `--owner`.
    let with_owner = |l: &Vec<String>| {
        let mut config = served(l);
        config.owner = Some(turboreld::Owner { name: "OWNER".into(), password: Some("secret".into()) });
        running(config)
    };
    let mut nodes: Vec<Running> = lines.iter().map(with_owner).collect();
    assert!(done_at(&nodes[0], "CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL UNIQUE)"), "the first serves");
    assert!(done_at(&nodes[0], "INSERT INTO T VALUES (1), (2)"));
    assert!(done_at(&nodes[0], "COMMIT WORK"));
    for backup in &nodes[1..] {
        assert!(eventually(|| rows_at(backup, "X", "SELECT K FROM T ORDER BY K") == ["1", "2"]));
        let mut c = connect(backup, "X");
        let refused = c.query("INSERT INTO T VALUES (9)").err().map(|e| e.sqlcode);
        assert_eq!(refused, Some(-917), "a backup refuses a write; the replicas told:\n{}", told(&scratch));
        drop(c.terminate());
    }
    let shown = "SELECT MEMBER, IS_SELF, ROLE FROM INFORMATION_SCHEMA.REPLICAS ORDER BY MEMBER";
    let mut seen = Vec::new();
    let agreed = eventually(|| {
        seen = rows_at(&nodes[0], "OWNER", shown);
        seen == ["1,YES,PRIMARY", "2,NO,BACKUP", "3,NO,BACKUP"]
    });
    assert!(agreed, "the owner made as it served; seen {seen:?}; the replicas told:\n{}", told(&scratch));

    let (_, stop, thread) = nodes.remove(0);
    stop.send(()).unwrap();
    thread.join().unwrap().unwrap();
    let log = std::fs::read_to_string(scratch.path("node1.log")).unwrap();
    assert!(log.contains(r#""event":"replica","change":"serving","view":0"#), "{log}");
    assert!(log.contains(r#""change":"connected","member":2"#), "{log}");
    let serving = eventually(|| nodes.iter().any(|n| done_at_once(n, "INSERT INTO T VALUES (3)")));
    assert!(serving, "a backup serves the next view");
    let primary = nodes
        .iter()
        .find(|n| {
            rows_at(n, "OWNER", "SELECT MEMBER FROM INFORMATION_SCHEMA.REPLICAS WHERE IS_SELF = 'YES' AND ROLE = 'PRIMARY'").len() == 1
        })
        .unwrap();
    assert!(done_at(primary, "COMMIT WORK"));
    let again = running(served(&lines[0]));
    assert!(eventually(|| rows_at(&again, "X", "SELECT K FROM T ORDER BY K") == ["1", "2", "3"]), "the first follows again");
    assert_eq!(rows_at(&again, "OWNER", "SELECT ROLE FROM INFORMATION_SCHEMA.REPLICAS WHERE IS_SELF = 'YES'"), ["BACKUP"]);
    for (_, stop, thread) in nodes.into_iter().chain([again]) {
        stop.send(()).unwrap();
        thread.join().unwrap().unwrap();
    }

    let mut offline = lines[0].clone();
    offline.push("user".into());
    offline.push("list".into());
    let Command::User(config, UserCommand::List) = turboreld::parse(&offline).unwrap() else { panic!("user list") };
    let refused = turboreld::run_user(&config, &UserCommand::List, (&mut std::io::empty(), &mut Vec::new())).unwrap_err();
    assert!(refused.contains("replicated"), "{refused}");
}

/// Whether `sql` is done at `running` now, as X, in a transaction of its
/// own connection kept open: a node serving takes it.
fn done_at_once(running: &Running, sql: &str) -> bool {
    let mut c = connect(running, "X");
    let outcome = c.query(sql);
    let taken = outcome.is_ok_and(|o| done(&o) == 0);
    if taken {
        drop(c.query("COMMIT WORK"));
    }
    drop(c.terminate());
    taken
}
