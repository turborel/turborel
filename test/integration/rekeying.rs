//! A database's files written in other formats from now on (RULES.md
//! D48): STORAGE.ENCODINGS and STORAGE.CHECKSUM_KEY changed at once, the
//! data file's pages, the log's next segment and the catalog's next file
//! taking them, what was written before read as it was until the sweep
//! `turborel-rekey` writes it again, and the files settled then.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Opened};
use turborel::error::{Code, Result, SqlError};
use turborel::hash::crc::{CRC32_ISCSI, Crc};
use turborel::hash::{Digest, HashFunction, HashId, MAX_KEY_NAME};
use turborel::value::Value;

use crate::common::{Kept, RunLength, Scramble, Scratch, after_crash, number, rows};

/// A keyed hash function, near enough: CRC-32 of the key's name and the
/// bytes, keyed by any name beginning with `k`, as a keyring holding
/// those would, or by the keys of a keyring given.
struct Keyed {
    name: String,
    held: Option<Vec<String>>,
    crc: Crc,
}

impl Keyed {
    fn by(name: &str) -> Box<dyn HashFunction> {
        Box::new(Keyed { name: name.to_owned(), held: None, crc: Crc::new(CRC32_ISCSI).unwrap() })
    }

    /// Keyed by `name`, of a keyring holding the keys `held` alone.
    fn holding(name: &str, held: &[&str]) -> Box<dyn HashFunction> {
        let held = Some(held.iter().map(|&k| k.to_owned()).collect());
        Box::new(Keyed { name: name.to_owned(), held, crc: Crc::new(CRC32_ISCSI).unwrap() })
    }

    /// Whether the keyring holds `name`.
    fn holds(&self, name: &str) -> bool {
        self.held.as_ref().map_or_else(|| name.starts_with('k'), |held| held.iter().any(|k| k == name))
    }
}

impl HashFunction for Keyed {
    fn id(&self) -> HashId {
        HashId(1001)
    }
    fn name(&self) -> &'static str {
        "TEST-KEYED"
    }
    fn digest_len(&self) -> usize {
        4
    }
    fn hash_parts(&self, parts: &[&[u8]]) -> Digest {
        let mut all = self.name.as_bytes().to_vec();
        for part in parts {
            all.extend_from_slice(part);
        }
        self.crc.hash(&all)
    }
    fn keyed(&self) -> bool {
        true
    }
    fn key_name(&self) -> Option<&str> {
        Some(&self.name)
    }
    fn keyed_by(&self, name: &str) -> Result<Arc<dyn HashFunction>> {
        if !self.holds(name) {
            return Err(SqlError::new(Code::Api, format!("the keyring has no key {name}")));
        }
        Ok(Arc::new(Keyed { name: name.to_owned(), held: self.held.clone(), crc: Crc::new(CRC32_ISCSI).unwrap() }))
    }
}

fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

/// A database with the keyed function and the test encodings registered,
/// checksummed with the function keyed by `k1`, its background work left
/// to the test (JOBS.POLICY MANUAL), its events kept.
fn prepared(encodings: bool) -> (Database, Kept) {
    let db = Database::new();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone())).unwrap();
    db.register_hash_function(Keyed::by("k1")).unwrap();
    if encodings {
        db.register_encoding(Box::new(RunLength)).unwrap();
        db.register_encoding(Box::new(Scramble)).unwrap();
    }
    db.set_setting("STORAGE.CHECKSUM", &text("TEST-KEYED")).unwrap();
    db.set_setting("JOBS.POLICY", &text("MANUAL")).unwrap();
    (db, kept)
}

/// INFORMATION_SCHEMA.FILE_FORMATS of `db`, a row a string.
fn file_formats(db: &Database) -> Vec<String> {
    let mut rows = Vec::new();
    let s = db.session("A");
    let mut c = s.open_cursor("SELECT KIND, IS_SETTLED, BEHIND FROM INFORMATION_SCHEMA.FILE_FORMATS ORDER BY KIND").unwrap();
    while let Some(r) = c.fetch().unwrap() {
        rows.push(r.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join("|"));
    }
    rows
}

/// The events `kept` holds of the files settled.
fn settled_events(kept: &Kept) -> Vec<String> {
    kept.taken().into_iter().map(|(_, e)| e).filter(|e| e.contains("FormatsSettled")).collect()
}

/// The field a file names a key in: the name's length, the name, zeros
/// to the field's end.
fn key_field(name: &str) -> Vec<u8> {
    let mut field = vec![u8::try_from(name.len()).unwrap()];
    field.extend_from_slice(name.as_bytes());
    field.resize(MAX_KEY_NAME + 1, 0);
    field
}

