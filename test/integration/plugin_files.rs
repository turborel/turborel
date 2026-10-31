//! Plug-ins' shadowed directories (RULES.md D56): what a plug-in writes,
//! makes and removes in a directory it declares shadowed reaches its files
//! at the database's checkpoints, so after a crash the directory is as the
//! last checkpoint left it; a directory kept as written is written at
//! once.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use turborel::catalog::TableDesc;
use turborel::db::{Database, OpenOptions};
use turborel::disk::io::{FileSystem, OpenMode};
use turborel::engine::rows::{RowStore, TableOptions};
use turborel::error::{Code, Result, SqlError};
use turborel::files::plugins::Kept;
use turborel::log::Lsn;
use turborel::memory::Host;
use turborel::settings::Configurable;
use turborel::storage::{StorageEngine, TableStorage};

use crate::common::{Noted, Scratch, after_crash, whole};
use crate::faults::{FaultyFs, Op, Rule};

/// The name of the plug-in, and of its directory.
const NAME: &str = "shadowing";

/// The plug-in's file whose presence fails its checkpoints.
const REFUSE: &str = "refuse";

/// A plug-in engine whose directory is declared shadowed: at each
/// checkpoint it writes, to its file `state`, the position the checkpoint
/// holds the log through and the value of its setting SHADOWING.DEPTH it
/// was told (RULES.md D36), then fails if its file `refuse` is there. Its
/// rows are kept by a row store in memory.
struct Shadowing {
    rows: RowStore,
    host: Arc<Mutex<Option<Host>>>,
    noted: Noted,
}

/// The plug-in's setting.
const DEPTH: &str = "SHADOWING.DEPTH";

impl Shadowing {
    /// The engine, what it will be given, and its settings hook.
    fn new() -> (Box<Shadowing>, Arc<Mutex<Option<Host>>>, Noted) {
        let (host, noted) = (Arc::new(Mutex::new(None)), Noted::new(DEPTH));
        let engine = Shadowing { rows: RowStore::in_memory(TableOptions::default()), host: Arc::clone(&host), noted: noted.clone() };
        (Box::new(engine), host, noted)
    }
}

impl StorageEngine for Shadowing {
    fn name(&self) -> &'static str {
        NAME
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.rows.attach(host)?;
        host.files.declare(NAME, Kept::Shadowed)?;
        *self.host.lock().unwrap() = Some(host.clone());
        Ok(())
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.noted)
    }
    fn create_table(&self, desc: &TableDesc) -> Result<Box<dyn TableStorage>> {
        self.rows.create_table(desc)
    }
    fn checkpoint(&self, at: Lsn) -> Result<()> {
        let host = self.host.lock().unwrap().clone().unwrap();
        let (fs, dir) = host.files.directory(NAME)?;
        let path = dir.join("state");
        let file = fs.open(&path, if fs.exists(&path)? { OpenMode::ReadWrite } else { OpenMode::CreateNew })?;
        file.set_size(0)?;
        file.write_all_at(&[at.0.to_be_bytes().as_slice(), &self.noted.told().to_be_bytes()].concat(), 0)?;
        if fs.exists(&dir.join(REFUSE))? {
            return Err(SqlError::new(Code::Io, "[DESIGN] refused, as the test asked"));
        }
        Ok(())
    }
}

/// A database kept in `dir` of `fs`, the plug-in registered, what the
/// plug-in was given, and its settings hook.
fn opened(fs: &Arc<dyn FileSystem>, dir: &Path) -> (Database, Host, Noted) {
    let (engine, host, noted) = Shadowing::new();
    let db = Database::new();
    db.register_engine(engine).unwrap();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    let given = host.lock().unwrap().clone().unwrap();
    (db, given, noted)
}

/// The plug-in's directory, as the plug-in sees it.
fn directory(host: &Host) -> (Arc<dyn FileSystem>, PathBuf) {
    host.files.directory(NAME).unwrap()
}

/// Writes `bytes` to the plug-in's file `name`, made if missing.
fn put(host: &Host, name: &str, bytes: &[u8]) {
    let (fs, dir) = directory(host);
    let path = dir.join(name);
    let file = fs.open(&path, if fs.exists(&path).unwrap() { OpenMode::ReadWrite } else { OpenMode::CreateNew }).unwrap();
    file.set_size(0).unwrap();
    file.write_all_at(bytes, 0).unwrap();
}

