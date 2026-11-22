//! Unit tests of what a session does when the snapshot cannot be taken
//! (`database.rs`).

use std::thread;

use super::*;

/// Verifies: [DESIGN] RULES.md D27: if a thread panicked while publishing
/// a snapshot, every call reports SQLCODE -910 (the definitions cannot be
/// trusted), and a failed call does not leave its session marked busy.
#[test]
fn a_poisoned_snapshot_is_reported() {
    let db = Database::new();
    let shared = Arc::clone(&db.0);
    let poisoned = thread::spawn(move || {
        let _held = shared.current.write();
        panic!("a thread panicked while publishing");
    });
    assert!(poisoned.join().is_err());
    let s = db.session("A");
    for attempt in ["first call", "second call"] {
        let failed = s.run("COMMIT WORK").err();
        assert!(failed.is_some_and(|e| e.code == Code::Api && e.msg.contains("panicked")), "{attempt}: not refused as busy");
    }
}

/// Verifies: [DESIGN] RULES.md D780: a session's statements take its own
/// copy of the snapshot, taking no count of the one published, until a
/// change of definitions is published, which its next statement sees.
/// MC/DC of taking it again: a snapshot published since or not.
#[test]
fn a_session_takes_its_own_snapshot_until_one_is_published() {
    let db = Database::new();
    let s = db.session("A");
    let first = s.current_state().unwrap();
    let published = db.snapshot().unwrap();
    let holders = Arc::strong_count(&published);
    let again = s.current_state().unwrap();
    assert!(Arc::ptr_eq(&first, &again), "none published since: its own copy");
    assert!(!Arc::ptr_eq(&first, &published), "a copy, not the published snapshot");
    assert_eq!(Arc::strong_count(&published), holders, "no count of the published snapshot taken");
    drop((first, again));
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A INTEGER)").unwrap();
    let after = s.current_state().unwrap();
    assert!(after.generation > published.generation, "taken again once one is published");
}
