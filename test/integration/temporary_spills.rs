//! Temporary tables larger than the memory budget (RULES.md D49): each
//! session's instance keeps its rows, index entries and the older versions
//! its transactions keep in a buffer pool of
//! MEMORY.WORK_BYTES over a spill file of the database's `tmp/`, written
//! through the encodings spill files take (D38, D48), removed as the
//! instance is emptied or its session ends, and cleared after a crash.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;
use turborel::params::Params;

use crate::common::{Scramble, Scratch, after_crash, bytes_of, exec, holds, number, rows, small_pool};

/// The rows each instance is filled with: about 4 MB of strings, twice
/// the budget.
const ROWS: u32 = 4_000;
/// MEMORY.BUDGET_BYTES of the database: 2 MiB.
const BUDGET: i128 = 2 << 20;

/// The database in `dir`, opened as a crashed one is (`after_crash`), the
/// test scrambling registered: a budget of 2 MiB, its data file's pool
/// 256 KiB, work areas 128 KiB; if `define`, schema S defined, of T, ON
/// COMMIT PRESERVE ROWS, keyed by K, a hash index on V; and G, ON COMMIT
/// DELETE ROWS.
fn open(dir: &Path, define: bool) -> Database {
    let db = Database::new();
    db.register_encoding(Box::new(Scramble)).unwrap();
    db.set_setting("MEMORY.BUDGET_BYTES", &number(BUDGET)).unwrap();
    small_pool(&db, 32);
    db.set_setting("MEMORY.WORK_BYTES", &number(128_i128 << 10_i32)).unwrap();
    db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    if define {
        db.schema(
            "CREATE SCHEMA AUTHORIZATION S
               CREATE GLOBAL TEMPORARY TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER, S CHARACTER(1000)) ON COMMIT PRESERVE ROWS
               CREATE GLOBAL TEMPORARY TABLE G (K INTEGER, S CHARACTER(1000))",
        )
        .unwrap();
        db.create_index("T_V", "S.T", &["V"], "hash").unwrap();
    }
    db
}

/// The S of row K = `k`: a string near 1,000 characters, of the text
/// `plain` and `k`.
fn text(k: u32) -> String {
    format!("plain {k:05} ").repeat(80)
}

/// Inserts `ROWS` rows into `table` in `s`'s transaction, K from 0 and,
/// for T, V of K mod 50.
fn fill(s: &Session, table: &str) {
    let values: Vec<String> = (0..ROWS)
        .map(|k| if table == "T" { format!("({k}, {}, '{}')", k % 50, text(k)) } else { format!("({k}, '{}')", text(k)) })
        .collect();
    for chunk in values.chunks(200) {
        exec(s, &format!("INSERT INTO {table} VALUES {}", chunk.join(", "))).unwrap();
    }
}

/// The spill files in `dir`'s `tmp/`, and their bytes in all.
fn spill_files(dir: &Path) -> (usize, u64) {
    let (mut files, mut bytes) = (0_usize, 0_u64);
    for entry in std::fs::read_dir(dir.join("tmp")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("spill")) {
            files += 1;
            bytes += std::fs::metadata(&path).unwrap().len();
        }
    }
    (files, bytes)
}

/// The S of `s`'s row of `table` whose K is `k`, if one, without its
/// padding.
fn s_of(s: &Session, table: &str, k: u32) -> Vec<String> {
    rows(s, &format!("SELECT S FROM {table} WHERE K = {k}")).unwrap().into_iter().map(|v| v.trim_end().to_owned()).collect()
}

/// Its one row's one value of `sql`.
fn one(s: &Session, sql: &str) -> String {
    rows(s, sql).unwrap().remove(0)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.9, §14.3 GR3]; [DESIGN]
