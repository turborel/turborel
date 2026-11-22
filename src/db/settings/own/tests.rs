//! Unit tests of the state overlaid with a session's own settings: kept
//! while the published state and the session's values stay (RULES.md D36,
//! R-TEST-1).

use std::sync::Arc;

use crate::catalog::QualifiedName;
use crate::db::{Database, Session, State};
use crate::settings::MEMORY_WORK_BYTES;

/// MEMORY.WORK_BYTES as `st` holds it.
fn work(st: &State) -> i128 {
    st.settings.integer(MEMORY_WORK_BYTES)
}

/// `n`, as `work` reads it.
fn bytes(n: i128) -> i128 {
    n
}

/// Whether `st` has table APP.`name`.
fn has(st: &State, name: &str) -> bool {
    st.catalog.table(&QualifiedName { auth: "APP".into(), ident: name.into() }).is_some()
}

/// A database of schema APP, and a session of it that set MEMORY.WORK_BYTES.
fn set() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("APP");
    s.run("SET MEMORY.WORK_BYTES = 131072").unwrap();
    (db, s)
}

/// Verifies: [DESIGN] RULES.md D36: a session of no setting of its own
/// runs against its copy of the published state (D780); one that set one, against a
/// copy made once and kept for its next statements, its value read
/// there; a new SET makes the copy anew, of the new value.
#[test]
fn the_overlaid_state_is_kept_until_a_set() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("APP");
    let published = db.snapshot().unwrap();
    let plain = s.current_state().unwrap();
    // Its own copy of the published state (RULES.md D780), no overlay.
    assert!(Arc::ptr_eq(&s.current_state().unwrap(), &plain), "no setting of its own, no copy made again");
    assert_eq!(plain.generation, published.generation);
    s.run("SET MEMORY.WORK_BYTES = 131072").unwrap();
    let first = s.current_state().unwrap();
    assert!(!Arc::ptr_eq(&first, &plain));
    assert!(Arc::ptr_eq(&s.current_state().unwrap(), &first), "kept for the next statement");
    assert_eq!(work(&first), bytes(131_072));
    s.run("SET MEMORY.WORK_BYTES = 262144").unwrap();
    let second = s.current_state().unwrap();
    assert!(!Arc::ptr_eq(&second, &first), "a SET makes it anew");
    assert_eq!(work(&second), bytes(262_144));
}

/// Verifies: [DESIGN] RULES.md D36: a definition another session
/// publishes, a new state, is seen by the next statement of a session
/// that set a setting of its own, its value kept over it.
#[test]
fn a_definition_published_by_another_session_is_seen() {
    let (db, s) = set();
    let before = s.current_state().unwrap();
    assert!(!has(&before, "U"));
    let other = db.session("APP");
    other.run("CREATE TABLE U (K INTEGER)").unwrap();
    other.commit().unwrap();
    let after = s.current_state().unwrap();
    assert!(has(&after, "U"), "the published definition seen");
    assert_eq!(work(&after), bytes(131_072), "its own value kept");
    s.run("INSERT INTO U VALUES (1)").unwrap();
}

/// Verifies: [DESIGN] RULES.md D36: a role set does not change what a
/// session's settings are: its next statements read its own value over
/// the published state still, and see a definition published since.
#[test]
fn a_role_change_reads_the_same_settings() {
    let db = Database::new();
    db.create_user("OWNER", b"x").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER)").unwrap();
    let owner = db.session("OWNER");
    owner.run("CREATE ROLE CLERK").unwrap();
    owner.run("GRANT CLERK TO APP").unwrap();
    owner.commit().unwrap();
    let s = db.session("APP");
    s.run("SET MEMORY.WORK_BYTES = 131072").unwrap();
    s.run("SET ROLE 'CLERK'").unwrap();
    assert_eq!(work(&s.current_state().unwrap()), bytes(131_072));
    let other = db.session("APP");
    other.run("CREATE TABLE U (K INTEGER)").unwrap();
    other.commit().unwrap();
    s.run("SET ROLE NONE").unwrap();
    let st = s.current_state().unwrap();
    assert!(has(&st, "U"));
    assert_eq!(work(&st), bytes(131_072));
}
