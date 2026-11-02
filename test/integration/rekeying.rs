//! A database's files written in other formats from now on (RULES.md
//! D48): STORAGE.ENCODINGS and STORAGE.CHECKSUM_KEY changed at once, the
//! data file's pages, the log's next segment and the catalog's next file
//! taking them, what was written before read as it was.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Opened};
use turborel::error::{Code, Result, SqlError};
use turborel::hash::crc::{CRC32_ISCSI, Crc};
use turborel::hash::{Digest, HashFunction, HashId, MAX_KEY_NAME};
use turborel::value::Value;

use crate::common::{RunLength, Scramble, Scratch, after_crash, rows};

/// A keyed hash function, near enough: CRC-32 of the key's name and the
/// bytes, keyed by any name beginning with `k`, as a keyring holding
/// those would.
struct Keyed {
    name: String,
    crc: Crc,
}

impl Keyed {
    fn by(name: &str) -> Box<dyn HashFunction> {
        Box::new(Keyed { name: name.to_owned(), crc: Crc::new(CRC32_ISCSI).unwrap() })
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
        if name.starts_with('k') {
            Ok(Arc::from(Keyed::by(name)))
        } else {
            Err(SqlError::new(Code::Api, format!("the keyring has no key {name}")))
        }
    }
}

fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

/// A database with the keyed function and the test encodings registered,
/// checksummed with the function keyed by `k1`.
fn prepared(encodings: bool) -> Database {
    let db = Database::new();
    db.register_hash_function(Keyed::by("k1")).unwrap();
    if encodings {
        db.register_encoding(Box::new(RunLength)).unwrap();
        db.register_encoding(Box::new(Scramble)).unwrap();
    }
    db.set_setting("STORAGE.CHECKSUM", &text("TEST-KEYED")).unwrap();
    db
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
/// encodings its files name.
#[test]
fn files_follow_the_formats_set_at_once() {
    let scratch = Scratch::new("rekeying");
    let dir = scratch.path("db");
    let db = prepared(true);
    db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
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

    db.set_setting("STORAGE.ENCODINGS", &text("test run-length,test scramble")).unwrap();
    db.set_setting("STORAGE.CHECKSUM_KEY", &text("k2")).unwrap();
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
    drop((s, db));

    assert_eq!(prepared(false).open_files(&after_crash(), &dir, &OpenOptions::default()).err().map(|e| e.code), Some(Code::Api));
    let again = prepared(true);
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
    again.close().unwrap();
    assert!(!any_holds(&dir, b"secret text"), "the formats went on after the crash");
    let clean = prepared(true);
    assert!(matches!(clean.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap(), Opened::Reopened));
    assert_eq!(rows(&clean.session("A"), "SELECT COUNT(*) FROM T"), Ok(vec!["601".into()]));
}

/// Verifies: [DESIGN] RULES.md D36, D48: STORAGE.CHECKSUM_KEY is refused
/// (SQLCODE -910) where STORAGE.CHECKSUM is not a keyed function, where
/// it names a key the function cannot take, or a name too long for a
/// file's field, the value kept as it was; a key the function takes is
/// set, and the empty name is the key the function was registered with. A
/// database kept in memory takes both settings, having no file to tell.
#[test]
fn a_key_the_checksum_cannot_take_is_refused() {
    let plain = Database::new();
    assert_eq!(plain.set_setting("STORAGE.CHECKSUM_KEY", &text("k1")).err().map(|e| e.code), Some(Code::Api), "CRC is not keyed");
    assert_eq!(plain.setting("STORAGE.CHECKSUM_KEY").unwrap().to_string(), "");
    let db = prepared(true);
    assert_eq!(db.set_setting("STORAGE.CHECKSUM_KEY", &text("x9")).err().map(|e| e.code), Some(Code::Api), "no such key");
    assert_eq!(db.set_setting("STORAGE.CHECKSUM_KEY", &text(&"k".repeat(MAX_KEY_NAME + 1))).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(db.setting("STORAGE.CHECKSUM_KEY").unwrap().to_string(), "");
    db.set_setting("STORAGE.CHECKSUM_KEY", &text("k3")).unwrap();
    assert_eq!(db.setting("STORAGE.CHECKSUM_KEY").unwrap().to_string(), "k3");
    db.set_setting("STORAGE.CHECKSUM_KEY", &text("")).unwrap();
    db.set_setting("STORAGE.ENCODINGS", &text("TEST SCRAMBLE")).unwrap();
    assert_eq!(db.setting("STORAGE.ENCODINGS").unwrap().to_string(), "test scramble");
}
