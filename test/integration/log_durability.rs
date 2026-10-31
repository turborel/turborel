//! The log's durability as a setting (RULES.md D25, D36): LOG.DURABILITY,
//! changed while the database is open, holds from the log's next sync,
//! and each segment of the file log begun after is made as it says; the
//! first sync after NONE makes every record before it durable, earlier
//! segments and their directory too; the Strict policy takes the log as
//! durable as it is now; the value survives the database being opened
//! again; a log plug-in is told it, and the memory log stays not durable.

use std::path::PathBuf;
use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Opened};
use turborel::disk::io::{Durability, FileSystem};
use turborel::error::Code;
use turborel::log::{FileLog, FileLogOptions, Log, Lsn, MemoryLog};
use turborel::threads::JobPolicy;
use turborel::txn::Policy;
use turborel::value::Value;

use crate::common::{Scratch, Watched, count, crc32c, os, small_segments};
use crate::durability::{declare_in, insert_alone};
use crate::faults::{Faults, FaultyFs, Op, Rule};
use crate::log_sync::last_segment;
use crate::logs::{all, record};

/// A word of LOG.DURABILITY, as a value.
fn word(durability: Durability) -> Value {
    Value::Char(durability.word().to_owned())
}

/// The segment files of the log in `dir`, sorted.
fn segments(fs: &dyn FileSystem, dir: &std::path::Path) -> Vec<PathBuf> {
    fs.read_dir(dir).unwrap()
}

/// The durabilities of the syncs `faults` saw since asked last.
fn synced_with(faults: &Faults) -> Vec<Durability> {
    faults.take_syncs().into_iter().map(|(_, d)| d).collect()
}

/// Appends records `from` to `to` (not included) to `log`; the last one's
/// position.
fn appended(log: &FileLog, from: usize, to: usize) -> Lsn {
    (from..to).map(|n| log.append(&record(n)).unwrap()).last().unwrap()
}

