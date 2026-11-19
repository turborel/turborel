//! Replication's host kinds served by a real process (RULES.md D270 to
//! D272): the example plug-in's transport LOOP and journal KEPT
//! (`plugin/examples/plugin_example/replication.rs`), declared by
//! PLUGINS.HOST and reached as a node reaches them (`plugin::plugged`).
//! What the simulator's stand-ins cannot show is shown here: a program
//! that ends having written part of a message, and one that ends having
//! written part of a record.

use std::path::{Path, PathBuf};

use turborel::db::Database;
use turborel::value::Value;
use turborel_replication::plugin::{Plugged, plugged};
use turborel_replication::{Lsn, Member, ReplicaId, Transport};
use turborel_test_support::Scratch;

/// The example's program, as the workspace's build made it.
fn program() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().and_then(Path::parent).unwrap();
    let built = debug.join("examples").join(format!("plugin_example{}", std::env::consts::EXE_SUFFIX));
    assert!(built.exists(), "{} is not built: cargo build -p turborel-plugin --example plugin_example", built.display());
    built
}

/// The example installed in `scratch` as the host plug-in REPL serving
/// LOOP and KEPT.
fn installed(scratch: &Scratch) -> PathBuf {
    let dir = scratch.path("repl");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = format!("plugin_example{}", std::env::consts::EXE_SUFFIX);
    let manifest =
        format!("NAME REPL; VERSION '1.0.0'; PROTOCOL 1; EXECUTABLE '{exe}';\nHOST REPLICA TRANSPORT LOOP;\nHOST JOURNAL KEPT;\n");
    std::fs::write(dir.join("turborel-plugin.manifest"), manifest).unwrap();
    if std::fs::hard_link(program(), dir.join(&exe)).is_err() {
        std::fs::copy(program(), dir.join(&exe)).unwrap();
    }
    dir
}

/// A node's database declaring REPL in `dir`, and its parts so plugged.
fn node(scratch: &Scratch, dir: &Path) -> (Database, Plugged) {
    let db = Database::new();
    db.set_setting("PLUGINS.DIRECTORIES", &Value::Char(scratch.dir.display().to_string())).unwrap();
    db.set_setting("PLUGINS.HOST", &Value::Char(format!("REPL={}", dir.display()))).unwrap();
    let members = [Member { id: ReplicaId(1), address: "one".into() }, Member { id: ReplicaId(2), address: "two".into() }];
    let plugs = plugged(&db, &scratch.path("node"), (&members, ReplicaId(1))).unwrap();
    (db, plugs)
}

/// Verifies: [EXTENSION] RULES.md D270, D271: a transport whose process
/// ends mid-message gives no part of one: the call fails, the node stops,
/// and every later call fails.
#[test]
fn a_transport_killed_mid_message_stops_the_node() {
    let scratch = Scratch::new("plugin-transport-killed");
    let (db, plugs) = node(&scratch, &installed(&scratch));
    let transport = plugs.transport.unwrap();
    assert_eq!(transport.members(), [ReplicaId(1), ReplicaId(2)]);
    transport.send(ReplicaId(2), b"hello").unwrap();
    assert_eq!(transport.receive().unwrap(), [(ReplicaId(2), b"hello".to_vec())]);
    assert!(db.host_stopped().unwrap().is_none());
    let half = transport.send(ReplicaId(2), b"HALF").unwrap_err();
    assert!(half.msg.contains("D270"), "{half}");
    assert!(db.host_stopped().unwrap().is_some());
    assert!(transport.failed().is_some());
    assert!(transport.receive().is_err());
}

/// Verifies: [EXTENSION] RULES.md D270, D272: a journal's record is
/// acknowledged only once SYNC names it; a process ended mid-append
/// acknowledges nothing more, and the journal opened again holds only
/// what was named, its torn entry left out.
#[test]
fn a_journal_killed_mid_append_acknowledges_only_what_was_durable() {
    let scratch = Scratch::new("plugin-journal-killed");
    let dir = installed(&scratch);
    let (db, plugs) = node(&scratch, &dir);
    let (mut journal, recovered) = (plugs.journal.unwrap())().unwrap();
    assert!(recovered.records.is_empty());
    journal.persist(Lsn(1), b"kept").unwrap();
    assert_eq!(journal.sync().unwrap(), [Lsn(1)]);
    journal.persist(Lsn(2), b"unsynced").unwrap();
    assert!(journal.persist(Lsn(3), b"CRASH").is_err());
    assert!(journal.sync().is_err(), "nothing is acknowledged once the process died");
    assert!(db.host_stopped().unwrap().is_some());
    drop((journal, db));
    let (_again, plugs) = node(&scratch, &dir);
    let (_journal, recovered) = (plugs.journal.unwrap())().unwrap();
    assert_eq!(recovered.records, [b"kept".to_vec()]);
}
