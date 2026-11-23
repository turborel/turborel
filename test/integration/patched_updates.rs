//! \[DESIGN\] A point UPDATE written as a patch of its row's record
//! (RULES.md D915 to D918). A compiled program's UPDATE of one row by its
//! key reads of the row the columns its statement needs alone, and its
//! write makes the new record of the bytes of the one it replaces with
//! the columns set written again. The same UPDATE interpreted runs the
//! general path, which reads the row whole and encodes it whole. Each
//! case here runs both and finds the same rows, the same outcomes, the
//! same index entries and the same log, of a table whose rows are patched
//! and of each table whose rows are not.

use std::sync::Arc;
use std::thread;

use turborel::clock::FixedClock;
use turborel::db::{Database, OpenOptions, Opened, Session};
use turborel::disk::io::FileSystem;
use turborel::storage::{AfterImage, Patch, TableStorage};
use turborel::value::{Decimal, Value};

use crate::common::{MemoryTable, RunLength, Scramble, Scratch, after_crash, bytes_of, holds, os, rows, until_waiting};

/// What makes a table's rows patched or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Rows of the row store, every value in line: patched.
    Plain,
    /// Values past 16 bytes kept out of line: a record holding one, or
    /// that would, is written whole.
    OutOfLine,
    /// Rows sealed through an encoding: written whole.
    Sealed,
    /// A large object column: written whole.
    Lob,
    /// The column store: a row frozen is written whole.
    Columns,
    /// A system-time period, whose stamp the write puts: read whole.
    SystemTime,
    /// A CHECK constraint, which reads a column of the row written that
    /// no UPDATE here reads: read whole.
    Checked,
    /// A generated column of two columns, of which an UPDATE may set one
    /// and read neither, the other a column no index keys: read whole.
    Generated,
}

const KINDS: [Kind; 8] =
    [Kind::Plain, Kind::OutOfLine, Kind::Sealed, Kind::Lob, Kind::Columns, Kind::SystemTime, Kind::Checked, Kind::Generated];

/// T's columns, a column of each kind of type a record holds, the first a
/// key and the last of varying width.
const COLUMNS: &str = "K INTEGER PRIMARY KEY, A INTEGER, B INTEGER, C CHARACTER(12), D DECIMAL(6,2) NOT NULL, V VARCHAR(40), \
    F DOUBLE PRECISION, L BOOLEAN, DT DATE, TM TIME(3), TS TIMESTAMP(6) WITH TIME ZONE, IV INTERVAL DAY TO SECOND, BN BINARY(4), \
    VB VARBINARY(20), AR INTEGER ARRAY[4], TAIL VARCHAR(30)";

/// The columns read back.
const READ: &str = "K, A, B, C, D, V, F, L, DT, TM, TS, IV, BN, VB, AR, TAIL";

/// T's definition for `kind`.
fn schema(kind: Kind) -> String {
    let (more, with) = match kind {
        Kind::Plain => ("", ""),
        Kind::OutOfLine => ("", " WITH (OUT_OF_LINE = 16)"),
        Kind::Sealed => ("", " WITH (ENCODINGS = 'TEST RUN-LENGTH')"),
        Kind::Lob => (", BIG CLOB(1M)", ""),
        Kind::Columns => ("", " USING columnstore"),
        Kind::SystemTime => (
            ", SB TIMESTAMP(6) GENERATED ALWAYS AS ROW START, SE TIMESTAMP(6) GENERATED ALWAYS AS ROW END, PERIOD FOR SYSTEM_TIME (SB, SE)",
            "",
        ),
        Kind::Checked => (", CHECK (D IS NOT NULL)", ""),
        Kind::Generated => (", G DECIMAL(8,2) GENERATED ALWAYS AS (A + D)", ""),
    };
    format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T ({COLUMNS}{more}){with}")
}

