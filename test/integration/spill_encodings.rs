//! The encodings spill areas' files pass through (RULES.md D38, D48):
//! those SPILL.ENCODINGS names, as a compression, then an encryption while
//! the database encrypts anything; and a transaction's undo entries,
//! spilled before a table was encrypted, spilled encrypted from then on.

use std::path::Path;

use turborel::db::Database;
use turborel::disk::encoding::{Encoding, EncodingId};
use turborel::error::Code;
use turborel::value::{Decimal, Value};

use crate::common::{RunLength, Scramble, Scratch, exec, os, rows};

/// The run-length encoding's id, and the scrambling one's (`common.rs`).
const RUN_LENGTH: u32 = 1001;
const SCRAMBLE: u32 = 1002;
/// Where a pager file's header lists its encodings: their count, then
/// their ids (`disk/pager/format.rs`).
const ENCODINGS_AT: usize = 24;

/// An encoding that keeps its input as it is, of id `0` and name `1`.
struct Same(u32, &'static str);

impl Encoding for Same {
    fn id(&self) -> EncodingId {
        EncodingId(self.0)
    }
    fn name(&self) -> &'static str {
        self.1
    }
    fn overhead(&self, _: &str) -> usize {
        0
    }
    fn encode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> turborel::error::Result<()> {
        out.extend_from_slice(input);
        Ok(())
    }
    fn decode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> turborel::error::Result<()> {
        out.extend_from_slice(input);
        Ok(())
    }
}

/// A database made in `dir`, the test encodings registered, of a table
/// P of 6,000 rows of padded strings, its work areas 64 KiB.
fn database(dir: &Path) -> Database {
    let db = Database::new();
    db.register_encoding(Box::new(RunLength)).unwrap();
    db.register_encoding(Box::new(Scramble)).unwrap();
    for (id, name) in [(1010, "test same a"), (1011, "test same b"), (1012, "test same c")] {
        db.register_encoding(Box::new(Same(id, name))).unwrap();
    }
    db.open_files(&os(), dir, &turborel::db::OpenOptions::default()).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE P (K INTEGER, S CHARACTER(60))").unwrap();
    let s = db.session("S");
    let values: Vec<String> = (0..6000).map(|k| format!("({k}, 'plain {k:05}')")).collect();
    for chunk in values.chunks(200) {
        exec(&s, &format!("INSERT INTO P VALUES {}", chunk.join(", "))).unwrap();
    }
    exec(&s, "COMMIT WORK").unwrap();
    db.set_setting("WORK_MEMORY", &Value::Exact(Decimal::new(64 << 10, 0))).unwrap();
    db
}

/// The encodings each spill file in `dir`'s `tmp` names, by id, and the
/// bytes of them all.
fn spill_files(dir: &Path) -> (Vec<Vec<u32>>, usize) {
    let (mut named, mut bytes) = (Vec::new(), 0_usize);
    for entry in std::fs::read_dir(dir.join("tmp")).unwrap() {
        let path = entry.unwrap().path();
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("spill")) {
            continue;
        }
        let file = std::fs::read(&path).unwrap();
        let word = |at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap());
        let count = usize::try_from(word(ENCODINGS_AT)).unwrap();
        named.push((0..count).map(|n| word(ENCODINGS_AT + 4 + 4 * n)).collect());
        bytes += file.len();
    }
    named.sort();
    (named, bytes)
}

/// The spill files of `dir` while a sort of P's strings has given its
/// first row, the sort still open.
fn sorting(db: &Database, dir: &Path) -> (Vec<Vec<u32>>, usize) {
    let s = db.session("S");
    let mut c = s.open_cursor("SELECT S FROM P ORDER BY S").unwrap();
    assert!(c.fetch().unwrap().is_some());
    spill_files(dir)
}

