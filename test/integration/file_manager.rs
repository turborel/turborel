//! The file manager (RULES.md D56): a database's files opened through it,
//! no more open at once than FILES.MAX_OPEN, the one used least recently
//! closed and opened again as needed, each shown in
//! INFORMATION_SCHEMA.FILES to the database's owner.

use std::sync::{Arc, Mutex};

use turborel::catalog::TableDesc;
use turborel::db::{Database, OpenOptions};
use turborel::disk::io::{DiskFile, Durability, FileSystem, OpenMode};
use turborel::engine::rows::{RowStore, TableOptions};
use turborel::error::{Code, Result};
use turborel::files::{FileManager, Registry};
use turborel::memory::Host;
use turborel::storage::{StorageEngine, TableStorage};
use turborel::value::{Decimal, Value};

use crate::common::{Kept, Scratch, os, rows, small_segments};
use crate::faults::{FaultyFs, Op, Rule};

/// Verifies: [DESIGN] RULES.md D56, D36: a database of more files than
/// FILES.MAX_OPEN, its log and its older rows' in many small segments,
/// works as any: what it writes is read back, through a checkpoint and a
/// close and an open again; no more than the limit are open at once; and
/// INFORMATION_SCHEMA.FILES shows its owner each file, of its kind, and
/// another user none.
#[test]
fn a_database_of_more_files_than_its_limit() {
    let scratch = Scratch::new("file-manager");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.set_setting("LOG.SEGMENT_SIZE", &Value::Exact(Decimal::new(4096, 0))).unwrap();
        db.set_setting("FILES.MAX_OPEN", &Value::Exact(Decimal::new(4, 0))).unwrap();
        db.open_files(&os(), &dir, &OpenOptions { undo: small_segments(), ..OpenOptions::default() }).unwrap();
        db
    };
    let db = open();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, S CHARACTER(200))").unwrap();
    let a = db.session("A");
    for k in 0..400 {
        a.run(&format!("INSERT INTO T VALUES ({k}, 'row {k}')")).unwrap();
    }
    a.commit().unwrap();
    a.run("UPDATE T SET S = 'changed'").unwrap();
    let owner = db.session("OWNER");
    let shown = |sql: &str| rows(&owner, sql).unwrap();
    let files: usize = shown("SELECT COUNT(*) FROM INFORMATION_SCHEMA.FILES")[0].parse().unwrap();
    let open_now: usize = shown("SELECT COUNT(*) FROM INFORMATION_SCHEMA.FILES WHERE IS_OPEN = 'YES'")[0].parse().unwrap();
    assert!(files > 4 && open_now <= 4, "{files} files, {open_now} open");
    let kinds = shown("SELECT DISTINCT KIND FROM INFORMATION_SCHEMA.FILES ORDER BY KIND");
    assert!(["CATALOG", "DATA", "LOG", "UNDO"].iter().all(|k| kinds.contains(&(*k).to_owned())), "{kinds:?}");
    // As the Operator's Guide shows it.
    assert!(shown("SELECT KIND, COUNT(*) FROM INFORMATION_SCHEMA.FILES GROUP BY KIND").len() >= 4);
    assert_eq!(rows(&db.session("OTHER"), "SELECT COUNT(*) FROM INFORMATION_SCHEMA.FILES"), Ok(vec!["0".into()]));
    a.commit().unwrap();
    db.checkpoint().unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T WHERE S = 'changed'"), Ok(vec!["400".into()]));
    a.commit().unwrap();
    owner.commit().unwrap();
    db.close().unwrap();
    drop((a, owner, db));
    let again = open();
    assert_eq!(rows(&again.session("A"), "SELECT COUNT(*) FROM T WHERE S = 'changed'"), Ok(vec!["400".into()]));
}

/// Verifies: [DESIGN] RULES.md D56: on Unix, the directories and files a
/// database makes are its user's alone: directories 0700, files 0600.
#[cfg(unix)]
#[test]
fn files_made_are_their_users_alone() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("file-modes");
    let dir = scratch.path("db");
    let db = Database::new();
    db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    db.session("A").run("INSERT INTO T VALUES (1)").unwrap();
    db.close().unwrap();
    let mut seen = 0_usize;
    let mut pending = vec![dir];
    while let Some(at) = pending.pop() {
        let meta = std::fs::metadata(&at).unwrap();
        let expected = if meta.is_dir() { 0o700 } else { 0o600 };
        assert_eq!(meta.permissions().mode() & 0o777, expected, "{}", at.display());
        if meta.is_dir() {
            pending.extend(std::fs::read_dir(&at).unwrap().map(|e| e.unwrap().path()));
        }
        seen = seen.saturating_add(1);
    }
    assert!(seen > 3, "the directory, and its files: {seen}");
}

/// Verifies: [DESIGN] RULES.md D56: closing a database removes the spill
/// files left in its `tmp/` before it gives up its directory, and, every
/// file closed, tells of none left: its last event is that it closed.
#[test]
fn closing_removes_spill_files_and_closes_every_file() {
    let scratch = Scratch::new("file-close");
    let dir = scratch.path("db");
    let kept = Kept::default();
    let db = Database::new();
    db.set_diagnostics(Box::new(kept.clone()));
    db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    std::fs::create_dir_all(dir.join("tmp")).unwrap();
    let stray = dir.join("tmp").join("left-0.spill");
    std::fs::write(&stray, b"rows").unwrap();
    kept.taken();
    db.close().unwrap();
    assert!(!stray.exists(), "removed as it closed");
    let events: Vec<String> = kept.taken().into_iter().map(|(_, e)| e).collect();
    assert_eq!(events, vec!["Closed".to_owned()]);
}

