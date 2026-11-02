//! The standby seam (RULES.md D57): a database following another's log
//! replays it as recovery does, holds a transaction until it ends,
//! spilling what it holds past its budget, serves once promoted, ending
//! the transactions left open, and writes nothing once deposed; an export
//! of a database, its last checkpoint's image and durable files, is
//! installed by a database following in place of its own state.

use std::path::Path;
use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Standby};
use turborel::error::Code;
use turborel::log::record::Record;
use turborel::log::{Log, Lsn, MemoryLog};
use turborel::value::types::TypeRegistry;

use crate::common::{Counted, Kept, Scratch, Watched, exec, number, os, rows};
use crate::replicated_state::committed;

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

/// Verifies: [DESIGN] RULES.md D57, D38, the standby seam: the records a
/// standby holds of a transaction not yet ended past MEMORY.WORK_BYTES are
/// written to a spill area, not kept in memory, and read back, merged in
/// position order with those of a transaction committed among them, as
/// the transaction ends: committed, the standby holds what its primary
/// holds, rows and index entries; left open, promotion ends it, the
/// standby holding nothing of it. Regression test (R-TEST-7): a standby
/// held every record of an open transaction in memory, however many.
#[test]
fn a_standby_spills_the_records_it_holds() {
    for commit in [true, false] {
        let log = Arc::new(MemoryLog::new());
        let primary = primary(&log);
        let counted = Counted::default();
        let standby = following();
        standby.set_spill(Box::new(counted.clone())).unwrap();
        standby.set_setting("MEMORY.WORK_BYTES", &number(64_i64 << 10)).unwrap();
        replay_one_by_one(&standby, &*log, Lsn(0));
        counted.taken();

        let (mark, writer) = (log.end(), primary.session("APP"));
        for k in 100_u32..1100_u32 {
            exec(&writer, &format!("INSERT INTO T VALUES ({k}, 'v{k}')")).unwrap();
            if k == 600_u32 {
                committed(&primary, &["UPDATE T SET V = 'mid' WHERE K = 2"]);
            }
        }
        Standby::replay(&standby, &mut log.read_from(mark)).unwrap();
        assert!(counted.taken() > 0, "the records held spilled");
        assert_eq!(answers(&standby, &[]).0, ["1,uno       ", "2,mid       "], "the open transaction not shown");

        let ended_at = log.end();
        if commit {
            writer.commit().unwrap();
            Standby::replay(&standby, &mut log.read_from(ended_at)).unwrap();
            assert_eq!(answers(&standby, &["T_V"]), answers(&primary, &["T_V"]));
            assert_eq!(answers(&standby, &[]).0.len(), 1002_usize);
        } else {
            standby.promote(Arc::new(MemoryLog::new())).unwrap();
            assert_eq!(answers(&standby, &[]).0, ["1,uno       ", "2,mid       "], "ended by promotion");
            assert_eq!(standby.index_entries("T_V").unwrap(), 2_usize);
        }
    }
}

/// A spill whose areas the memory budget always refuses (SQLCODE -918).
struct Refusing;

impl turborel::spill::Spill for Refusing {
    fn name(&self) -> &'static str {
        "refusing"
    }
    fn area(
        &self,
        _memory: &Arc<turborel::memory::MemoryBudget>,
        _temp: &Arc<turborel::spill::TempFiles>,
        _area: turborel::spill::Area,
    ) -> turborel::error::Result<Box<dyn turborel::spill::SpillArea>> {
        Err(turborel::error::SqlError::new(Code::Resources, "[DESIGN] refused"))
    }
}

