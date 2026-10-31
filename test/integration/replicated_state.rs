//! Replicated state (RULES.md D57): all of a database's durable state is
//! in its log, its catalog, or the durable files its file manager knows.
//! Every kind of log record, listed in `record::KINDS`, is written by a
//! statement, and recovery, which is the database's replay of its log,
//! makes again from it what the statement did: a database crashed after
//! the statement and recovered holds the same rows, index entries and
//! Information Schema as before the crash. Every durable file the file
//! manager registers is in the database's export manifest, or carried by
//! the log or the catalog. A database following another's log, by the
//! standby seam, replays it as recovery does, and serves once promoted.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Opened, Standby};
use turborel::error::Code;
use turborel::files::FileOwner;
use turborel::index::BTreeMethod;
use turborel::log::record::{KINDS, Record};
use turborel::log::{FileLog, FileLogOptions, Log, Lsn, MemoryLog};
use turborel::value::types::TypeRegistry;

use crate::common::{Scratch, Watched, after_crash, contents, crc32c, exec, number, os, rows};
use crate::file_manager::keeping;
use crate::table_settings::{Calls, Noted};

/// The users whose sessions the state is read through: the schemas'
/// owners, and the database's owner, once set, who alone sees USERS.
const READERS: [&str; 3] = ["APP", "B", "U"];

/// The database in `dir`, with the index method "noted", which declares
/// options, and no checkpoint but those asked for, so recovery reads the
/// log from its start.
fn open(dir: &Path) -> (Database, Opened) {
    let db = Database::new();
    db.register_index_method(Box::new(Noted(BTreeMethod::in_memory(), Calls::default()))).unwrap();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    (db, opened)
}

/// Schema APP: T, with a UNIQUE key, a B-link index and one of "noted", 10
/// rows committed.
fn base(db: &Database) {
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(10))").unwrap();
    db.create_index("T_V", "APP.T", &["V"], "btree").unwrap();
    let s = db.session("APP");
    exec(&s, "CREATE INDEX T_N ON T (K) USING NOTED WITH (FILL = 60)").unwrap();
    for k in 0..10 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'v{k}')")).unwrap();
    }
    s.commit().unwrap();
}

/// Runs each of `statements` in a session of APP, committing after each.
fn committed(db: &Database, statements: &[&str]) {
    let s = db.session("APP");
    for sql in statements {
        exec(&s, sql).unwrap();
        s.commit().unwrap();
    }
}

/// A kind of record, and what writes one, on a database of `base`.
struct Writer {
    kind: u8,
    run: fn(&Database),
}