/// Verifies: [EXTENSION] RULES.md D36, D38, D48: a sort's spill files pass
/// through no encoding by default; through those SPILL.ENCODINGS names,
/// here a run-length compression, from the next statement on, fewer bytes
/// then; through those and an encryption once a table is encrypted, and
/// through an encryption named alone, not twice; and through the
/// encryption alone when the two would be more than a pipeline holds.
/// Its rows are read back through them. A name not registered is refused
/// (SQLCODE -910).
#[test]
fn spills_pass_through_the_encodings_named() {
    let scratch = Scratch::new("spill-encodings");
    let dir = scratch.path("db");
    let db = database(&dir);
    let (plain, plain_bytes) = sorting(&db, &dir);
    assert_eq!(plain, vec![Vec::<u32>::new()]);
    let set = |names: &str| db.set_setting("SPILL.ENCODINGS", &Value::Char(names.to_owned()));
    set("test run-length").unwrap();
    let (compressed, compressed_bytes) = sorting(&db, &dir);
    assert_eq!(compressed, vec![vec![RUN_LENGTH]]);
    assert!(compressed_bytes < plain_bytes, "{compressed_bytes} of {plain_bytes}");
    assert_eq!(set("no such encoding").err().map(|e| e.code), Some(Code::Api));

    db.schema("CREATE SCHEMA AUTHORIZATION E CREATE TABLE T (K INTEGER) WITH (ENCODINGS = 'test scramble')").unwrap();
    assert_eq!(sorting(&db, &dir).0, vec![vec![RUN_LENGTH, SCRAMBLE]]);
    set("test scramble").unwrap();
    assert_eq!(sorting(&db, &dir).0, vec![vec![SCRAMBLE]], "encrypted once");
    set("test run-length, test same a, test same b, test same c").unwrap();
    assert_eq!(sorting(&db, &dir).0, vec![vec![SCRAMBLE]], "the encryption alone");
    let s = db.session("S");
    let sorted = rows(&s, "SELECT S FROM P ORDER BY S").unwrap();
    assert_eq!((sorted.len(), sorted[0].trim_end(), sorted[5999].trim_end()), (6000, "plain 00000", "plain 05999"));
}

/// Verifies: [EXTENSION] RULES.md D38, D48: a transaction whose changes
/// to a table spilled while nothing was encrypted, past the table's
/// TRANSACTIONS.HELD_BYTES, spills those of its statements after another
/// table is encrypted through the encryption, to a new area, the runs
/// written before staying as they were; rolled back, it undoes them all.
#[test]
fn a_transactions_changes_spill_encrypted_once_a_table_is() {
    let scratch = Scratch::new("spill-encodings-changes");
    let dir = scratch.path("db");
    let db = database(&dir);
    db.schema("CREATE SCHEMA AUTHORIZATION Q CREATE TABLE Q (K INTEGER, S CHARACTER(40)) WITH (TRANSACTIONS.HELD_BYTES = 4096)").unwrap();
    let q = db.session("Q");
    let values: Vec<String> = (0..3000).map(|k| format!("({k}, 'v{k}')")).collect();
    for chunk in values.chunks(500) {
        exec(&q, &format!("INSERT INTO Q VALUES {}", chunk.join(", "))).unwrap();
    }
    exec(&q, "COMMIT WORK").unwrap();
    exec(&q, "UPDATE Q SET S = 'changed' WHERE K < 1500").unwrap();
    assert_eq!(spill_files(&dir).0, vec![Vec::<u32>::new()], "spilled as they are");
    db.schema("CREATE SCHEMA AUTHORIZATION E CREATE TABLE T (K INTEGER) WITH (ENCODINGS = 'test scramble')").unwrap();
    exec(&q, "UPDATE Q SET S = 'again' WHERE K >= 1500").unwrap();
    assert_eq!(spill_files(&dir).0, vec![vec![], vec![SCRAMBLE]], "a new area, encrypted");
    exec(&q, "ROLLBACK WORK").unwrap();
    assert_eq!(rows(&q, "SELECT COUNT(*) FROM Q WHERE S = 'changed' OR S = 'again'").unwrap(), ["0".to_owned()]);
    assert_eq!(rows(&q, "SELECT COUNT(*) FROM Q WHERE S = 'v2999'").unwrap(), ["1".to_owned()]);
}