/// What a plug-in was given as it was registered.
type Given = Arc<Mutex<Option<Host>>>;

/// A plug-in engine keeping, for each table it makes, a file of its own in
/// its directory, and a scratch file; its rows kept by a row store.
struct Keeping {
    rows: RowStore,
    host: Given,
    kept: Mutex<Vec<Box<dyn DiskFile>>>,
}

impl StorageEngine for Keeping {
    fn name(&self) -> &'static str {
        "keeping"
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.rows.attach(host)?;
        *self.host.lock().unwrap() = Some(host.clone());
        Ok(())
    }
    fn create_table(&self, desc: &TableDesc) -> Result<Box<dyn TableStorage>> {
        let host = self.host.lock().unwrap().clone().unwrap();
        let (fs, dir) = host.files.directory(self.name())?;
        let file = fs.open(&dir.join(&desc.name.ident), OpenMode::CreateNew)?;
        file.write_all_at(b"made", 0)?;
        file.sync(Durability::Full)?;
        let scratch = host.work.scratch();
        let path = scratch.path()?;
        drop(scratch.fs().open(&path, OpenMode::CreateNew)?);
        scratch.fs().remove(&path)?;
        self.kept.lock().unwrap().push(file);
        self.rows.create_table(desc)
    }
}

/// The engine, and what it is given.
fn keeping() -> (Box<Keeping>, Given) {
    let host = Given::default();
    let engine = Keeping { rows: RowStore::in_memory(TableOptions::default()), host: Arc::clone(&host), kept: Mutex::new(Vec::new()) };
    (Box::new(engine), host)
}

/// Verifies: [DESIGN] RULES.md D56, `Host::files`: a plug-in keeps files of
/// its own in a directory of its name, under the database's `plugins/`
/// once it is kept in a directory, though given before it opened, shown
/// in INFORMATION_SCHEMA.FILES as PLUGIN; in memory for a database kept in
/// memory; a name that is no directory's is refused (SQLCODE -910); and
/// it makes scratch files where the database does.
#[test]
fn plug_ins_keep_files_of_their_own() {
    let scratch = Scratch::new("file-plugins");
    let dir = scratch.path("db");
    let db = Database::new();
    db.register_engine(keeping().0).unwrap();
    db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER) USING keeping").unwrap();
    assert_eq!(std::fs::read(dir.join("plugins").join("keeping").join("T")).unwrap(), b"made");
    let shown = rows(&db.session("OWNER"), "SELECT COUNT(*) FROM INFORMATION_SCHEMA.FILES WHERE KIND = 'PLUGIN'").unwrap();
    assert_eq!(shown, ["1"]);

    let in_memory = Database::new();
    let (engine, given) = keeping();
    in_memory.register_engine(engine).unwrap();
    in_memory.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER) USING keeping").unwrap();
    in_memory.session("A").run("INSERT INTO T VALUES (1)").unwrap();
    let host = given.lock().unwrap().clone().unwrap();
    let (fs, mine) = host.files.directory("keeping").unwrap();
    assert!(fs.exists(&mine.join("T")).unwrap(), "in memory");
    let refused = |name: &str| host.files.directory(name).err().map(|e| e.code);
    assert_eq!((refused("../up"), refused("")), (Some(Code::Api), Some(Code::Api)));
    assert_eq!(refused("fine_name-1"), None);
}

/// Verifies: [DESIGN] RULES.md D56, D30: over the operating system's file
/// system, a file with writes not yet synced whose sync fails as it is to
/// be closed for another stays open, its failure told, until a sync of it
/// succeeds and it is closed; its next sync, opened again, returns that
/// failure, though the fault has passed, so no sync stands for the writes
/// lost; the one after succeeds, and what was written is there. Regression
/// test (R-TEST-7): the file was closed unsynced, and a sync of it opened
/// again succeeded.
#[test]
fn a_failed_sync_before_closing_fails_the_files_next_sync() {
    let scratch = Scratch::new("file-lost-sync");
    let (inner, faults) = FaultyFs::with_faults();
    let registry = Arc::new(Registry::default());
    registry.set_limit(1);
    let kept = Kept::default();
    registry.telling(Arc::new(kept.clone()));
    let fs = FileManager::over(inner, Arc::clone(&registry));
    let written = fs.open(&scratch.path("written"), OpenMode::CreateNew).unwrap();
    written.write_all_at(b"data", 0).unwrap();
    faults.inject(Rule::fail(Op::Sync).on("written").times(1));
    let other = fs.open(&scratch.path("other"), OpenMode::CreateNew);
    assert_eq!(faults.injected(), 1, "synced before it was closed");
    let told: Vec<String> = kept.taken().into_iter().map(|(_, e)| e).collect();
    assert!(told.len() == 1 && told[0].contains("injected: Sync"), "{told:?}");
    assert!(registry.infos().iter().any(|i| i.path.ends_with("written") && !i.open), "closed once synced");
    let lost = written.sync(Durability::Full).unwrap_err();
    assert_eq!(lost.code, Code::Io);
    assert!(lost.msg.contains("injected: Sync"), "{}", lost.msg);
    written.sync(Durability::Full).unwrap();
    let mut back = [0_u8; 4];
    written.read_exact_at(&mut back, 0).unwrap();
    assert_eq!(&back, b"data");
    drop((written, other));
}