/// Whether `needle` occurs in the file at `path`.
fn holds(path: &Path, needle: &[u8]) -> bool {
    std::fs::read(path).unwrap().windows(needle.len()).any(|w| w == needle)
}

/// Whether the file at `path` names the key `name` somewhere.
fn names_key(path: &Path, name: &str) -> bool {
    holds(path, &key_field(name))
}

/// The files of `dir`, sorted by name, and those of its directories.
fn files_of(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_of(&path));
        } else {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// Whether any file of `dir` holds `needle`.
fn any_holds(dir: &Path, needle: &[u8]) -> bool {
    files_of(dir).iter().any(|path| holds(path, needle))
}

/// Verifies: [DESIGN] RULES.md D36, D48: STORAGE.ENCODINGS and
/// STORAGE.CHECKSUM_KEY change at once on a database kept in a directory:
/// the log begins a segment in the new formats at its next append, the
/// catalog writes its next record to a new file in them, and the data
/// file's pages written from then on pass through the new encodings,
/// checksummed by the new key, while pages written before stay as they
/// were, read still; so rows written after the change are in no file as
/// written. The database recovered after a crash, and opened again after
/// a clean close, reads every row, its settings restored, its files going
/// on in the new formats; and is refused (SQLCODE -910) without the
/// encodings its files name. INFORMATION_SCHEMA.FILE_FORMATS shows the
/// data file behind by its pages, the log by its segments; the sweep
/// (`Database::rekey`) writes the pages behind again and settles the
/// files, told once as the checkpoint after it leaves no file behind,
/// and the owner is told `A` of it; the database then opens, and reads
/// every row, with a keyring holding the new key alone, and settling
/// again does nothing.
#[test]
fn files_follow_the_formats_set_at_once() {
    let scratch = Scratch::new("rekeying");
    let dir = scratch.path("db");
    let (db, kept) = prepared(true);
    db.set_setting("STORAGE.REKEY_BYTES_PER_SECOND", &number(65_536)).unwrap();
    db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    db.create_user("A", b"x").unwrap();
    db.set_owner("A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, S CHAR(200))").unwrap();
    let s = db.session("A");
    for k in 0..300 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'plain text {k}')")).unwrap();
    }
    s.commit().unwrap();
    db.checkpoint().unwrap();
    let (data, segments) = (dir.join("data"), dir.join("log"));
    assert!(names_key(&data, "k1") && !names_key(&data, "k2"));
    assert_eq!(files_of(&segments).len(), 1, "{:?}", files_of(&segments));
    assert!(names_key(&files_of(&segments)[0], "k1"));

    assert_eq!(file_formats(&db), ["CATALOG|YES|0", "DATA|YES|0", "LOG|YES|0"]);
    db.set_setting("STORAGE.ENCODINGS", &text("test run-length,test scramble")).unwrap();
    db.set_setting("STORAGE.CHECKSUM_KEY", &text("k2")).unwrap();
    let pages = db.jobs().iter().find(|j| j.label == "turborel-rekey").map(|j| j.state.word().to_owned());
    assert_eq!(pages.as_deref(), Some("DUE"), "asked for as the formats changed");
    for k in 300..600 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'secret text {k}')")).unwrap();
    }
    s.commit().unwrap();
    // The log began a segment before the first record appended after
    // each change; the catalog's next record, the checkpoint's image,
    // went to a new file, the one before it removed.
    let after = files_of(&segments);
    assert_eq!(after.len(), 3, "{after:?}");
    assert!(names_key(&after[0], "k1") && !names_key(&after[0], "k2"), "{:?}", after[0]);
    assert!(names_key(&after[1], "k1") && !names_key(&after[1], "k2"), "{:?}", after[1]);
    assert!(names_key(&after[2], "k2") && !names_key(&after[2], "k1"), "{:?}", after[2]);
    db.checkpoint().unwrap();
    let catalogs: Vec<PathBuf> = files_of(&dir).into_iter().filter(|p| p.to_string_lossy().contains("catalog.")).collect();
    assert_eq!(catalogs.len(), 1, "{catalogs:?}");
    assert!(names_key(&catalogs[0], "k2") && !names_key(&catalogs[0], "k1"), "{:?}", catalogs[0]);
    assert!(names_key(&data, "k1") && names_key(&data, "k2"), "pages before and after the change");
    assert!(holds(&data, b"plain text 0"), "a page written before stays as it was");
    assert!(!any_holds(&dir, b"secret text"), "written through the encodings, in every file");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE S LIKE 'plain%' OR S LIKE 'secret%'"), Ok(vec!["600".into()]));
    let shown = file_formats(&db);
    assert_eq!(shown[0], "CATALOG|YES|0");
    assert_eq!(shown[2], "LOG|YES|0", "the segments before the change truncated by the checkpoint");
    assert!(shown[1].starts_with("DATA|NO|") && shown[1] != "DATA|NO|0", "{shown:?}");
    assert_eq!(settled_events(&kept), Vec::<String>::new());
    drop((s, db));

    assert_eq!(prepared(false).0.open_files(&after_crash(), &dir, &OpenOptions::default()).err().map(|e| e.code), Some(Code::Api));
    let (again, kept) = prepared(true);
    let opened = again.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(again.setting("STORAGE.CHECKSUM_KEY").unwrap().to_string(), "k2");
    assert_eq!(again.setting("STORAGE.ENCODINGS").unwrap().to_string(), "test run-length,test scramble");
    let s = again.session("A");
    assert_eq!(
        rows(&s, "SELECT S FROM T WHERE K IN (7, 407) ORDER BY S"),
        Ok(vec![format!("{:200}", "plain text 7"), format!("{:200}", "secret text 407")])
    );
    s.run("INSERT INTO T VALUES (600, 'secret text 600')").unwrap();
    s.commit().unwrap();
    drop(s);
    assert!(file_formats(&again)[1].starts_with("DATA|NO|"), "the sweep is needed still after the crash");
    again.rekey().unwrap();
    assert_eq!(file_formats(&again), ["CATALOG|YES|0", "DATA|YES|0", "LOG|YES|0"]);
    assert_eq!(settled_events(&kept).len(), 1, "told once");
    again.rekey().unwrap();
    again.checkpoint().unwrap();
    assert_eq!(settled_events(&kept), Vec::<String>::new(), "not told again");
    assert!(!holds(&dir.join("data"), b"plain text"), "every page written again through the encodings");
    again.close().unwrap();
    assert!(!any_holds(&dir, b"secret text"), "the formats went on after the crash");
    let clean = Database::new();
    clean.register_hash_function(Keyed::holding("k2", &["k2"])).unwrap();
    clean.register_encoding(Box::new(RunLength)).unwrap();
    clean.register_encoding(Box::new(Scramble)).unwrap();
    clean.set_setting("STORAGE.CHECKSUM", &text("TEST-KEYED")).unwrap();
    assert!(matches!(clean.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap(), Opened::Reopened), "k1 retired");
    assert_eq!(rows(&clean.session("A"), "SELECT COUNT(*) FROM T"), Ok(vec!["601".into()]));
    assert_eq!(rows(&clean.session("A"), "SELECT S FROM T WHERE K = 7"), Ok(vec![format!("{:200}", "plain text 7")]));
}

