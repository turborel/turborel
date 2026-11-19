//! \[EXTENSION\] Advisory locks (RULES.md D187): held by a session to its
//! end or by a transaction to its end, exclusive or shared, waited for or
//! tried, waits ending as table and row locks' do.

use std::thread;

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::txn::LockWait;

/// A database whose sessions wait for locks.
fn database() -> Database {
    let db = Database::new();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db
}

/// The one value `call` gives in `s`, as text.
fn value(s: &Session, call: &str) -> Result<String, Code> {
    let mut c = s.open_cursor(&format!("VALUES ({call})")).map_err(|e| e.code)?;
    let row = c.fetch().map_err(|e| e.code)?.unwrap_or_default();
    Ok(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","))
}

/// Verifies: [EXTENSION] RULES.md D187: a session's exclusive lock stands
/// against another session's try until it is unlocked as many times as it
/// was taken; it outlives a commit; an unlock of a key not held is false.
#[test]
fn a_session_lock_is_held_until_unlocked_d187() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("A"));
    assert_eq!(value(&a, "ADVISORY_LOCK(7)"), Ok("TRUE".to_owned()));
    assert_eq!(value(&a, "TRY_ADVISORY_LOCK(7)"), Ok("TRUE".to_owned()), "taken again by its holder");
    a.commit().unwrap();
    assert_eq!(value(&b, "TRY_ADVISORY_LOCK(7)"), Ok("FALSE".to_owned()), "a session's hold outlives a commit");
    assert_eq!(value(&a, "ADVISORY_UNLOCK(7)"), Ok("TRUE".to_owned()));
    assert_eq!(value(&b, "TRY_ADVISORY_LOCK(7)"), Ok("FALSE".to_owned()), "taken twice, unlocked once");
    assert_eq!(value(&a, "ADVISORY_UNLOCK(7)"), Ok("TRUE".to_owned()));
    assert_eq!(value(&a, "ADVISORY_UNLOCK(7)"), Ok("FALSE".to_owned()), "no longer held");
    assert_eq!(value(&b, "TRY_ADVISORY_LOCK(7)"), Ok("TRUE".to_owned()));
}

/// Verifies: [EXTENSION] RULES.md D187: shared holds stand together, and
/// against an exclusive one; keys of a number, a pair and a name are each
/// their own.
#[test]
fn shared_holds_and_keys_of_each_kind_d187() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("A"));
    assert_eq!(value(&a, "ADVISORY_LOCK_SHARED(1)"), Ok("TRUE".to_owned()));
    assert_eq!(value(&b, "TRY_ADVISORY_LOCK_SHARED(1)"), Ok("TRUE".to_owned()));
    assert_eq!(value(&b, "TRY_ADVISORY_LOCK(1)"), Ok("FALSE".to_owned()));
    assert_eq!(value(&a, "ADVISORY_LOCK(1, 2)"), Ok("TRUE".to_owned()));
    assert_eq!(value(&b, "TRY_ADVISORY_LOCK(2, 1)"), Ok("TRUE".to_owned()), "another pair");
    assert_eq!(value(&a, "ADVISORY_LOCK('jobs')"), Ok("TRUE".to_owned()));
    assert_eq!(value(&b, "TRY_ADVISORY_LOCK('jobs')"), Ok("FALSE".to_owned()));
    let null = value(&b, "TRY_ADVISORY_LOCK(CAST(NULL AS INTEGER))").unwrap();
    assert!(null.is_empty() || null == "NULL", "of a null key, null: {null}");
    assert_eq!(value(&b, "ADVISORY_LOCK(1.5)"), Err(Code::SyntaxRule));
    assert_eq!(value(&a, "ADVISORY_UNLOCK_ALL()"), Ok("TRUE".to_owned()));
    assert_eq!(value(&b, "TRY_ADVISORY_LOCK('jobs')"), Ok("TRUE".to_owned()), "every hold of a unlocked");
}

/// Verifies: [EXTENSION] RULES.md D187: a transaction's hold ends with
/// it; a session's ends with the session.
#[test]
fn transaction_holds_end_with_it_and_session_holds_with_it_d187() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("A"));
    assert_eq!(value(&a, "ADVISORY_XACT_LOCK(3)"), Ok("TRUE".to_owned()));
    assert_eq!(value(&a, "ADVISORY_UNLOCK(3)"), Ok("FALSE".to_owned()), "no session hold to unlock");
    assert_eq!(value(&b, "TRY_ADVISORY_XACT_LOCK(3)"), Ok("FALSE".to_owned()));
    b.rollback().unwrap();
    a.rollback().unwrap();
    assert_eq!(value(&b, "TRY_ADVISORY_XACT_LOCK(3)"), Ok("TRUE".to_owned()), "a's ended with its transaction");
    b.commit().unwrap();
    assert_eq!(value(&a, "ADVISORY_LOCK(4)"), Ok("TRUE".to_owned()));
    drop(a);
    assert_eq!(value(&b, "TRY_ADVISORY_LOCK(4)"), Ok("TRUE".to_owned()), "a's ended with the session");
}

/// Verifies: [EXTENSION] RULES.md D187: a lock held by another session is
/// waited for until unlocked; a cycle of waits is a deadlock, refused at
/// once, SQLCODE -911; a session that does not wait is refused at once.
#[test]
fn waits_end_at_an_unlock_or_a_deadlock_d187() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("A"));
    assert_eq!(value(&a, "ADVISORY_LOCK(1)"), Ok("TRUE".to_owned()));
    assert_eq!(value(&b, "ADVISORY_LOCK(2)"), Ok("TRUE".to_owned()));
    let waiter = thread::spawn(move || (value(&b, "ADVISORY_LOCK(1)"), b));
    while db.waiting_statements().unwrap() == 0 {
        assert!(!waiter.is_finished(), "b waits for a");
        thread::yield_now();
    }
    assert_eq!(value(&a, "ADVISORY_LOCK(2)"), Err(Code::Conflict), "a deadlock");
    assert_eq!(value(&a, "ADVISORY_UNLOCK(1)"), Ok("TRUE".to_owned()));
    let (got, b) = waiter.join().unwrap();
    assert_eq!(got, Ok("TRUE".to_owned()));
    drop(b);
    let quick = Database::new();
    let (c, d) = (quick.session("A"), quick.session("A"));
    assert_eq!(value(&c, "ADVISORY_LOCK(5)"), Ok("TRUE".to_owned()));
    assert_eq!(value(&d, "ADVISORY_LOCK(5)"), Err(Code::Conflict), "no-wait sessions are refused");
}
