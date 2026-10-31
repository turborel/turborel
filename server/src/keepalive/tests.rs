//! Unit tests of TCP keepalive's times read from the settings (RULES.md
//! R-TEST-1); `transport/tests.rs` sets them on sockets.

use std::time::Duration;

use turborel::db::Database;
use turborel::error::Code;
use turborel::value::{Decimal, Value};

use super::{Keepalive, together};
use crate::settings::{self, KEEPALIVE_IDLE_MS, KEEPALIVE_INTERVAL_MS, KEEPALIVE_PROBES};

/// A database with the server's settings declared.
fn declared() -> Database {
    let db = Database::new();
    settings::declare(&db).unwrap();
    db
}

/// Sets `db`'s whole-number setting `name` to `n`.
fn set(db: &Database, name: &str, n: i128) -> turborel::error::Result<()> {
    db.set_setting(name, &Value::Exact(Decimal::new(n, 0)))
}

/// Verifies: [DESIGN] RULES.md D42: SERVER.KEEPALIVE_IDLE_MS,
/// SERVER.KEEPALIVE_INTERVAL_MS and SERVER.KEEPALIVE_PROBES are 0 by
/// default, the system's own times; given, a time is rounded up to whole
/// seconds, as the systems count them, and each is read apart.
#[test]
fn the_settings_give_the_times() {
    let db = declared();
    assert_eq!(Keepalive::of(&db), Keepalive::default(), "the system's own");
    set(&db, KEEPALIVE_IDLE_MS, 1_500_i128).unwrap();
    assert_eq!(Keepalive::of(&db), Keepalive { idle: Some(Duration::from_secs(2_u64)), ..Keepalive::default() });
    set(&db, KEEPALIVE_INTERVAL_MS, 1_i128).unwrap();
    set(&db, KEEPALIVE_PROBES, 4_i128).unwrap();
    let all = Keepalive { idle: Some(Duration::from_secs(2_u64)), interval: Some(Duration::from_secs(1_u64)), probes: Some(4_u32) };
    assert_eq!(Keepalive::of(&db), all);
    set(&db, KEEPALIVE_IDLE_MS, 0_i128).unwrap();
    assert_eq!(Keepalive::of(&db), Keepalive { idle: None, ..all }, "0 is the system's own again");
}

/// Verifies: [DESIGN] RULES.md D42: a time past Linux's most (32767
/// seconds), or probes past its 127, is refused as any setting out of its
/// range is (SQLCODE -910); the most is taken.
#[test]
fn the_times_are_bounded() {
    let db = declared();
    for name in [KEEPALIVE_IDLE_MS, KEEPALIVE_INTERVAL_MS] {
        assert_eq!(set(&db, name, 32_767_001_i128).err().map(|e| e.code), Some(Code::Api), "{name}");
        assert_eq!(set(&db, name, -1_i128).err().map(|e| e.code), Some(Code::Api), "{name}");
        set(&db, name, 32_767_000_i128).unwrap();
    }
    assert_eq!(set(&db, KEEPALIVE_PROBES, 128_i128).err().map(|e| e.code), Some(Code::Api));
    set(&db, KEEPALIVE_PROBES, 127_i128).unwrap();
    assert_eq!(Keepalive::of(&db).probes, Some(127_u32));
}

/// Verifies: [DESIGN] RULES.md D42: Windows sets the idle time and the
/// interval together, so one given, the other is Windows' own default (2
/// hours idle, 1 second between probes); neither given, neither is set.
#[cfg(windows)]
#[test]
fn windows_sets_idle_and_interval_together() {
    let (idle, interval) = (Some(Duration::from_secs(5_u64)), Some(Duration::from_secs(3_u64)));
    assert_eq!(together(None, None), (None, None));
    assert_eq!(together(idle, None), (idle, Some(Duration::from_secs(1_u64))));
    assert_eq!(together(None, interval), (Some(Duration::from_secs(7_200_u64)), interval));
    assert_eq!(together(idle, interval), (idle, interval));
}

/// Verifies: [DESIGN] RULES.md D42: elsewhere than Windows, the idle time
/// and the interval are set apart, each only if given.
#[cfg(not(windows))]
#[test]
fn the_idle_time_and_interval_are_set_apart() {
    let (idle, interval) = (Some(Duration::from_secs(5_u64)), Some(Duration::from_secs(3_u64)));
    assert_eq!(together(None, None), (None, None));
    assert_eq!(together(idle, None), (idle, None));
    assert_eq!(together(None, interval), (None, interval));
}
