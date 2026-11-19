//! Unit tests: a registered XQuery module's timestamps (RULES.md
//! R-TEST-1), which no view shows.

use std::sync::Arc;

use crate::catalog::{QualifiedName, Stamps};
use crate::clock::FixedClock;
use crate::db::{Database, Standby};
use crate::log::{Log, Lsn, MemoryLog};
use crate::value::datetime::{Now, utc_of_unix};

/// 2024-02-29 12:34:56.78 UTC, in microseconds from 1970-01-01.
const T1: i64 = 1_709_210_096_780_000;
/// A minute later.
const T2: i64 = T1 + 60_000_000;

/// A library module of the namespace `urn:m`, declaring `name`.
fn module(name: &str) -> String {
    format!("CREATE XQUERY MODULE M AS 'module namespace m = ''urn:m''; declare function m:{name}() {{ 1 }};'")
}

/// The timestamps the catalog of `db` keeps of A.M.
fn stamps(db: &Database) -> Stamps {
    let qn = QualifiedName { auth: "A".into(), ident: "M".into() };
    db.snapshot().unwrap().catalog.xquery_module(&qn).unwrap().stamps
}

/// Verifies: [EXTENSION] RULES.md D106, D37 (Feature T011): a registered
/// XQuery module keeps CREATE XQUERY MODULE's CURRENT_TIMESTAMP as both
/// its timestamps, and ALTER XQUERY MODULE moves the last-altered one to
/// its own, the creation one kept [ISO/IEC 9075-2:2023 §11.60 GR5, §11.61
/// GR4]; a standby replaying the log keeps them, its clock a day later.
#[test]
fn alter_xquery_module_moves_last_altered_alone() {
    let log = Arc::new(MemoryLog::new());
    let db = Database::new();
    let clock = Arc::new(FixedClock::at(T1));
    let (read, logged): (Arc<dyn crate::clock::Clock>, Arc<dyn Log>) = (clock.clone(), log.clone());
    db.set_clock(read).unwrap();
    db.set_log(logged).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    a.run(&module("ONE")).unwrap();
    a.commit().unwrap();
    let (t1, t2) = (Now { utc: utc_of_unix(T1), zone: 0 }, Now { utc: utc_of_unix(T2), zone: 0 });
    assert_eq!(stamps(&db), Stamps::at(t1));
    clock.set(T2);
    a.run(&module("TWO").replacen("CREATE", "ALTER", 1)).unwrap();
    a.commit().unwrap();
    assert_eq!(stamps(&db), Stamps { created: t1, last_altered: t2 });

    let standby = Database::new();
    standby.set_clock(Arc::new(FixedClock::at(T2 + 86_400_000_000))).unwrap();
    standby.set_log(Arc::new(MemoryLog::new())).unwrap();
    Standby::replay(&standby, &mut log.read_from(Lsn(0))).unwrap();
    assert_eq!(stamps(&standby), stamps(&db));
}