/// One point UPDATE each, but PICK's two. A value of -1, of 'none', or a
/// count of 0, sets null. COPYC's SET value reads a column it does not
/// set, that no index keys. PICK reads back one of three columns of the
/// row it wrote, chosen by its argument, and stores it.
const PROCEDURES: [&str; 13] = [
    "CREATE PROCEDURE SETA (IN KK INTEGER, IN X INTEGER) MODIFIES SQL DATA BEGIN UPDATE T SET A = NULLIF(X, -1) WHERE K = KK; END",
    "CREATE PROCEDURE SETC (IN KK INTEGER, IN X CHARACTER(12)) MODIFIES SQL DATA
       BEGIN UPDATE T SET C = NULLIF(X, 'none') WHERE K = KK; END",
    "CREATE PROCEDURE SETV (IN KK INTEGER, IN X VARCHAR(40)) MODIFIES SQL DATA BEGIN UPDATE T SET V = NULLIF(X, 'none') WHERE K = KK; END",
    "CREATE PROCEDURE SETTAIL (IN KK INTEGER, IN X VARCHAR(30)) MODIFIES SQL DATA
       BEGIN UPDATE T SET TAIL = NULLIF(X, 'none') WHERE K = KK; END",
    "CREATE PROCEDURE SETD (IN KK INTEGER, IN X INTEGER) MODIFIES SQL DATA BEGIN UPDATE T SET D = NULLIF(X, -1) WHERE K = KK; END",
    "CREATE PROCEDURE SETNUM (IN KK INTEGER, IN N INTEGER) MODIFIES SQL DATA
       BEGIN UPDATE T SET D = D + N, F = CASE WHEN N > 0 THEN CAST(N AS DOUBLE PRECISION) / 4 END, L = CASE WHEN N > 0 THEN N > 2 END
         WHERE K = KK; END",
    "CREATE PROCEDURE SETTIMES (IN KK INTEGER, IN N INTEGER) MODIFIES SQL DATA
       BEGIN UPDATE T SET DT = CASE WHEN N > 0 THEN DATE '2024-02-29' END, TM = CASE WHEN N > 0 THEN TIME '10:20:30.125' END,
           TS = CASE WHEN N > 0 THEN TIMESTAMP '2024-02-29 10:20:30.000001+02:00' END,
           IV = CASE WHEN N > 0 THEN INTERVAL '1 02:03:04' DAY TO SECOND END
         WHERE K = KK; END",
    "CREATE PROCEDURE SETBYTES (IN KK INTEGER, IN N INTEGER) MODIFIES SQL DATA
       BEGIN UPDATE T SET BN = CASE WHEN N > 0 THEN X'01020304' END, VB = CASE WHEN N > 1 THEN X'AABBCC' WHEN N > 0 THEN X'' END,
           AR = CASE WHEN N > 0 THEN ARRAY[N, N + 1] END
         WHERE K = KK; END",
    "CREATE PROCEDURE SETMANY (IN KK INTEGER, IN X INTEGER, IN Y VARCHAR(40), IN Z VARCHAR(30)) MODIFIES SQL DATA
       BEGIN UPDATE T SET A = X, V = Y, TAIL = Z, B = B + X WHERE K = KK; END",
    "CREATE PROCEDURE REKEY (IN KK INTEGER, IN X INTEGER) MODIFIES SQL DATA BEGIN UPDATE T SET K = X WHERE K = KK; END",
    "CREATE PROCEDURE COPYC (IN KK INTEGER) MODIFIES SQL DATA BEGIN UPDATE T SET TAIL = TRIM(C) WHERE K = KK; END",
    "CREATE PROCEDURE BUMPB (IN KK INTEGER) MODIFIES SQL DATA BEGIN UPDATE T SET B = B + 1 WHERE K = KK; END",
    "CREATE PROCEDURE PICK (IN KK INTEGER, IN N INTEGER) MODIFIES SQL DATA
       BEGIN DECLARE R VARCHAR(40);
         SELECT CASE N WHEN 1 THEN C WHEN 2 THEN V ELSE TAIL END INTO R FROM FINAL TABLE (UPDATE T SET B = B + 1 WHERE K = KK);
         UPDATE T SET TAIL = TRIM(R) WHERE K = KK;
       END",
];