/// Verifies: [DESIGN] RULES.md D57, D38, D40, the standby seam: a standby
/// whose spill area for the records it holds is refused (SQLCODE -918)
/// tells it as an event and holds them in memory all the same, never
/// losing one: its replay succeeds, and once the transaction commits it
/// holds what its primary holds.
#[test]
fn a_standby_holds_its_records_when_they_cannot_spill() {
    let log = Arc::new(MemoryLog::new());
    let primary = primary(&log);
    let (standby, kept) = (following(), Kept::default());
    standby.set_spill(Box::new(Refusing)).unwrap();
    standby.set_setting("MEMORY.WORK_BYTES", &number(64_i64 << 10)).unwrap();
    standby.set_diagnostics(Box::new(kept.clone())).unwrap();
    let writer = primary.session("APP");
    for k in 100_u32..1100_u32 {
        exec(&writer, &format!("INSERT INTO T VALUES ({k}, 'v{k}')")).unwrap();
    }
    writer.commit().unwrap();
    let commit = Lsn(log.end().0 - 1);
    let held: Vec<_> = log.read_from(Lsn(0)).filter(|r| r.as_ref().is_ok_and(|(at, _)| *at < commit)).collect();
    Standby::replay(&standby, &mut held.into_iter()).unwrap();
    assert!(kept.taken().iter().any(|(_, e)| e.contains("Failed") && e.contains("refused")), "the refusal told");
    // The replay of the transaction, as recovery's, spills its changes.
    standby.set_spill(Box::new(turborel::spill::PagedSpill)).unwrap();
    Standby::replay(&standby, &mut log.read_from(commit)).unwrap();
    assert_eq!(answers(&standby, &["T_V"]), answers(&primary, &["T_V"]));
}

/// Verifies: [EXTENSION] RULES.md D57: a database's export is its last
/// checkpoint's image and its durable files, the data file among them,
/// read while it lives, during which it takes no checkpoint (SQLCODE
/// -910); a database following installs it in place of its own state:
/// its files replaced, the image its catalog's, and itself opened again
/// from it, holding the log through the position the image does, with
/// the exporter's rows and schemata, following still, so it takes no
/// change (-917); the export dropped, checkpoints go on. A database
/// serving takes no export (-910), and one kept in memory exports none.
#[test]
fn an_export_is_installed_by_a_following_database() {
    let scratch = Scratch::new("replicated-export");
    let source = Database::new();
    source.open_files(&os(), &scratch.path("source"), &OpenOptions::default()).unwrap();
    source.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, S CHAR(200))").unwrap();
    let s = source.session("APP");
    for k in 0..300_i32 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'exported {k}')")).unwrap();
    }
    s.commit().unwrap();
    drop(s);
    source.checkpoint().unwrap();
    let export = source.export().unwrap();
    assert!(export.files().iter().any(|f| f.within == Path::new("data")), "{:?}", export.files());
    assert_eq!(source.checkpoint().err().map(|e| e.code), Some(Code::Api), "no checkpoint while the export lives");
    assert_eq!(Standby::import(&source, Box::new(Nothing)).err().map(|e| e.code), Some(Code::Api), "serving");

    let follower = Database::new();
    follower.open_following(&os(), &scratch.path("follower"), &OpenOptions::default()).unwrap();
    let holds = Standby::import(&follower, export).unwrap();
    assert!(holds.0 > 0, "{holds:?}");
    let expected = rows(&source.session("APP"), "SELECT S FROM T WHERE K IN (7, 299) ORDER BY S");
    assert_eq!(rows(&follower.session("APP"), "SELECT S FROM T WHERE K IN (7, 299) ORDER BY S"), expected);
    assert_eq!(rows(&follower.session("APP"), "SELECT COUNT(*) FROM T"), Ok(vec!["300".into()]));
    assert_eq!(follower.session("APP").run("INSERT INTO T VALUES (300, 'mine')").err().map(|e| e.code), Some(Code::TransactionState));
    source.checkpoint().unwrap();
    assert_eq!(Database::new().export().err().map(|e| e.code), Some(Code::Api));
    follower.close().unwrap();
    source.close().unwrap();
}

/// An export of nothing: an image of no bytes, no file.
struct Nothing;

impl turborel::db::StateSource for Nothing {
    fn image(&self) -> &[u8] {
        &[]
    }
    fn files(&self) -> &[turborel::db::ExportedFile] {
        &[]
    }
    fn read(&mut self, _: usize, _: u64, _: &mut [u8]) -> turborel::error::Result<usize> {
        Ok(0)
    }
}
