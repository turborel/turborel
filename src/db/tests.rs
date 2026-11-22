//! A session refuses a call back into itself (RULES.md R-ERR-4, R-TEST-2
//! robustness); a served session collects the tables it wrote itself.

use std::sync::atomic::Ordering;

use super::*;

/// Verifies: [DESIGN] RULES.md R-ERR-4: a call into a session already
/// running a call, as a plug-in calling back into it would make, is
/// refused with SQLCODE -910; once the first call ends the session works.
#[test]
fn a_session_refuses_a_call_back_into_itself() {
    let db = Database::new();
    let s = db.session("A");
    let call = s.enter().unwrap();
    assert_eq!(s.enter().err().map(|e| e.code), Some(Code::Api));
    drop(call);
    assert!(s.enter().is_ok());
}

/// Verifies: [DESIGN] RULES.md D450, D453: a session collecting after its
/// reply collects the table its transaction wrote, its old version's index
/// entry taken back, though another session's collection of every table
/// awaiting it is under way. Regression test (R-TEST-7): its collection passed by while
/// another was under way, so at many sessions one collected every table,
/// fell behind the commits, and the transactions kept for validation grew.
#[test]
fn a_served_session_collects_the_tables_it_wrote() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER NOT NULL, X INTEGER)").unwrap();
    db.create_index("T_X", "APP.T", &["X"], "btree").unwrap();
    let s = db.session("APP");
    s.run("INSERT INTO T VALUES (1, 0), (2, 0)").unwrap();
    s.commit().unwrap();
    s.collect_after_reply(true);
    s.run("UPDATE T SET X = 1 WHERE ID = 1").unwrap();
    s.commit().unwrap();
    let state = Arc::clone(s.enter().unwrap().state());
    state.tx.read_collecting().store(true, Ordering::SeqCst);
    assert_eq!(db.index_entries("T_X").unwrap(), 3, "the old version's entry kept");
    s.collect();
    assert_eq!(db.index_entries("T_X").unwrap(), 2, "collected by the session that wrote it");
    state.tx.read_collecting().store(false, Ordering::SeqCst);
}