/// For every kind of record, a statement that writes it. Records that are
/// not synced as written (an abort record) are followed by a commit, whose
/// sync makes them durable.
const WRITERS: [Writer; 18] = [
    Writer { kind: 1, run: |db| committed(db, &["INSERT INTO T VALUES (100, 'new')"]) },
    Writer { kind: 2, run: |db| committed(db, &["UPDATE T SET V = 'changed' WHERE K = 1"]) },
    Writer { kind: 3, run: |db| committed(db, &["DELETE FROM T WHERE K = 2"]) },
    Writer { kind: 4, run: |db| committed(db, &["INSERT INTO T VALUES (101, 'one')"]) },
    Writer {
        kind: 5,
        run: |db| {
            let s = db.session("APP");
            s.run("INSERT INTO T VALUES (102, 'gone')").unwrap();
            s.rollback().unwrap();
            committed(db, &["INSERT INTO T VALUES (103, 'after')"]);
        },
    },
    Writer {
        kind: 6,
        run: |db| {
            db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE S (K INTEGER)").unwrap();
        },
    },
    Writer { kind: 7, run: |db| db.create_index("T_K", "APP.T", &["K"], "hash").unwrap() },
    Writer {
        kind: 8,
        run: |db| {
            let s = db.session("APP");
            s.run("INSERT INTO T VALUES (104, 'kept')").unwrap();
            assert!(s.run("INSERT INTO T VALUES (3, 'duplicate')").is_err(), "UNIQUE: the statement undone");
            s.commit().unwrap();
        },
    },
    Writer { kind: 9, run: |db| db.checkpoint().unwrap() },
    Writer { kind: 10, run: |db| committed(db, &["DROP INDEX T_N"]) },
    Writer { kind: 11, run: |db| committed(db, &["CREATE TABLE U (K INTEGER)", "DROP TABLE U CASCADE"]) },
    Writer { kind: 12, run: |db| committed(db, &["ALTER TABLE T ADD COLUMN W INTEGER DEFAULT 7"]) },
    Writer { kind: 13, run: |db| db.set_setting("OPTIMIZER.MAX_PASSES", &number(3)).unwrap() },
    Writer { kind: 14, run: |db| committed(db, &["ALTER INDEX T_N SET (FILL = 40)"]) },
    Writer {
        kind: 15,
        run: |db| {
            // Versions a reader may still see, awaiting collection as the
            // checkpoint is taken: their index entries are logged.
            let reader = db.session("APP");
            assert_eq!(rows(&reader, "SELECT COUNT(*) FROM T WHERE K < 5").map(|r| r.len()), Ok(1), "a snapshot, held open");
            committed(db, &["UPDATE T SET V = 'later' WHERE K < 5"]);
            db.checkpoint().unwrap();
            reader.commit().unwrap();
        },
    },
    Writer { kind: 16, run: |db| db.create_user("U", b"verifier").unwrap() },
    Writer {
        kind: 17,
        run: |db| {
            db.create_user("U", b"verifier").unwrap();
            db.set_owner("U").unwrap();
        },
    },
    Writer { kind: 18, run: |db| committed(db, &["CREATE VIEW W AS SELECT K FROM T", "GRANT SELECT ON T TO B"]) },
];

/// What the database holds, as its users see it, through a session of each
/// of `READERS`.
fn state(db: &Database) -> Vec<String> {
    contents(db, &READERS)
}

/// The kinds of record in the log in `dir`, read as recovery will read
/// them.
fn kinds_logged(dir: &Path) -> Vec<u8> {
    let (log, _) = FileLog::open(os(), &dir.join("log"), &[crc32c()], &[], FileLogOptions::default()).unwrap();
    log.read_from(Lsn(0)).map(|read| Record::decode(&read.unwrap().1, &TypeRegistry::default()).unwrap().change.kind()).collect()
}

/// Verifies: [DESIGN] RULES.md D57, D25, the coverage of record kinds: for
/// every kind `record::KINDS` lists, a statement writes a record of it,
/// and a database crashed after it and recovered, by the replay of its
/// log, holds what it held before the crash: each table's rows, each
/// index's entries, and the Information Schema, its definitions, settings
/// and users among them; the statements listed cover every kind.
#[test]
fn every_kind_of_record_is_written_and_replayed() {
    let mut covered: Vec<u8> = WRITERS.iter().map(|w| w.kind).collect();
    covered.sort_unstable();
    assert_eq!(covered, KINDS.to_vec(), "a statement for every kind of record, and for none other");
    for writer in &WRITERS {
        let scratch = Scratch::new(&format!("replay-kind-{}", writer.kind));
        let dir = scratch.path("db");
        let (crashed, _) = open(&dir);
        base(&crashed);
        (writer.run)(&crashed);
        let expected = state(&crashed);
        let logged = kinds_logged(&dir);
        assert!(logged.contains(&writer.kind), "kind {} written: {logged:?}", writer.kind);
        let (db, opened) = open(&dir);
        let Opened::Recovered(report) = opened else { panic!("kind {}: {opened:?}", writer.kind) };
        assert!(report.records > 0, "kind {}: {report:?}", writer.kind);
        assert_eq!(state(&db), expected, "kind {}", writer.kind);
        drop((db, crashed));
    }
}

