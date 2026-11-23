//! A session refuses a call back into itself (RULES.md R-ERR-4, R-TEST-2
//! robustness).

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
