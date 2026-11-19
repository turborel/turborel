//! Replication's host kinds served by a real process (RULES.md D270 to
//! D272): the example plug-in's transport LOOP and journal KEPT
//! (`plugin/examples/plugin_example/replication.rs`), declared by
//! PLUGINS.HOST and reached as a node reaches them (`plugin::plugged`).
//! What the simulator's stand-ins cannot show is shown here: a program
//! that ends having written part of a message, one that ends having
//! written part of a record, and a state transfer, SHIPPED, that ends
//! mid-chunk. A consensus has no example program, as one would need a
//! whole protocol: the simulator's stand-ins serve it through the real
//! bridges (`plugged.rs`).

use std::path::{Path, PathBuf};

use turborel::db::{Database, ExportedFile, StateSource};
use turborel::error::{Code, Result, SqlError};
use turborel::files::FileOwner;
use turborel::value::Value;
use turborel_replication::plugin::{Plugged, PluginTransfer, plugged};
use turborel_replication::{Lsn, Member, ReplicaId, StateTransfer, Transport, staging_dir};
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
/// LOOP, KEPT and SHIPPED.
fn installed(scratch: &Scratch) -> PathBuf {
    let dir = scratch.path("repl");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = format!("plugin_example{}", std::env::consts::EXE_SUFFIX);
    let manifest = format!(
        "NAME REPL; VERSION '1.0.0'; PROTOCOL 1; EXECUTABLE '{exe}';\nHOST REPLICA TRANSPORT LOOP;\nHOST JOURNAL KEPT;\nHOST STATE TRANSFER SHIPPED;\n"
    );
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
    assert_eq!(recovered.records.len(), 0);
    journal.persist(Lsn(1), b"kept").unwrap();
    assert_eq!(journal.sync().unwrap(), [Lsn(1)]);
    journal.persist(Lsn(2), b"unsynced").unwrap();
    assert!(journal.persist(Lsn(3), b"CRASH").is_err());
    assert!(journal.sync().is_err(), "nothing is acknowledged once the process died");
    assert!(db.host_stopped().unwrap().is_some());
    drop((journal, db));
    let (_again, reopened) = node(&scratch, &dir);
    let (_journal, held) = (reopened.journal.unwrap())().unwrap();
    assert_eq!(held.records, [b"kept".to_vec()]);
}

/// An export held in memory: an image and files of the bytes given.
struct Held {
    files: Vec<ExportedFile>,
    bytes: Vec<Vec<u8>>,
}

impl Held {
    /// The export of `bytes`, a file each.
    fn of(bytes: Vec<Vec<u8>>) -> Held {
        let files = bytes.iter().enumerate().map(|(i, b)| ExportedFile {
            path: PathBuf::from(format!("held/{i}")),
            within: PathBuf::from(format!("f{i}")),
            length: u64::try_from(b.len()).unwrap(),
            owner: FileOwner::Database,
        });
        Held { files: files.collect(), bytes }
    }
}

impl StateSource for Held {
    fn image(&self) -> &[u8] {
        b"image"
    }
    fn files(&self) -> &[ExportedFile] {
        &self.files
    }
    fn read(&mut self, file: usize, at: u64, into: &mut [u8]) -> Result<usize> {
        let from = self.bytes.get(file).map_or(&[][..], Vec::as_slice);
        let rest = from.get(usize::try_from(at).unwrap_or(usize::MAX)..).unwrap_or_default();
        let n = rest.len().min(into.len());
        for (to, b) in into.iter_mut().zip(rest) {
            *to = *b;
        }
        Ok(n)
    }
}

/// `transfer` fetching `source` from itself, a request answered at a time,
/// and the image and files it staged, read back.
fn fetch_through(transfer: &PluginTransfer, source: &mut Held) -> Result<Vec<Vec<u8>>> {
    let unanswered = || SqlError::new(Code::Api, "the transfer gave no request, or SHIPPED no answer");
    let mut fetching = transfer.begin(ReplicaId(2))?;
    loop {
        let request = fetching.request().ok_or_else(unanswered)?;
        let answer = transfer.serve(source, (ReplicaId(1), 7), &request)?.ok_or_else(unanswered)?;
        if fetching.take(&answer)? {
            break;
        }
    }
    let mut staged = fetching.fetched()?;
    let lengths: Vec<u64> = staged.files().iter().map(|f| f.length).collect();
    let mut files = vec![staged.image().to_vec()];
    for (i, length) in lengths.into_iter().enumerate() {
        let mut bytes = vec![0; usize::try_from(length).unwrap_or(0)];
        let read = staged.read(i, 0, &mut bytes)?;
        bytes.truncate(read);
        files.push(bytes);
    }
    Ok(files)
}

/// Verifies: [EXTENSION] RULES.md D270, D272: a state transfer whose
/// process ends mid-chunk stops the node, its staging left part written;
/// the node started again clears it as the next fetch begins, and that
/// fetch stages every file whole, as the export holds them.
#[test]
fn a_transfer_killed_mid_chunk_stops_the_node_and_the_next_fetch_starts_clean() {
    let scratch = Scratch::new("plugin-transfer-killed");
    let dir = installed(&scratch);
    let big: Vec<u8> = (0..200_000_u32).map(|i| u8::try_from(i % 251).unwrap()).collect();
    let mut source = Held::of(vec![big, b"small".to_vec(), Vec::new()]);
    std::fs::write(dir.join("crash.chunk"), b"").unwrap();
    let (db, plugs) = node(&scratch, &dir);
    let transfer = plugs.transfer.unwrap();
    let killed = fetch_through(&transfer, &mut source).unwrap_err();
    assert!(killed.msg.contains("D270"), "{killed}");
    assert!(db.host_stopped().unwrap().is_some());
    assert!(transfer.failed().is_some());
    let staging = staging_dir(&scratch.path("node"));
    assert!(std::fs::metadata(staging.join("0")).unwrap().len() > 0, "the first chunk was staged");
    assert!(transfer.begin(ReplicaId(2)).is_err(), "nothing more once the process died");
    drop((transfer, db));
    std::fs::remove_file(dir.join("crash.chunk")).unwrap();
    let (_again, restarted) = node(&scratch, &dir);
    let staged = fetch_through(&restarted.transfer.unwrap(), &mut source).unwrap();
    let mut expected = vec![b"image".to_vec()];
    expected.extend(source.bytes.iter().cloned());
    assert_eq!(staged, expected);
    assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 3, "only the new fetch's files are staged");
}