/// Every file under `dir`, on disk, by path.
fn on_disk(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        for entry in std::fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Verifies: [DESIGN] RULES.md D57, D56, the file manager's check: after
/// statements writing every kind of record but a second user's, a plug-in's table keeping a
/// file of its own, and a checkpoint, every file INFORMATION_SCHEMA.FILES
/// shows of CLASS DURABLE is in the export manifest, but the log's and the
/// catalog's, which their own stores carry (no export hook is declared);
/// each file in the directory on disk is listed, of those classes' kinds,
/// or the directory's lock; each listed has its length on disk and its
/// owner, the plug-in's own file the plug-in's; a file of no kind the
/// database knows is refused (SQLCODE -910), naming it; a database in
/// memory exports nothing (-910).
#[test]
fn every_durable_file_is_exported() {
    let scratch = Scratch::new("replicated-files");
    let dir = scratch.path("db");
    let db = Database::new();
    db.register_engine(keeping().0).unwrap();
    db.register_index_method(Box::new(Noted(BTreeMethod::in_memory(), Calls::default()))).unwrap();
    db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    base(&db);
    // Each once, in an order they all run in: the index altered before it
    // is dropped; the user made by the owner's statement alone.
    let order = WRITERS.iter().filter(|w| w.kind != 10 && w.kind != 16).chain(WRITERS.iter().filter(|w| w.kind == 10));
    for writer in order {
        (writer.run)(&db);
    }
    db.schema("CREATE SCHEMA AUTHORIZATION K CREATE TABLE P (K INTEGER) USING keeping").unwrap();
    db.checkpoint().unwrap();
    let manifest = db.export_manifest().unwrap();
    let listed: Vec<&Path> = manifest.iter().map(|f| f.path.as_path()).collect();
    let shown = rows(&db.session("U"), "SELECT PATH, KIND, CLASS FROM INFORMATION_SCHEMA.FILES").unwrap();
    let durable: Vec<(String, String)> = shown
        .iter()
        .filter_map(|row| {
            let mut fields = row.split(',');
            let (path, kind, class) = (fields.next()?, fields.next()?, fields.next()?);
            (class == "DURABLE").then(|| (path.to_owned(), kind.to_owned()))
        })
        .collect();
    assert!(durable.iter().any(|(_, kind)| kind == "DATA") && durable.iter().any(|(_, kind)| kind == "PLUGIN"), "{shown:?}");
    for (path, kind) in &durable {
        let carried = kind == "LOG" || kind == "CATALOG";
        assert!(carried || listed.contains(&Path::new(path)), "{path}, of {kind}, registered durable, is in the manifest");
    }
    for path in on_disk(&dir) {
        let name = path.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/");
        let elsewhere = ["lock", "catalog.", "data.shadow", "log/", "undo/", "tmp/"].iter().any(|p| name.starts_with(p));
        assert_eq!(listed.contains(&path.as_path()), !elsewhere, "{name}");
    }
    for file in &manifest {
        assert_eq!(file.length, std::fs::metadata(&file.path).unwrap().len(), "{}", file.path.display());
    }
    let owner = |p: PathBuf| manifest.iter().find(|f| f.path == p).map(|f| f.owner.clone());
    assert_eq!(owner(dir.join("data")), Some(FileOwner::Database));
    assert_eq!(owner(dir.join("plugins").join("keeping").join("P")), Some(FileOwner::Plugin("keeping".into())));

    std::fs::write(dir.join("stray"), b"state").unwrap();
    let refused = db.export_manifest().unwrap_err();
    assert_eq!(refused.code, Code::Api);
    assert!(refused.msg.contains("stray"), "{refused}");
    std::fs::remove_file(dir.join("stray")).unwrap();
    assert_eq!(db.export_manifest().unwrap(), manifest);
    assert_eq!(Database::new().export_manifest().err().map(|e| e.code), Some(Code::Api));
}

/// Schema APP of the standby's tests: T with a UNIQUE key and an index on
/// V, logged to `log`; rows 1 and 2 committed, 1 updated, a row rolled
/// back.
fn primary(log: &Arc<MemoryLog>) -> Database {
    let db = Database::new();
    db.set_log(Arc::clone(log) as Arc<dyn Log>).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(10))").unwrap();
    db.create_index("T_V", "APP.T", &["V"], "btree").unwrap();
    committed(&db, &["INSERT INTO T VALUES (1, 'one')", "INSERT INTO T VALUES (2, 'two')", "UPDATE T SET V = 'uno' WHERE K = 1"]);
    let s = db.session("APP");
    s.run("INSERT INTO T VALUES (9, 'gone')").unwrap();
    s.rollback().unwrap();
    db
}