/// Rows of T as the cases begin: row K holds A = K + 100, B = K + 200.
const ROWS: i64 = 12;

/// A database with the test encodings, T of `kind`, its rows, its indexes
/// and `PROCEDURES`, all committed, in `dir` through `fs`, and A's
/// session in it, its procedures compiled or not.
fn database(dir: &std::path::Path, fs: &Arc<dyn FileSystem>, (kind, compiled): (Kind, bool)) -> (Database, Session) {
    let db = encoded();
    // One instant for every commit, so two logs of the same changes are the same bytes.
    db.set_clock(Arc::new(FixedClock::at(1_000_000))).unwrap();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db.schema(&schema(kind)).unwrap();
    db.create_index("T_A", "A.T", &["A"], "btree").unwrap();
    db.create_index("T_B", "A.T", &["B"], "btree").unwrap();
    let s = session(&db, compiled);
    let values: Vec<String> =
        (1..=ROWS).map(|k| format!("({k}, {}, {}, 'row {k}', {k}.25, 'v {k}', 'last {k}')", k + 100, k + 200)).collect();
    s.run(&format!("INSERT INTO T (K, A, B, C, D, V, TAIL) VALUES {}", values.join(", "))).unwrap();
    for procedure in PROCEDURES {
        s.run(procedure).unwrap();
    }
    s.commit().unwrap();
    // The column store freezes what it holds.
    db.maintain().unwrap();
    (db, s)
}

/// A database that knows the test encodings.
fn encoded() -> Database {
    let db = Database::new();
    db.register_encoding(Box::new(RunLength)).unwrap();
    db.register_encoding(Box::new(Scramble)).unwrap();
    db
}

/// A's session in `db`, its procedures compiled or not.
fn session(db: &Database, compiled: bool) -> Session {
    let s = db.session("A");
    s.run(&format!("SET EXECUTOR.COMPILED_PROCEDURES = '{}'", if compiled { "ON" } else { "OFF" })).unwrap();
    s.commit().unwrap();
    s
}

/// The UPDATEs of the cases, a transaction each list. Every column is
/// set, to a value, to null and back: the first column, a key; columns in
/// the middle, of fixed width and of varying; the last column; a varying
/// column grown and shrunk; and several at once. Each procedure is called
/// often enough for its plan to be kept and its change written straight.
fn committed() -> Vec<Vec<String>> {
    let mut all = Vec::new();
    for k in 1..=6_i64 {
        all.push(vec![
            format!("CALL SETA({k}, {})", k + 500),
            format!("CALL SETC({k}, 'changed {k}')"),
            format!("CALL SETV({k}, 'a longer value of row {k}, grown')"),
            format!("CALL SETTAIL({k}, 'z')"),
            format!("CALL SETNUM({k}, {k})"),
            format!("CALL SETTIMES({k}, {})", k % 2),
            format!("CALL SETBYTES({k}, {})", k % 3),
            format!("CALL BUMPB({k})"),
            format!("CALL COPYC({k})"),
        ]);
    }
    for k in 1..=6_i64 {
        all.push(vec![
            format!("CALL SETMANY({k}, {}, 'both {k}', 'the last column of row {k}')", k + 600),
            format!("CALL SETV({k}, 'none')"),
            format!("CALL SETV({k}, 'v')"),
            format!("CALL SETC({k}, 'none')"),
            format!("CALL SETTAIL({k}, 'none')"),
            format!("CALL SETTAIL({k}, 'again {k}')"),
            format!("CALL SETNUM({k}, 0)"),
            format!("CALL SETTIMES({k}, {})", (k + 1) % 2),
            format!("CALL SETBYTES({k}, {})", (k + 1) % 3),
            format!("CALL COPYC({k})"),
            format!("CALL PICK({k}, {})", k % 3 + 1),
            format!("CALL SETA({k}, -1)"),
            format!("CALL SETA({k}, {})", k + 700),
        ]);
    }
    // The key itself, a row at a time; a value its column refuses; a row not there.
    all.push((7..=ROWS).map(|k| format!("CALL REKEY({k}, {})", k + 100)).collect());
    all.push(vec![
        "CALL SETD(1, 5)".to_owned(),
        "CALL SETD(2, -1)".to_owned(),
        "CALL SETA(99, 1)".to_owned(),
        "CALL REKEY(1, 2)".to_owned(),
    ]);
    all
}

