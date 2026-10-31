//! Unit tests of temporary tables' instances: their collection as their
//! session's transactions end (RULES.md D49, R-TEST-1). Split from
//! temporary.rs (RULES.md R-SIZE-2).

use super::storage::Entries;
use super::*;
use crate::db::Database;
use crate::error::Code;
use crate::index::searched;
use crate::index::{Key, KeyOrder, Probe};
use crate::storage::RowId;
use crate::txn::NO_TRANSACTION;

/// The rows of `session`'s instance of APP.`table`, if made, and the
/// entries of each of its indexes, each pair once.
fn instance(session: &Session, table: &str) -> (Option<Arc<dyn TableStorage>>, Vec<usize>) {
    let made = lock_builtin(&session.temporaries.made);
    let (_, found) = made.iter().find(|((name, _), _)| name.ident == table).unwrap();
    let searched_whole = |entries: &Arc<Entries>| {
        let index: Arc<dyn Index> = Arc::<Entries>::clone(entries);
        searched(&index, &Probe::All).unwrap().len()
    };
    let entries = found.rows.all_entries().iter().map(searched_whole).collect();
    (found.rows.made(), entries)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.9] RULES.md D49: an
/// instance of ON COMMIT PRESERVE ROWS is collected as each transaction of
/// its session ends, of every version superseded or deleted, its index
/// entries with them, though another session's transaction, open
/// throughout, holds the database's horizon: no other session reads it.
/// Regression test (R-TEST-7): an instance was collected only by the
/// database's horizon, so another session's long transaction kept its
/// every version, and a collection due later went to another session's
/// instance.
#[test]
fn an_instance_is_collected_as_its_session_ends_a_transaction() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION APP
           CREATE TABLE P (K INTEGER)
           CREATE GLOBAL TEMPORARY TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(20)) ON COMMIT PRESERVE ROWS",
    )
    .unwrap();
    let (s, other) = (db.session("APP"), db.session("APP"));
    other.run("INSERT INTO P VALUES (1)").unwrap();
    assert!(s.open_cursor("SELECT K FROM T WHERE K = 1").unwrap().fetch().unwrap().is_none(), "no entries made yet");
    s.run("INSERT INTO T VALUES (1, 'a')").unwrap();
    s.commit().unwrap();
    for _ in 0_i32..20_i32 {
        s.run("UPDATE T SET K = K + 1").unwrap();
        s.commit().unwrap();
    }
    s.run("INSERT INTO T VALUES (100, 'gone')").unwrap();
    s.run("DELETE FROM T WHERE K = 100").unwrap();
    s.commit().unwrap();
    let (rows, entries) = instance(&s, "T");
    let rows = rows.unwrap();
    assert!(!rows.uncollected(), "nothing awaits collection");
    assert_eq!(entries, vec![1], "one entry, of the one version left");
    let kept: Vec<String> = Arc::clone(&rows).scan(NO_TRANSACTION, &[0]).map(|r| r.unwrap().1[0].to_string()).collect();
    assert_eq!(kept, ["21"]);
    other.rollback().unwrap();
}

/// Verifies: [DESIGN] RULES.md D49: an instance's entries that could not
/// be made complete, its session's transaction having its rows open,
/// refuse every use that reads or writes them (SQLCODE -911), and give the
/// planner no estimate; entries not yet made hold none, a hash index's as
/// a B-link tree's, and are made as first written.
#[test]
fn entries_not_made_complete_refuse_every_use() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE GLOBAL TEMPORARY TABLE T (K INTEGER)").unwrap();
    let st = db.snapshot().unwrap();
    let desc = st.catalog.tables().into_iter().find(|t| t.name.ident == "T").cloned().unwrap();
    let ix = |method: &str| IndexDesc {
        name: "T_K".into(),
        table: desc.name.clone(),
        columns: vec![0],
        method: method.into(),
        unique: false,
        options: Vec::new(),
    };
    let key = Key(vec![crate::value::Value::Exact(crate::value::Decimal::new(1_i128, 0_u32))]);
    let refusing = Arc::new(Entries::refusing(&desc, &ix("btree")));
    let refused = |r: Result<()>| r.map_err(|e| e.code);
    assert_eq!(refused(refusing.insert(key.clone(), 1)), Err(Code::Conflict));
    assert_eq!(refused(refusing.remove(&key, 1)), Err(Code::Conflict));
    assert_eq!(refusing.rows(&key).map_err(|e| e.code), Err(Code::Conflict));
    let mut none = std::iter::empty();
    assert_eq!(refused(refusing.load(&mut none, (&st.work, crate::spill::Area::new(1_024, 64)))), Err(Code::Conflict));
    assert_eq!((refusing.estimate(&Probe::All), refusing.distinct_keys()), (Some(0), None));
    let searched_refused = Arc::clone(&refusing).search(&Probe::All, KeyOrder::Ascending).next().unwrap();
    assert_eq!(searched_refused.map_err(|e| e.code).err(), Some(Code::Conflict));
    for method in ["hash", "btree"] {
        let entries = Arc::new(Entries::new(&desc, &ix(method), &st.memory));
        assert_eq!(entries.rows(&key).unwrap(), Vec::<RowId>::new(), "{method}: none made");
        assert_eq!(entries.remove(&key, 1).map_err(|e| e.code), Ok(()));
        assert!(Arc::clone(&entries).search(&Probe::All, KeyOrder::Ascending).next().is_none());
        entries.insert(key.clone(), 7).unwrap();
        assert_eq!(entries.rows(&key).unwrap(), vec![7], "{method}");
    }
}