/// Verifies: [DESIGN] RULES.md D36: LOG.DURABILITY takes the words of the
/// durabilities, in any case, DATA by default, and no other word; each
/// word names its durability.
#[test]
fn log_durability_takes_the_durabilities_words() {
    for durability in [Durability::None, Durability::Data, Durability::Full] {
        assert_eq!(Durability::of(durability.word()), durability);
        assert_eq!(Durability::of(&durability.word().to_ascii_lowercase()), durability);
        assert!(Durability::WORDS.contains(&durability.word()));
    }
    let db = Database::new();
    assert_eq!(db.setting("LOG.DURABILITY").unwrap().to_string(), "DATA");
    db.set_setting("log.durability", &Value::Char("full".to_owned())).unwrap();
    assert_eq!(db.setting("LOG.DURABILITY").unwrap().to_string(), "FULL");
    let refused = db.set_setting("LOG.DURABILITY", &Value::Char("SOMETIMES".to_owned()));
    assert_eq!(refused.err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [DESIGN] RULES.md D25, D36, file log: its durability changed
/// while it is open holds at once: under `None` no sync, of a record or of
/// a segment begun or sealed, and it says it is not durable; raised to
/// `Full`, the next sync syncs, as `Full`, every segment sealed unsynced
/// and the current one, so after a failure of the system (each segment
/// cut to its last sync) every record is there; lowered to `None` again,
/// it syncs nothing, nor its directory as it truncates.
#[test]
fn a_file_log_syncs_as_its_durability_says_now() {
    let scratch = Scratch::new("log-durability-file");
    let dir = scratch.path("log");
    let (fs, faults) = FaultyFs::with_faults();
    let log = FileLog::create(Arc::clone(&fs), &dir, crc32c(), small_segments()).unwrap();
    log.sync(log.append(&record(0)).unwrap()).unwrap();
    assert_eq!(synced_with(&faults).last(), Some(&Durability::Data));
    log.set_durability(Durability::None);
    assert!(!log.capabilities().durable);
    let last = appended(&log, 1, 60);
    log.sync(last).unwrap();
    assert!(log.counts().rotations > 1, "{:?}", log.counts());
    assert_eq!(synced_with(&faults), [], "nothing synced, no segment's creation either");
    log.set_durability(Durability::Full);
    assert!(log.capabilities().durable);
    log.sync(last).unwrap();
    let synced = faults.take_syncs();
    assert!(synced.iter().all(|(_, d)| *d == Durability::Full), "{synced:?}");
    for segment in segments(&*fs, &dir) {
        assert!(synced.iter().any(|(p, _)| *p == segment), "{} synced", segment.display());
    }
    drop(log);
    for segment in segments(&*fs, &dir) {
        faults.cut_to_synced(&segment);
    }
    let (log, recovery) = FileLog::open(Arc::clone(&fs), &dir, &[crc32c()], &[], small_segments()).unwrap();
    assert_eq!((all(&log, Lsn(0)).len(), recovery.cut), (60_usize, 0_u64));
    log.set_durability(Durability::None);
    let later = appended(&log, 60, 120);
    log.sync(later).unwrap();
    faults.inject(Rule::fail(Op::SyncDir));
    log.truncate_before(later).unwrap();
    assert_eq!((synced_with(&faults), faults.injected()), (vec![], 0_usize), "nothing synced");
}

/// Verifies: [DESIGN] RULES.md D25, D36, file log: the first sync after
/// its durability rises from `None` syncs its directory, where segments
/// were made unsynced, before claiming any record durable: that sync
/// failing fails it and every later one. A log that never synced nothing
/// syncs no directory as it syncs. A log opened syncing nothing takes no
/// older segment as durable: raised, its next sync syncs each.
///
/// MC/DC of whether a sync syncs the directory (an entry made unsynced):
/// a log made `None`, then raised, against one made `Data`.
#[test]
fn a_raised_durability_syncs_the_directory_first() {
    let scratch = Scratch::new("log-durability-directory");
    let (fs, faults) = FaultyFs::with_faults();
    let unsynced = FileLogOptions { durability: Durability::None, ..small_segments() };
    let made_none = FileLog::create(Arc::clone(&fs), &scratch.path("none"), crc32c(), unsynced.clone()).unwrap();
    let made_data = FileLog::create(Arc::clone(&fs), &scratch.path("data"), crc32c(), small_segments()).unwrap();
    let (none_last, data_last) = (appended(&made_none, 0, 60), appended(&made_data, 0, 60));
    faults.inject(Rule::fail(Op::SyncDir));
    made_data.sync(data_last).unwrap();
    made_none.set_durability(Durability::Data);
    assert_eq!(made_none.sync(none_last).err().map(|e| e.code), Some(Code::Io), "its directory not synced");
    faults.clear();
    assert_eq!(made_none.sync(none_last).err().map(|e| e.code), Some(Code::Io), "nor any later sync");
    drop(made_none);
    faults.take_syncs();
    let (opened, _) = FileLog::open(Arc::clone(&fs), &scratch.path("none"), &[crc32c()], &[], unsynced).unwrap();
    opened.set_durability(Durability::Data);
    opened.sync(Lsn(opened.end().0 - 1_u64)).unwrap();
    let synced = faults.take_syncs();
    for segment in segments(&*fs, &scratch.path("none")) {
        assert!(synced.iter().any(|(p, d)| *p == segment && *d == Durability::Data), "{} synced", segment.display());
    }
}

/// Verifies: [DESIGN] RULES.md D25, D36, a database kept in a directory:
/// a commit under LOG.DURABILITY NONE is lost by a failure of the system
/// (the log cut to its last sync), and the setting, synced as it changed,
/// survives it, the database opened again under it; raised to DATA, the
/// next commit, synced, survives one, and the setting with it.
#[test]
fn a_change_of_log_durability_holds_from_the_next_sync() {
    let scratch = Scratch::new("log-durability-database");
    let dir = scratch.path("db");
    let (fs, faults) = FaultyFs::with_faults();
    let open = || {
        let db = Database::new();
        let opened = db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        db.set_job_policy(JobPolicy::Manual).unwrap();
        (db, opened)
    };
    let (db, _) = open();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER)").unwrap();
    db.set_setting("LOG.DURABILITY", &word(Durability::None)).unwrap();
    let insert = |db: &Database, id: i32| {
        let s = db.session("APP");
        s.run(&format!("INSERT INTO T (ID) VALUES ({id})")).unwrap();
        s.commit().unwrap();
    };
    insert(&db, 1_i32);
    drop(db);
    faults.cut_to_synced(&last_segment(&dir));
    let (db, opened) = open();
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(db.setting("LOG.DURABILITY").unwrap().to_string(), "NONE", "the setting kept");
    assert_eq!(count(&db.session("APP"), "SELECT ID FROM T"), 0_usize, "the commit under NONE lost");
    db.set_setting("LOG.DURABILITY", &word(Durability::Data)).unwrap();
    insert(&db, 2_i32);
    drop(db);
    faults.cut_to_synced(&last_segment(&dir));
    let (db, _) = open();
    assert_eq!(db.setting("LOG.DURABILITY").unwrap().to_string(), "DATA");
    assert_eq!(count(&db.session("APP"), "SELECT ID FROM T"), 1_usize, "the commit under DATA kept");
    db.close().unwrap();
}

/// Verifies: [X3.135-1986 §4.16]; [DESIGN] RULES.md D25, D36, a database
/// kept in a directory: under the Strict policy, a `Forced` engine is
/// written and a `None` engine refused while the file log is durable,
/// and the other way round once LOG.DURABILITY is NONE, at once; set
/// FULL by its owner in SQL, as the Operator's Guide shows (`SET DATABASE
/// LOG.DURABILITY = 'FULL'`), as at first again.
#[test]
fn strict_takes_the_log_as_durable_as_it_is_now() {
    let scratch = Scratch::new("log-durability-strict");
    let db = Database::new();
    db.open_files(&os(), &scratch.path("db"), &OpenOptions::default()).unwrap();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    declare_in(&db, Policy::Strict);
    let written = |db: &Database| (insert_alone(db, "F"), insert_alone(db, "N"));
    assert_eq!(written(&db), (None, Some(Code::Policy)), "durable");
    db.set_setting("LOG.DURABILITY", &word(Durability::None)).unwrap();
    assert_eq!(written(&db), (Some(Code::Policy), None), "not durable");
    db.session("OWNER").run("SET DATABASE LOG.DURABILITY = 'FULL'").unwrap();
    assert_eq!(db.setting("LOG.DURABILITY").unwrap().to_string(), "FULL");
    assert_eq!(written(&db), (None, Some(Code::Policy)), "durable again");
}

/// Verifies: [DESIGN] RULES.md D25, D36, log plug-ins: a plug-in is told
/// LOG.DURABILITY as the database is given it and at each change, and the
/// Strict policy takes it as durable as it then says: one over the file
/// log, passing it on, follows it; one that keeps its own durability
/// keeps it; the memory log, told it, stays not durable.
#[test]
fn a_log_plug_in_is_told_the_durability() {
    let scratch = Scratch::new("log-durability-plug-in");
    let file = FileLog::create(os(), &scratch.path("log"), crc32c(), FileLogOptions::default()).unwrap();
    let memory = Arc::new(MemoryLog::new());
    // Each plug-in, and the codes of a write to the `Forced` engine's table
    // before and after LOG.DURABILITY is NONE.
    let refused = Some(Code::Policy);
    let kinds = [
        (Watched { follows: true, ..Watched::over(Arc::new(file)) }, (None, refused)),
        (Watched { durable: true, ..Watched::default() }, (None, None)),
        (Watched { follows: true, ..Watched::over(Arc::clone(&memory) as Arc<dyn Log>) }, (refused, refused)),
    ];
    for (n, (plug_in, expected)) in kinds.into_iter().enumerate() {
        let db = Database::new();
        declare_in(&db, Policy::Strict);
        let log = Arc::new(plug_in);
        db.set_log(Arc::clone(&log) as Arc<dyn Log>).unwrap();
        let before = insert_alone(&db, "F");
        db.set_setting("LOG.DURABILITY", &word(Durability::None)).unwrap();
        assert_eq!((before, insert_alone(&db, "F")), expected, "plug-in {n}");
        assert_eq!(*log.told.lock().unwrap(), [Durability::Data, Durability::None], "plug-in {n}");
    }
    assert!(!memory.capabilities().durable);
}