/// Runs each list of `script` in `s` and commits it: what each call gave.
fn run(s: &Session, script: &[Vec<String>]) -> Vec<String> {
    let mut gave = Vec::new();
    for transaction in script {
        for call in transaction {
            gave.push(format!("{call}: {:?}", s.run(call).map(|_| ()).map_err(|e| (e.sqlcode(), e.sqlstate().as_str().to_owned()))));
        }
        s.commit().unwrap();
    }
    gave
}

/// T as `s` reads it: its rows, then the keys some values of A and of B
/// are found under, so each index answers for the entries it holds.
fn contents(s: &Session, kind: Kind) -> Vec<String> {
    let generated = if kind == Kind::Generated { ", G" } else { "" };
    let mut all = rows(s, &format!("SELECT {READ}{generated} FROM T ORDER BY K")).unwrap();
    for (column, values) in [("A", [101, 107, 112, 501, 606, 701, 706]), ("B", [201, 208, 212, 804, 811, 812, 813])] {
        for v in values {
            all.push(format!("{column} = {v}: {:?}", rows(s, &format!("SELECT K FROM T WHERE {column} = {v} ORDER BY K")).unwrap()));
        }
    }
    all.push(format!("A IS NULL: {:?}", rows(s, "SELECT K FROM T WHERE A IS NULL ORDER BY K").unwrap()));
    all.push(format!("K = 107: {:?}", rows(s, "SELECT A FROM T WHERE K = 107").unwrap()));
    s.commit().unwrap();
    all
}

/// What the cases leave of T of `kind`, compiled or not, in `dir`: what
/// each call gave and the rows after, the entries of its indexes, and its
/// log. A transaction of them rolled back between leaves the rows as they
/// were.
fn left(dir: &std::path::Path, (kind, compiled): (Kind, bool)) -> (Vec<String>, (usize, usize), Vec<u8>) {
    let (db, s) = database(dir, &os(), (kind, compiled));
    let mut gave = run(&s, &committed());
    let before = contents(&s, kind);
    for call in ["CALL SETA(2, 7)", "CALL SETV(2, 'undone')", "CALL SETMANY(3, 8, 'undone', 'undone')", "CALL BUMPB(3)", "CALL PICK(4, 2)"]
    {
        s.run(call).unwrap();
    }
    s.rollback().unwrap();
    assert_eq!(contents(&s, kind), before, "rolled back, {kind:?}, compiled {compiled}");
    gave.extend(before);
    let held = (db.index_entries("T_A").unwrap(), db.index_entries("T_B").unwrap());
    drop((s, db));
    (gave, held, bytes_of(dir, "log"))
}