/// RULES.md D38, D49: an instance of a temporary table twice as large as
/// MEMORY.BUDGET_BYTES is written, read through its UNIQUE index and a
/// hash index, updated and deleted from, its pool of MEMORY.WORK_BYTES
/// what it holds of the budget, the rest in a spill file larger than the
/// budget; UNIQUE still refuses a duplicate (SQLCODE -201). ON COMMIT DELETE ROWS empties an instance as large and
/// removes its file; another session's instance is apart; the session's
/// end removes every file of its instances. A session whose own
/// MEMORY.WORK_BYTES the budget cannot grant is refused (SQLCODE -918),
/// the file made for it removed.
#[test]
fn an_instance_larger_than_the_budget_spills_to_a_file() {
    let scratch = Scratch::new("temporary-spills");
    let dir = scratch.path("db");
    let db = open(&dir, true);
    let greedy = db.session("S");
    exec(&greedy, "SET MEMORY.WORK_BYTES = 4194304").unwrap();
    assert_eq!(exec(&greedy, "INSERT INTO T VALUES (1, 1, 'x')").err(), Some(Code::Resources), "its own setting: past the budget");
    drop(greedy);
    assert_eq!(spill_files(&dir).0, 0, "the file of the pool refused removed");
    let s = db.session("S");
    fill(&s, "T");
    exec(&s, "COMMIT WORK").unwrap();
    let (files, bytes) = spill_files(&dir);
    assert_eq!(files, 1, "one file, of T's rows and entries");
    assert!(bytes > u64::try_from(BUDGET).unwrap(), "past the budget: {bytes}");
    let (limit, reserved) = db.memory().unwrap();
    assert!(reserved <= limit, "{reserved} of {limit}");

    assert_eq!(one(&s, "SELECT COUNT(*) FROM T"), "4000");
    assert_eq!(s_of(&s, "T", 3210), [text(3210).trim_end()]);
    assert!(s.physical_plan("SELECT S FROM T WHERE V = 7", &Params::new()).unwrap().contains("T_V"));
    assert_eq!(one(&s, "SELECT COUNT(*) FROM T WHERE V = 7"), "80");
    exec(&s, "UPDATE T SET S = 'changed', V = 99 WHERE K < 2000").unwrap();
    exec(&s, "DELETE FROM T WHERE K >= 3000").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    assert_eq!(one(&s, "SELECT COUNT(*) FROM T"), "3000");
    assert_eq!(one(&s, "SELECT COUNT(*) FROM T WHERE V = 99"), "2000", "the hash index, updated");
    assert_eq!(s_of(&s, "T", 2500), [text(2500).trim_end()]);
    assert_eq!(rows(&s, "SELECT V FROM T WHERE K = 3500").unwrap(), Vec::<String>::new(), "deleted");
    assert_eq!(exec(&s, "INSERT INTO T VALUES (1, 1, 'again')").err(), Some(Code::Constraint));

    fill(&s, "G");
    assert_eq!(one(&s, "SELECT COUNT(*) FROM G"), "4000");
    assert_eq!(spill_files(&dir).0, 2, "G's file beside T's");
    exec(&s, "COMMIT WORK").unwrap();
    assert_eq!(spill_files(&dir).0, 1, "G's file removed");
    assert_eq!(one(&s, "SELECT COUNT(*) FROM G"), "0", "deleted at commit");
    assert_eq!(one(&s, "SELECT COUNT(*) FROM T"), "3000", "preserved");
    let other = db.session("S");
    assert_eq!(one(&other, "SELECT COUNT(*) FROM T"), "0", "another session's instance");
    exec(&other, "COMMIT WORK").unwrap();
    drop(other);
    assert_eq!(spill_files(&dir).0, 2, "the other session's end removes its own");
    drop(s);
    assert_eq!(spill_files(&dir).0, 0, "the session's end removes its own");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.9]; [DESIGN] RULES.md D49,