/// A database to follow another's log, with a log of its own, which the
/// changes it would log are held at (`Standby`).
fn following() -> Database {
    let db = Database::new();
    db.set_log(Arc::new(MemoryLog::new())).unwrap();
    db
}

/// What the standby's tests compare: T's rows, a query by index V, and the
/// entries of each index named.
fn answers(db: &Database, indexes: &[&str]) -> (Vec<String>, Vec<String>, Vec<usize>) {
    let s = db.session("APP");
    let all = rows(&s, "SELECT K, V FROM T ORDER BY K").unwrap();
    let by_v = rows(&s, "SELECT K FROM T WHERE V = 'uno'").unwrap();
    (all, by_v, indexes.iter().map(|ix| db.index_entries(ix).unwrap()).collect())
}

/// Verifies: [DESIGN] RULES.md D57, the standby seam: a database given the
/// records of another's log (`Standby::replay`) holds what the other
/// committed, its rows and index entries, and nothing of a transaction
/// rolled back; given the records after those in a second call, it holds
/// what they did too, an index they define among it; from the first call it
/// follows, taking no change (SQLCODE -917, SQLSTATE 25000) while reads go
/// on; replay gives the position of the last record applied, 0 for none,
/// and refuses a malformed record (SQLCODE -916).
#[test]
fn a_standby_replays_what_its_primary_logged() {
    let log = Arc::new(MemoryLog::new());
    let primary = primary(&log);
    let standby = following();
    assert_eq!(standby.replay(&mut std::iter::empty()).unwrap(), Lsn(0));
    let first = log.end();
    assert_eq!(Standby::replay(&standby, &mut log.read_from(Lsn(0))).unwrap(), Lsn(first.0 - 1));
    assert_eq!(answers(&standby, &["T_V"]), answers(&primary, &["T_V"]));
    assert_eq!(answers(&standby, &[]).0, ["1,uno       ", "2,two       "]);
    let refused = standby.session("APP").run("INSERT INTO T VALUES (5, 'five')").unwrap_err();
    assert_eq!((refused.code, refused.sqlstate().as_str()), (Code::TransactionState, "25000"));
    assert!(refused.msg.contains("follows"), "{refused}");

    committed(&primary, &["DELETE FROM T WHERE K = 2", "INSERT INTO T VALUES (3, 'three')", "CREATE INDEX T_K ON T (K)"]);
    assert_eq!(Standby::replay(&standby, &mut log.read_from(first)).unwrap(), Lsn(log.end().0 - 1));
    assert_eq!(answers(&standby, &["T_V", "T_K"]), answers(&primary, &["T_V", "T_K"]));
    assert_eq!(answers(&standby, &[]).0.len(), 2);

    let malformed = [Ok((Lsn(1), b"not a record".to_vec()))];
    assert_eq!(Database::new().replay(&mut malformed.into_iter()).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Verifies: [DESIGN] RULES.md D57, D25, the standby seam: a standby
/// promoted over a log of its own takes changes again, writing them to
/// that log, its transactions numbered past those it replayed; deposed, it
/// takes none (SQLCODE -917), a writer open then cannot commit (-913), and
/// reads go on; deposed, it neither replays nor is promoted (-910) until
/// opened again. A database that never followed is promoted as it is.
#[test]
fn a_standby_serves_once_promoted_until_deposed() {
    let log = Arc::new(MemoryLog::new());
    drop(primary(&log));
    let last = log.read_from(Lsn(0)).map(|read| Record::decode(&read.unwrap().1, &TypeRegistry::default()).unwrap().tx).max();
    let standby = following();
    Standby::replay(&standby, &mut log.read_from(Lsn(0))).unwrap();
    let own = Arc::new(MemoryLog::new());
    standby.promote(Arc::clone(&own) as Arc<dyn Log>).unwrap();
    let s = standby.session("APP");
    s.run("INSERT INTO T VALUES (4, 'four')").unwrap();
    assert!(s.transaction() > last, "past the ids replayed");
    s.commit().unwrap();
    assert_eq!(own.read_from(Lsn(0)).count(), 2, "its insert and commit, to its own log");

    let writer = standby.session("APP");
    writer.run("INSERT INTO T VALUES (6, 'six')").unwrap();
    standby.depose().unwrap();
    let refused = s.run("INSERT INTO T VALUES (5, 'five')").unwrap_err();
    assert_eq!(refused.code, Code::TransactionState);
    assert!(refused.msg.contains("deposed"), "{refused}");
    assert_eq!(writer.commit().err().map(|e| e.code), Some(Code::CommitFailed));
    assert_eq!(rows(&s, "SELECT K FROM T ORDER BY K").unwrap(), ["1", "2", "4"], "reads go on");
    assert_eq!(Standby::replay(&standby, &mut log.read_from(Lsn(0))).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(standby.promote(Arc::new(MemoryLog::new())).err().map(|e| e.code), Some(Code::Api));

    let fresh = Database::new();
    fresh.promote(Arc::new(MemoryLog::new())).unwrap();
    fresh.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE S (K INTEGER)").unwrap();
}

/// The records `log` holds from `from` on, each given to `standby` in a
/// call of its own, as a group commits them one by one.
fn replay_one_by_one(standby: &Database, log: &dyn Log, from: Lsn) {
    for read in log.read_from(from) {
        let (at, bytes) = read.unwrap();
        assert_eq!(Standby::replay(standby, &mut std::iter::once(Ok((at, bytes)))).unwrap(), at);
    }
}

/// Verifies: [DESIGN] RULES.md D57, the standby seam's continuous replay:
/// given its primary's log one record per call, a standby holds the
/// records of each transaction until the record that ends it, so it shows
/// nothing of a transaction still open, all of one committed across calls
/// (a statement undone among them left out), and nothing of one rolled
/// back across calls; whatever the calls, it ends holding what its primary
/// holds, rows and index entries.
#[test]
fn a_standby_holds_a_transaction_until_it_ends() {
    let log = Arc::new(MemoryLog::new());
    let primary = primary(&log);
    let standby = following();
    replay_one_by_one(&standby, &*log, Lsn(0));
    assert_eq!(answers(&standby, &["T_V"]), answers(&primary, &["T_V"]));

    let mark = log.end();
    let open = primary.session("APP");
    exec(&open, "INSERT INTO T VALUES (5, 'five')").unwrap();
    assert!(open.run("INSERT INTO T VALUES (1, 'again')").is_err(), "UNIQUE: the statement undone");
    let rolled_back = primary.session("APP");
    exec(&rolled_back, "INSERT INTO T VALUES (8, 'eight')").unwrap();
    committed(&primary, &["INSERT INTO T VALUES (6, 'six')"]);
    replay_one_by_one(&standby, &*log, mark);
    assert_eq!(answers(&standby, &[]).0, ["1,uno       ", "2,two       ", "6,six       "], "neither open transaction shown");

    let mark = log.end();
    exec(&open, "INSERT INTO T VALUES (7, 'seven')").unwrap();
    open.commit().unwrap();
    rolled_back.rollback().unwrap();
    committed(&primary, &["UPDATE T SET V = 'dos' WHERE K = 2"]);
    replay_one_by_one(&standby, &*log, mark);
    assert_eq!(answers(&standby, &[]).0, ["1,uno       ", "2,dos       ", "5,five      ", "6,six       ", "7,seven     "]);
    assert_eq!(answers(&standby, &["T_V"]), answers(&primary, &["T_V"]));
}

/// The ids of the transactions whose abort records `log` holds, in order.
fn aborted(log: &dyn Log) -> Vec<u64> {
    let decoded = log.read_from(Lsn(0)).map(|read| Record::decode(&read.unwrap().1, &TypeRegistry::default()).unwrap());
    decoded.filter(|r| matches!(r.change, turborel::log::record::Change::Abort)).map(|r| r.tx).collect()
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.2], the standby seam: promoted,
/// a standby ends each transaction its log left open, appending its abort
/// record to its new log and syncing it before it serves, so a standby
/// following that log after the first ends it too, holding nothing of it;
/// its own transactions are numbered past the open ones; if the new log
/// cannot sync, promotion fails, the standby still follows (SQLCODE -917)
/// and the transactions stay open, ended by the next promotion.
#[test]
fn a_promoted_standby_ends_the_transactions_left_open() {
    let log = Arc::new(MemoryLog::new());
    let primary = primary(&log);
    let open = primary.session("APP");
    exec(&open, "INSERT INTO T VALUES (9, 'nine')").unwrap();
    let left = open.transaction().unwrap();
    let standby = following();
    Standby::replay(&standby, &mut log.read_from(Lsn(0))).unwrap();

    let failing = Arc::new(Watched::default());
    failing.fail_syncs.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(standby.promote(Arc::clone(&failing) as Arc<dyn Log>).err().map(|e| e.code), Some(Code::Io));
    assert_eq!(standby.session("APP").run("INSERT INTO T VALUES (4, 'four')").err().map(|e| e.code), Some(Code::TransactionState));

    let own = Arc::new(MemoryLog::new());
    standby.promote(Arc::clone(&own) as Arc<dyn Log>).unwrap();
    assert_eq!(aborted(&*own), [left], "the transaction left open ended in the new log");
    let s = standby.session("APP");
    s.run("INSERT INTO T VALUES (4, 'four')").unwrap();
    assert!(s.transaction().unwrap() > left, "past the ids held");
    s.commit().unwrap();
    assert_eq!(answers(&standby, &[]).0, ["1,uno       ", "2,two       ", "4,four      "]);

    // A standby of the group: the first log, then the new primary's after it.
    let next = following();
    Standby::replay(&next, &mut log.read_from(Lsn(0))).unwrap();
    let after = log.end().0;
    let moved = own.read_from(Lsn(0)).map(|read| read.map(|(at, bytes)| (Lsn(at.0 + after), bytes)));
    Standby::replay(&next, &mut moved.collect::<Vec<_>>().into_iter()).unwrap();
    assert_eq!(answers(&next, &["T_V"]), answers(&standby, &["T_V"]));
    let last = Arc::new(MemoryLog::new());
    next.promote(Arc::clone(&last) as Arc<dyn Log>).unwrap();
    assert_eq!(last.end(), Lsn(1), "nothing left open to end");
}

/// Verifies: [DESIGN] RULES.md D57, D25, the standby seam: deposed, a
/// database writes nothing more to the log it served over, which its
/// replica may serve again in a later view, for the database opened
/// again: a writer's rollback, and a writer's commit refused (SQLCODE
/// -913), leave the log as they found it. Regression test (R-TEST-7): the
/// rollback of a deposed database's transaction wrote its abort record.
#[test]
fn a_deposed_database_writes_nothing_to_its_log() {
    let own = Arc::new(MemoryLog::new());
    let db = following();
    db.promote(Arc::clone(&own) as Arc<dyn Log>).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER)").unwrap();
    let (rolled_back, refused) = (db.session("APP"), db.session("APP"));
    exec(&rolled_back, "INSERT INTO T VALUES (1)").unwrap();
    exec(&refused, "INSERT INTO T VALUES (2)").unwrap();
    let end = own.end();
    db.depose().unwrap();
    rolled_back.rollback().unwrap();
    assert_eq!(refused.commit().err().map(|e| e.code), Some(Code::CommitFailed));
    assert_eq!(own.end(), end, "nothing written once deposed");
}