/// Verifies: [DESIGN] RULES.md D48, D55: the job `turborel-rekey` looks
/// at STORAGE.REKEY_BYTES_PER_SECOND of the data file a second, a page a
/// run at the least, a run at a time under JOBS.POLICY MANUAL
/// (`run_due`); at 0 it looks at nothing, the files staying behind; the
/// formats changed under it, it starts again from the first page; and
/// under AUTOMATIC it goes on in its own time and settles the files,
/// told once, so the database opens with the last key alone, every page
/// read.
#[test]
fn the_sweep_runs_in_its_own_time() {
    let scratch = Scratch::new("rekeying-job");
    let dir = scratch.path("db");
    let (db, kept) = prepared(true);
    db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    db.create_user("A", b"x").unwrap();
    db.set_owner("A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, S CHAR(200))").unwrap();
    let s = db.session("A");
    for k in 0..300 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'plain text {k}')")).unwrap();
    }
    s.commit().unwrap();
    drop(s);
    db.checkpoint().unwrap();
    db.set_setting("STORAGE.REKEY_BYTES_PER_SECOND", &number(0)).unwrap();
    db.set_setting("STORAGE.CHECKSUM_KEY", &text("k2")).unwrap();
    db.run_due().unwrap();
    let behind = |db: &Database| file_formats(db)[1].trim_start_matches("DATA|NO|").parse::<u64>().unwrap();
    let pages = behind(&db);
    assert!(pages > 8, "paused, every page behind: {:?}", file_formats(&db));
    // A page a run, each asked for again (as a change of the pace asks
    // while the file is behind): four runs, then the key changes under
    // the walk.
    for n in 1..=4 {
        db.set_setting("STORAGE.REKEY_BYTES_PER_SECOND", &number(8192)).unwrap();
        db.run_due().unwrap();
        assert_eq!(behind(&db), pages - n);
    }
    // The pages written again so far reach the file under k2.
    db.checkpoint().unwrap();
    db.set_setting("STORAGE.CHECKSUM_KEY", &text("k3")).unwrap();
    assert_eq!(behind(&db), pages, "started again");
    db.set_setting("JOBS.POLICY", &text("AUTOMATIC")).unwrap();
    let settled = (0..20_000_u32).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(1));
        file_formats(&db) == ["CATALOG|YES|0", "DATA|YES|0", "LOG|YES|0"]
    });
    assert!(settled, "{:?}", file_formats(&db));
    db.checkpoint().unwrap();
    assert_eq!(settled_events(&kept).len(), 1, "told once");
    db.close().unwrap();
    let last = Database::new();
    last.register_hash_function(Keyed::holding("k3", &["k3"])).unwrap();
    last.register_encoding(Box::new(RunLength)).unwrap();
    last.register_encoding(Box::new(Scramble)).unwrap();
    last.set_setting("STORAGE.CHECKSUM", &text("TEST-KEYED")).unwrap();
    last.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    // Every page read: the rows scanned, and the index descended.
    assert_eq!(rows(&last.session("A"), "SELECT COUNT(*) FROM T WHERE S LIKE 'plain%'"), Ok(vec!["300".into()]));
    assert_eq!(rows(&last.session("A"), "SELECT S FROM T WHERE K = 7"), Ok(vec![format!("{:200}", "plain text 7")]));
}

