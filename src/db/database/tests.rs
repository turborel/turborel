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
