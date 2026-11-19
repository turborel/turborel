//! UUID_V4 and UUID_V7 (RULES.md D182, D183): the bits of the UUIDs they
//! make, each row its own, and version 7's order and clock.

use std::sync::Arc;

use turborel::clock::FixedClock;
use turborel::db::Database;
use turborel::error::Code;

use crate::common::{code, rows};

/// An instant, in microseconds from 1970: 2023-11-14 22:13:20.123 UTC.
const AT: i64 = 1_700_000_000_123_000;

/// A database of schema A, its clock fixed at `AT`, table T of 50 rows
/// numbered by K.
fn numbered() -> (Database, Arc<FixedClock>) {
    let db = Database::new();
    let clock = Arc::new(FixedClock::at(AT));
    db.set_clock(Arc::clone(&clock) as Arc<dyn turborel::clock::Clock>).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let values: Vec<String> = (1..=50).map(|k| format!("({k})")).collect();
    db.session("A").run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    (db, clock)
}

/// Verifies: [EXTENSION] RULES.md D182, D183 [RFC 9562 §4.1, §4.2]: each
/// row gets a UUID_V4 of its own, of version 4, and a UUID_V7 of version
/// 7, both of variant `10`; neither is folded into a constant.
#[test]
fn generated_uuids_carry_version_and_variant() {
    let (db, _) = numbered();
    let s = db.session("A");
    let made = rows(&s, "SELECT CAST(UUID_V4() AS VARCHAR(36)), CAST(UUID_V7() AS VARCHAR(36)) FROM T").unwrap();
    let pairs: Vec<(&str, &str)> = made.iter().map(|r| r.split_once(',').unwrap()).collect();
    for (four, seven) in &pairs {
        let (four, seven) = (four.as_bytes(), seven.as_bytes());
        assert_eq!((four[14], seven[14]), (b'4', b'7'), "{pairs:?}");
        assert!(b"89ab".contains(&four[19]) && b"89ab".contains(&seven[19]), "the variant: {pairs:?}");
    }
    let mut v4: Vec<&str> = pairs.iter().map(|(four, _)| *four).collect();
    v4.sort_unstable();
    v4.dedup();
    assert_eq!(v4.len(), 50, "each row its own");
    assert_eq!(code(&s, "SELECT UUID_V4(1) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT UUID_V7(K) FROM T"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] RULES.md D183 [RFC 9562 §5.7, §6.2]: UUID_V7
/// begins with the milliseconds of the database's clock, and each made
/// sorts after the last, in one statement, across statements and
/// sessions, and as the clock goes back; a later millisecond begins
/// with it.
#[test]
fn ordered_uuids_follow_the_clock() {
    let (db, clock) = numbered();
    let s = db.session("A");
    let first = rows(&s, "SELECT CAST(UUID_V7() AS VARCHAR(36)) FROM T ORDER BY K").unwrap();
    assert!(first.iter().all(|u| u.starts_with("018bcfe5-687b-7")), "the clock's milliseconds: {first:?}");
    assert!(first.windows(2).all(|w| w[0] < w[1]), "in the order made: {first:?}");
    clock.set(AT - 60_000_000);
    let back = rows(&db.session("A"), "SELECT CAST(UUID_V7() AS VARCHAR(36)) FROM T WHERE K = 1").unwrap();
    assert!(back[0] > first[49], "the clock gone back, still after the last");
    clock.set(AT + 1_000);
    let later = rows(&s, "SELECT CAST(UUID_V7() AS VARCHAR(36)) FROM T WHERE K = 1").unwrap();
    assert!(later[0].starts_with("018bcfe5-687c-7"), "the next millisecond: {later:?}");
}

/// Verifies: [EXTENSION] RULES.md D180, D183: UUIDs made into a keyed
/// column with an index on it are each kept, and a range of the index
/// over the clock's millisecond finds the version 7 ones alone.
#[test]
fn generated_uuids_fill_a_key() {
    let (db, _) = numbered();
    let s = db.session("A");
    s.run("CREATE TABLE U (ID UUID PRIMARY KEY, K INTEGER)").unwrap();
    s.commit().unwrap();
    db.create_index("U_ID", "A.U", &["ID"], "btree").unwrap();
    s.run("INSERT INTO U SELECT UUID_V7(), K FROM T").unwrap();
    s.run("INSERT INTO U SELECT UUID_V4(), K FROM T").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(DISTINCT ID) FROM U"), Ok(vec!["100".into()]));
    let ranged = "SELECT K FROM U WHERE ID > UUID '018bcfe5-687b-7000-8000-000000000000' \
                  AND ID < UUID '018bcfe5-687c-7000-8000-000000000000' ORDER BY K";
    let expected: Vec<String> = (1..=50).map(|k: i32| k.to_string()).collect();
    assert_eq!(rows(&s, ranged), Ok(expected), "the version 7 UUIDs alone, all of the clock's millisecond");
}