/// The content of the plug-in's file `name` as the plug-in reads it, if
/// it is there.
fn got(host: &Host, name: &str) -> Option<Vec<u8>> {
    let (fs, dir) = directory(host);
    let file = fs.open(&dir.join(name), OpenMode::ReadOnly).ok()?;
    let mut out = vec![0_u8; usize::try_from(file.size().unwrap()).unwrap()];
    file.read_exact_at(&mut out, 0).unwrap();
    Some(out)
}

/// The content of the file at `path` on the disk, if it is there.
fn on_disk(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// Verifies: [DESIGN] RULES.md D56, D30, a plug-in's shadowed directory:
/// what it writes, makes and removes there is read back at once, but
/// reaches the disk only at a checkpoint; a crash loses what was done
/// after the last checkpoint and keeps what was done before, the
/// directory as that checkpoint left it, what the plug-in wrote as its
/// engine was checkpointed among it, its setting as it was told (RULES.md
/// D36), replayed as it opens again; the side files are shown as SHADOW,
/// of the plug-in, and left out of an export. A directory kept as written
/// is written at once, and a crash leaves it as written.
#[test]
fn a_crash_leaves_a_shadowed_directory_as_the_last_checkpoint() {
    let scratch = Scratch::new("plugin-shadowed");
    let dir = scratch.path("db");
    let mine = dir.join("plugins").join(NAME);
    let (db, host, _) = opened(&after_crash(), &dir);
    put(&host, "kept", b"before");
    put(&host, "removed", b"there");
    db.set_setting(DEPTH, &whole(3)).unwrap();
    db.checkpoint().unwrap();
    let state = on_disk(&mine.join("state")).unwrap();
    assert_eq!(state[8..], 3_i64.to_be_bytes(), "its setting's value, as it was told");
    put(&host, "kept", b"AFTER!");
    put(&host, "made", b"new");
    let (fs, plugin) = directory(&host);
    fs.remove(&plugin.join("removed")).unwrap();
    assert_eq!(
        (got(&host, "kept").unwrap(), got(&host, "made").unwrap(), got(&host, "removed")),
        (b"AFTER!".to_vec(), b"new".to_vec(), None)
    );
    assert_eq!(on_disk(&mine.join("kept")).unwrap(), b"before", "not yet on the disk");
    assert!(on_disk(&mine.join("made")).is_none() && on_disk(&mine.join("removed")).is_some());
    let (written, place) = host.files.directory("written").unwrap();
    written.open(&place.join("file"), OpenMode::CreateNew).unwrap().write_all_at(b"at once", 0).unwrap();
    assert_eq!(on_disk(&place.join("file")).unwrap(), b"at once", "as written");
    let exported: Vec<String> = db.export_manifest().unwrap().iter().map(|f| f.path.display().to_string()).collect();
    assert!(exported.iter().any(|p| p.ends_with("kept")) && !exported.iter().any(|p| p.contains(".shadow")), "{exported:?}");
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    let shown = crate::common::rows(
        &db.session("OWNER"),
        "SELECT KIND, CLASS FROM INFORMATION_SCHEMA.FILES WHERE PATH LIKE '%shadowing%.shadow_' GROUP BY KIND, CLASS",
    )
    .unwrap();
    assert_eq!(shown, ["SHADOW,DURABLE AT COMMIT"]);
    drop(db);

    let (db, host, noted) = opened(&after_crash(), &dir);
    assert_eq!(noted.told(), 3, "its setting replayed as it opened");
    assert_eq!(got(&host, "kept").unwrap(), b"before", "the write after the checkpoint lost");
    assert_eq!((got(&host, "made"), got(&host, "removed").unwrap()), (None, b"there".to_vec()), "made lost, removed back");
    assert_eq!(got(&host, "state").unwrap(), state, "as the plug-in's checkpoint wrote it");
    assert_eq!(on_disk(&place.join("file")).unwrap(), b"at once");
    let left: Vec<PathBuf> = std::fs::read_dir(&mine).unwrap().map(|e| e.unwrap().path()).collect();
    assert!(!left.iter().any(|p| p.to_string_lossy().contains(".shadow")), "side files thrown away: {left:?}");
    put(&host, "kept", b"closed");
    db.close().unwrap();
    assert_eq!(on_disk(&mine.join("kept")).unwrap(), b"closed", "a close is a checkpoint");
}

/// Verifies: [DESIGN] RULES.md D56, D30, a plug-in's capture that fails
/// gives up what was held, and the checkpoint fails; a crash between the
/// seal of a plug-in's shadowed directory and its commit: a checkpoint whose
/// commit fails before the commit block is durable leaves the files as the
/// checkpoint before; one that fails after it, as the file is copied
/// into, has the copy made as the database opens again, the files as that
/// checkpoint captured them.
#[test]
fn a_crash_between_seal_and_commit() {
    let scratch = Scratch::new("plugin-sealed");
    let dir = scratch.path("db");
    let (fs, faults) = FaultyFs::with_faults();
    let (db, host, _) = opened(&fs, &dir);
    put(&host, "file", b"one");
    put(&host, REFUSE, b"");
    assert_eq!(db.checkpoint().err().map(|e| e.code), Some(Code::Io), "the plug-in's capture fails");
    assert_eq!(got(&host, "state").map(|s| s.len()), Some(16), "what it wrote, held, given up to its side file");
    let (fs_of, place) = directory(&host);
    fs_of.remove(&place.join(REFUSE)).unwrap();
    db.checkpoint().unwrap();
    put(&host, "file", b"two");
    faults.inject(Rule::fail(Op::Sync).on("shadowing/file.shadow"));
    assert_eq!(db.checkpoint().err().map(|e| e.code), Some(Code::Io), "sealed, not committed");
    faults.clear();
    let mine = dir.join("plugins").join(NAME);
    assert_eq!(on_disk(&mine.join("file")).unwrap(), b"one");
    drop(db);
    // The crash: what no sync covered lost, the commit block among it.
    for side in ["file.shadow0", "file.shadow1"].map(|name| mine.join(name)).iter().filter(|p| p.exists()) {
        faults.cut_to_synced(side);
    }
    let (db, host, _) = opened(&after_crash(), &dir);
    assert_eq!(got(&host, "file").unwrap(), b"one", "as the checkpoint before");

    drop((db, host));
    let (db, host, _) = opened(&fs, &dir);
    put(&host, "file", b"four");
    faults.inject(Rule::fail(Op::SetSize).on("shadowing/file"));
    assert_eq!(db.checkpoint().err().map(|e| e.code), Some(Code::Io), "the copy fails");
    faults.clear();
    assert_eq!(on_disk(&dir.join("plugins").join(NAME).join("file")).unwrap(), b"one", "not yet copied");
    drop(db);
    let (_, host, _) = opened(&after_crash(), &dir);
    assert_eq!(got(&host, "file").unwrap(), b"four", "the copy made again as it opened");
}

/// Verifies: [DESIGN] RULES.md D56, declarations: a directory declared
/// otherwise than before, or shadowed once given as written, is refused,
/// as a name that is no directory's, with SQLCODE -910; a database in
/// memory keeps a shadowed directory as written.
#[test]
fn declarations_are_kept() {
    let (db, (engine, host, _)) = (Database::new(), Shadowing::new());
    db.register_engine(engine).unwrap();
    let host = host.lock().unwrap().clone().unwrap();
    let code = |r: Result<()>| r.err().map(|e| e.code);
    assert_eq!(code(host.files.declare(NAME, Kept::AsWritten)), Some(Code::Api), "declared shadowed");
    assert_eq!(code(host.files.declare(NAME, Kept::Shadowed)), None, "again, the same");
    assert_eq!(code(host.files.declare("no/name", Kept::Shadowed)), Some(Code::Api));
    host.files.directory("given").unwrap();
    assert_eq!(code(host.files.declare("given", Kept::Shadowed)), Some(Code::Api), "given as written");
    assert_eq!(code(host.files.declare("given", Kept::AsWritten)), None);
    put(&host, "file", b"in memory");
    assert_eq!(got(&host, "file").unwrap(), b"in memory");
    assert!(format!("{:?}", host.files).contains("Shadowed"));
}