/// Verifies: [DESIGN] RULES.md D915, D916, D917, D918, D841;
/// [X3.135-1986 §8.12 GR3]: the same UPDATEs written as patches of their
/// rows' records and written whole by the general path give the same
/// outcomes and leave the same rows, the same entries in each index and
/// the same log, byte for byte. Of every type of column, a value made
/// another, null and a value again; the first column, a key whose index
/// entry moves with it, columns between and the last; a varying column
/// grown and shrunk; several at once; a NOT NULL column refused null and
/// a key refused a duplicate; a SET value of a column it does not set; a
/// column read back through a CASE over an argument; and a transaction of
/// them rolled back. And of each table
/// whose rows are not patched, which falls back to the write it had: a
/// value out of line before or after, rows sealed, a large object column,
/// the column store's frozen rows, a system-time period, a CHECK of a
/// column no UPDATE reads, and a generated column of one.
#[test]
fn updates_written_as_patches_leave_what_whole_updates_leave() {
    let scratch = Scratch::new("patched-updates-alike");
    for kind in KINDS {
        let interpreted = left(&scratch.path(&format!("{kind:?}-interpreted")), (kind, false));
        let compiled = left(&scratch.path(&format!("{kind:?}-compiled")), (kind, true));
        for (n, (c, i)) in compiled.0.iter().zip(&interpreted.0).enumerate() {
            assert_eq!(c, i, "{kind:?}, line {n}: compiled, then interpreted");
        }
        assert_eq!(compiled.0.len(), interpreted.0.len(), "{kind:?}");
        assert_eq!(compiled.1, interpreted.1, "{kind:?}: index entries");
        let (c, i) = (&compiled.2, &interpreted.2);
        assert!(c == i, "{kind:?}: the logs differ: {} bytes compiled, {} interpreted", c.len(), i.len());
        if kind == Kind::Plain {
            assert!(holds(c, "the last column of row 6") && holds(c, "changed 1"), "the log holds the changes");
            let gave = &compiled.0;
            let refused = |call: &str| gave.iter().any(|line| line.starts_with(&format!("{call}: Err(")));
            assert!(refused("CALL SETD(2, -1)") && refused("CALL REKEY(1, 2)"), "NOT NULL and the key are checked of a patch: {gave:?}");
            assert!(gave.contains(&"A = 706: [\"6\"]".to_owned()) && gave.contains(&"K = 107: [\"107\"]".to_owned()), "{gave:?}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D916, D25; [X3.135-1986 §4.16]: recovery
/// after a crash replays the records of UPDATEs written as patches. Those
/// committed after the last checkpoint are done again, and those of a
/// transaction open at the crash are undone, every column they did not
/// set as it was. The rows and the keys each index finds are those of a
/// database that ran the committed UPDATEs alone, written whole.
#[test]
fn recovery_replays_updates_written_as_patches() {
    let scratch = Scratch::new("patched-updates-recovered");
    // The rows the open transaction holds, 9 and up, are left to it.
    let script: Vec<Vec<String>> = committed().into_iter().filter(|t| !t.iter().any(|call| call.starts_with("CALL REKEY("))).collect();
    let expected = {
        let (_db, s) = database(&scratch.path("whole"), &os(), (Kind::Plain, false));
        run(&s, &script);
        contents(&s, Kind::Plain)
    };
    let dir = scratch.path("crashed");
    let (crashed, s) = database(&dir, &after_crash(), (Kind::Plain, true));
    let open = session(&crashed, true);
    crashed.checkpoint().unwrap();
    // Still open at the crash, each of its rows changed more than once.
    for k in 9..=ROWS {
        for call in [
            format!("CALL SETA({k}, {k})"),
            format!("CALL SETMANY({k}, 0, 'open {k}', 'open')"),
            format!("CALL BUMPB({k})"),
            format!("CALL SETTIMES({k}, 1)"),
            format!("CALL SETV({k}, 'none')"),
        ] {
            open.run(&call).unwrap();
        }
    }
    // Its changes in the pages the checkpoint holds, for recovery to undo.
    crashed.checkpoint().unwrap();
    run(&s, &script);
    drop((s, crashed));
    let db = encoded();
    let opened = db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(db.damaged_tables().unwrap(), vec![]);
    let s = session(&db, true);
    assert_eq!(contents(&s, Kind::Plain), expected);
    // Its records hold as the rows it finds: patched again, they read the same.
    run(&s, &[vec!["CALL SETA(9, 109)".to_owned(), "CALL BUMPB(10)".to_owned(), "CALL SETV(11, 'after')".to_owned()]]);
    assert_eq!(
        rows(&s, "SELECT A, B, V FROM T WHERE K IN (9, 10, 11) ORDER BY K"),
        Ok(vec!["109,209,v 9".to_owned(), "110,211,v 10".to_owned(), "111,211,after".to_owned()])
    );
    drop(open);
}

/// The bytes of every file under `dir` but the log's.
fn beside_the_log(dir: &std::path::Path) -> Vec<u8> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("log")) {
            continue;
        }
        all.extend(if path.is_dir() { beside_the_log(&path) } else { std::fs::read(&path).unwrap() });
    }
    all
}

/// Verifies: [DESIGN] RULES.md D916, D30: a table told to seal its rows
/// after they were written is patched no more. A row written plain, then
/// updated by a compiled point UPDATE, is stored sealed as every row
/// written from then on is, here compressed by runs: the run of one
/// letter it is given is nowhere in the table's files as it is, where the
/// run a row was given before the option was set still is, and the row
/// reads back whole.
#[test]
fn a_row_of_a_table_sealed_since_is_written_sealed() {
    let scratch = Scratch::new("patched-updates-sealed-since");
    let dir = scratch.path("db");
    let (db, s) = database(&dir, &os(), (Kind::Plain, true));
    let (plain, sealed) = ("q".repeat(12), "z".repeat(12));
    for k in 5..=ROWS {
        s.run(&format!("CALL SETC({k}, '{plain}')")).unwrap();
        s.commit().unwrap();
    }
    s.run("ALTER TABLE T SET (ENCODINGS = 'test run-length')").unwrap();
    s.commit().unwrap();
    // Often enough for its plan to be kept again and its change written straight.
    for k in 1..=8_i64 {
        s.run(&format!("CALL SETC({k}, '{sealed}')")).unwrap();
        s.commit().unwrap();
    }
    assert_eq!(rows(&s, "SELECT C, V, TAIL FROM T WHERE K = 8"), Ok(vec![format!("{sealed},v 8,last 8")]));
    db.checkpoint().unwrap();
    drop((s, db));
    let files = beside_the_log(&dir);
    assert!(holds(&files, &plain), "written before the option, as it is");
    assert!(!holds(&files, &sealed), "written after it, sealed");
}

/// What sessions at `level` see of a row patched by another, compiled or
/// not: the row read before the patch commits, after it commits in the
/// transaction that read it before, and in the next; then what a writer
/// that waited for an open patch of its row gave once the patch
/// committed, and the row after.
fn seen_at(level: &str, compiled: bool) -> Vec<String> {
    let scratch = Scratch::new("patched-updates-seen");
    let (db, a) = database(&scratch.path("db"), &os(), (Kind::Plain, compiled));
    for k in 1..=6_i64 {
        a.run(&format!("CALL SETV({k}, 'warm')")).unwrap();
        a.run(&format!("CALL BUMPB({k})")).unwrap();
        a.commit().unwrap();
    }
    // A writer waits for the transaction that holds its row.
    db.set_setting("LOCKS.WAIT", &Value::Char("ON".to_owned())).unwrap();
    let b = session(&db, compiled);
    let read = |s: &Session| rows(s, "SELECT B, V, C FROM T WHERE K = 1").unwrap().concat();
    let mut seen = Vec::new();
    b.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
    a.run("CALL SETV(1, 'patched')").unwrap();
    a.run("CALL BUMPB(1)").unwrap();
    seen.push(format!("open: {}", read(&b)));
    a.commit().unwrap();
    seen.push(format!("committed: {}", read(&b)));
    b.commit().unwrap();
    seen.push(format!("next: {}", read(&b)));
    b.commit().unwrap();
    // A writer of the row the patch holds waits, then writes over what it committed or is refused.
    b.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
    seen.push(format!("before: {}", read(&b)));
    a.run("CALL SETV(1, 'held')").unwrap();
    let waiter = thread::spawn(move || {
        let gave = b.run("CALL SETC(1, 'waiter')").map(|_| ()).map_err(|e| e.sqlstate().as_str().to_owned());
        let ended = if gave.is_ok() { b.commit() } else { b.rollback() };
        (format!("{gave:?}"), ended.is_ok())
    });
    until_waiting(&db, 1);
    a.commit().unwrap();
    seen.push(format!("waiter: {:?}", waiter.join().unwrap()));
    seen.push(format!("after: {}", read(&a)));
    seen
}

/// Verifies: [DESIGN] RULES.md D916, D917, D29; [ISO/IEC 9075:1992 §4.28]:
/// a row patched is seen as a row written whole is. At READ COMMITTED and
/// at SERIALIZABLE, another transaction reads the row before while the
/// patch is open; once it commits, READ COMMITTED reads the row after
/// and SERIALIZABLE the one of its snapshot, until its next transaction.
/// A writer of another column that waited for the patch writes over what
/// it committed at READ COMMITTED, its row read again whole, the column
/// the patch set kept, and is refused at SERIALIZABLE (SQLCODE -911).
/// Compiled and interpreted give the same.
#[test]
fn a_row_patched_is_seen_as_a_row_written_whole() {
    for level in ["READ COMMITTED", "SERIALIZABLE"] {
        let (interpreted, compiled) = (seen_at(level, false), seen_at(level, true));
        assert_eq!(compiled, interpreted, "{level}");
        assert_eq!(&compiled[..1], ["open: 202,warm,row 1       "], "{level}");
        let snapshot = level == "SERIALIZABLE";
        let committed = if snapshot { "committed: 202,warm,row 1       " } else { "committed: 203,patched,row 1       " };
        assert_eq!((compiled[1].as_str(), compiled[2].as_str()), (committed, "next: 203,patched,row 1       "), "{level}");
        let (waiter, after) = if snapshot {
            ("waiter: (\"Err(\\\"TB911\\\")\", true)", "after: 203,held,row 1       ")
        } else {
            ("waiter: (\"Ok(())\", true)", "after: 203,held,waiter      ")
        };
        assert_eq!((compiled[4].as_str(), compiled[5].as_str()), (waiter, after), "{level}");
    }
}

/// Verifies: [DESIGN] RULES.md D916, the default
/// `TableStorage::replace_set`: an engine that is not the row store,
/// told the columns an update sets and given a row that holds their
/// values alone, reads the row whole, gives it those values and writes
/// it whole, every other column kept; told no columns, it writes the row
/// as it is given; and of no such row it is refused (SQLCODE -910).
#[test]
fn an_engine_that_patches_nothing_writes_the_row_whole() {
    let int = |n: i128| Value::Exact(Decimal::new(n, 0));
    let table = MemoryTable::new();
    let id = table.insert(0, vec![int(1), Value::Char("kept".to_owned()), int(3)]).unwrap();
    let shown = |row: &[Value]| row.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
    let set = vec![Value::Null, Value::Null, int(9)];
    let mut image = AfterImage::new();
    let before = table.replace_set(0, id, Some(&set), (Some(Patch { set: &[2], image: &mut image }), &[2])).unwrap();
    assert_eq!(shown(&before), "1,kept,3", "the row replaced");
    assert_eq!(shown(&table.get(0, id).unwrap().unwrap()), "1,kept,9", "the columns not set are kept");
    let nulled = vec![Value::Null, Value::Null, Value::Null];
    table.replace_set(0, id, Some(&nulled), (Some(Patch { set: &[0, 2], image: &mut image }), &[0, 2])).unwrap();
    assert_eq!(shown(&table.get(0, id).unwrap().unwrap()), format!("{0},kept,{0}", Value::Null), "set to null");
    table.replace_set(0, id, Some(&set), (None, &[2])).unwrap();
    assert_eq!(shown(&table.get(0, id).unwrap().unwrap()), format!("{0},{0},9", Value::Null), "told no columns: the row as given");
    let missing = table.replace_set(0, id + 1, Some(&set), (Some(Patch { set: &[2], image: &mut image }), &[2]));
    assert_eq!(missing.err().map(|e| e.sqlcode()), Some(-910));
}