/// D56: a crash leaves the spill files of a session's instances, which
/// the database removes as it opens again; the rows are never recovered.
#[test]
fn a_crash_leaves_no_instance_behind() {
    let scratch = Scratch::new("temporary-spills-crash");
    let dir = scratch.path("db");
    let crashed = open(&dir, true);
    let s = crashed.session("S");
    fill(&s, "T");
    exec(&s, "COMMIT WORK").unwrap();
    assert_eq!(spill_files(&dir).0, 1);

    let db = open(&dir, false);
    assert_eq!(spill_files(&dir).0, 0, "cleared as it opened");
    assert_eq!(one(&db.session("S"), "SELECT COUNT(*) FROM T"), "0", "not recovered");
    drop((s, crashed));
}

/// Verifies: [EXTENSION] RULES.md D48, D49: an instance's file is written
/// as it is while nothing is encrypted, and through an encryption once a
/// table is, so a temporary table's rows never reach the disk in the
/// clear; they are read back through it.
#[test]
fn an_instance_spills_encrypted_while_anything_is() {
    let scratch = Scratch::new("temporary-spills-encrypted");
    let dir = scratch.path("db");
    let db = open(&dir, true);
    let s = db.session("S");
    fill(&s, "G");
    assert!(holds(&bytes_of(&dir.join("tmp"), ""), "plain 0"), "nothing encrypted: as it is");
    exec(&s, "COMMIT WORK").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION E CREATE TABLE P (K INTEGER) WITH (ENCODINGS = 'test scramble')").unwrap();
    fill(&s, "G");
    assert!(!holds(&bytes_of(&dir.join("tmp"), ""), "plain 0"), "a table encrypted: so is the instance");
    assert_eq!(s_of(&s, "G", 3999), [text(3999).trim_end()]);
    assert_eq!(one(&s, "SELECT COUNT(*) FROM G"), "4000");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.9, §14.3]; [DESIGN] RULES.md
/// D38, D49: an UPDATE, then a DELETE, of every row of an instance twice
/// as large as MEMORY.BUDGET_BYTES keep the older versions in the
/// instance's file, which grows by about the instance, not in memory;
/// each rolled back restores every row, and a committed UPDATE leaves
/// them changed. The session's end removes the file.
/// Regression test (R-TEST-7): the older versions went to a log in
/// memory, outside the budget, until the transaction ended, so the file
/// did not grow.
#[test]
fn older_versions_of_an_instance_spill_to_its_file() {
    let scratch = Scratch::new("temporary-spills-undo");
    let dir = scratch.path("db");
    let db = open(&dir, true);
    let s = db.session("S");
    fill(&s, "T");
    exec(&s, "COMMIT WORK").unwrap();
    let before = spill_files(&dir).1;
    exec(&s, "UPDATE T SET S = 'changed'").unwrap();
    let grown = spill_files(&dir).1;
    assert!(grown >= before + u64::try_from(BUDGET).unwrap(), "older versions in the file: {before} to {grown}");
    assert_eq!(one(&s, "SELECT COUNT(*) FROM T WHERE S = 'changed'"), "4000");
    exec(&s, "ROLLBACK WORK").unwrap();
    assert_eq!(s_of(&s, "T", 3210), [text(3210).trim_end()], "restored");
    assert_eq!(one(&s, "SELECT COUNT(*) FROM T WHERE S = 'changed'"), "0");

    exec(&s, "DELETE FROM T").unwrap();
    assert_eq!(one(&s, "SELECT COUNT(*) FROM T"), "0");
    exec(&s, "ROLLBACK WORK").unwrap();
    assert_eq!(one(&s, "SELECT COUNT(*) FROM T"), "4000", "restored");
    assert_eq!(s_of(&s, "T", 17), [text(17).trim_end()]);
    assert_eq!(one(&s, "SELECT COUNT(*) FROM T WHERE V = 7"), "80", "the hash index, restored");

    exec(&s, "UPDATE T SET S = 'changed'").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    assert_eq!(one(&s, "SELECT COUNT(*) FROM T WHERE S = 'changed'"), "4000");
    let (limit, reserved) = db.memory().unwrap();
    assert!(reserved <= limit, "{reserved} of {limit}");
    drop(s);
    assert_eq!(spill_files(&dir).0, 0, "the session's end removes its file");
}
