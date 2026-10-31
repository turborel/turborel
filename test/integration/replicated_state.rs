//! Replicated state (RULES.md D57): all of a database's durable state is
//! in its log, its catalog, or the durable files its file manager knows.
//! Every kind of log record, listed in `record::KINDS`, is written by a
//! statement, and recovery, which is the database's replay of its log,
//! makes again from it what the statement did: a database crashed after
//! the statement and recovered holds the same rows, index entries and
//! Information Schema as before the crash. Every durable file the file
//! manager registers is in the database's export manifest, or carried by
//! the log or the catalog.

use std::path::{Path, PathBuf};

use turborel::db::{Database, OpenOptions, Opened};
use turborel::error::Code;
use turborel::files::FileOwner;
use turborel::index::BTreeMethod;
use turborel::log::record::{KINDS, Record};
use turborel::log::{FileLog, FileLogOptions, Log, Lsn};
use turborel::value::types::TypeRegistry;

use crate::common::{Scratch, after_crash, crc32c, exec, number, os, rows};
use crate::file_manager::keeping;
use crate::table_settings::{Calls, Noted};

/// The Information Schema's tables that show what the database is doing
/// now, not what it holds, so differ between two runs of it.
const RUNNING: [&str; 3] = ["FILES", "JOBS", "MEMORY_RESERVATIONS"];

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

/// What the database holds, as its users see it: each Information Schema
/// table's rows but those of `RUNNING`, each base table's rows, and each
/// index's entries, through a session of each of `READERS`, sorted.
fn state(db: &Database) -> Vec<String> {
    let mut found = Vec::new();
    for user in READERS {
        let s = db.session(user);
        let tables = rows(&s, "SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_TYPE <> 'VIEW'").unwrap();
        for table in tables {
            let (schema, name) = table.split_once(',').unwrap();
            if schema == "INFORMATION_SCHEMA" && RUNNING.contains(&name) {
                continue;
            }
            let mut read = rows(&s, &format!("SELECT * FROM {schema}.{name}")).unwrap();
            read.sort();
            found.extend(read.into_iter().map(|row| format!("{user} {schema}.{name}: {row}")));
        }
        let indexes = rows(&s, "SELECT INDEX_NAME FROM INFORMATION_SCHEMA.INDEXES").unwrap();
        found.extend(indexes.iter().map(|ix| format!("{user} {ix}: {} entries", db.index_entries(ix.trim()).unwrap())));
        s.commit().unwrap();
    }
    found
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