/// Verifies: [DESIGN] RULES.md D36, D48: of a database opened from its
/// files, STORAGE.CHECKSUM_KEY is refused (SQLCODE -910) where
/// STORAGE.CHECKSUM is not a keyed function, where it names a key the
/// function cannot take, or a name too long for a file's field, the value
/// kept as it was; a key the function takes is set, and the empty name is
/// the key the function was registered with. Given before the database
/// opens, a key the function cannot take refuses the opening instead. A
/// database kept in memory takes both settings, having no file to tell,
/// and nothing to sweep.
#[test]
fn a_key_the_checksum_cannot_take_is_refused() {
    let scratch = Scratch::new("rekeying-refused");
    let plain = Database::new();
    plain.open_files(&after_crash(), &scratch.path("plain"), &OpenOptions::default()).unwrap();
    assert_eq!(plain.set_setting("STORAGE.CHECKSUM_KEY", &text("k1")).err().map(|e| e.code), Some(Code::Api), "CRC is not keyed");
    assert_eq!(plain.setting("STORAGE.CHECKSUM_KEY").unwrap().to_string(), "");
    plain.close().unwrap();
    let early = Database::new();
    early.set_setting("STORAGE.CHECKSUM_KEY", &text("k1")).unwrap();
    let refused = early.open_files(&after_crash(), &scratch.path("plain"), &OpenOptions::default());
    assert_eq!(refused.err().map(|e| e.code), Some(Code::Api), "refused as it opens");
    let (db, _) = prepared(true);
    db.open_files(&after_crash(), &scratch.path("keyed"), &OpenOptions::default()).unwrap();
    assert_eq!(db.set_setting("STORAGE.CHECKSUM_KEY", &text("x9")).err().map(|e| e.code), Some(Code::Api), "no such key");
    assert_eq!(db.set_setting("STORAGE.CHECKSUM_KEY", &text(&"k".repeat(MAX_KEY_NAME + 1))).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(db.setting("STORAGE.CHECKSUM_KEY").unwrap().to_string(), "");
    db.set_setting("STORAGE.CHECKSUM_KEY", &text("k3")).unwrap();
    assert_eq!(db.setting("STORAGE.CHECKSUM_KEY").unwrap().to_string(), "k3");
    db.set_setting("STORAGE.CHECKSUM_KEY", &text("")).unwrap();
    db.close().unwrap();
    let (memory, _) = prepared(true);
    memory.set_setting("STORAGE.CHECKSUM_KEY", &text("k3")).unwrap();
    memory.set_setting("STORAGE.ENCODINGS", &text("TEST SCRAMBLE")).unwrap();
    assert_eq!(memory.setting("STORAGE.ENCODINGS").unwrap().to_string(), "test scramble");
    memory.create_user("A", b"x").unwrap();
    memory.set_owner("A").unwrap();
    memory.rekey().unwrap();
    assert_eq!(file_formats(&memory), Vec::<String>::new(), "no file");
}
