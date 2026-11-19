//! Unit tests of a pool that starts no program: what it refuses before a
//! process is started, and what it counts (RULES.md R-TEST-1, R-TEST-2).
//! A pool of a program that runs is `test/integration/plugins.rs`'s.

use super::*;
use crate::plugin::limits::Told;

/// A directory of its own for the test `test`, emptied.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("turborel-pool-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ext")).unwrap();
    dir
}

/// A pool of the program `missing` in `dir`/ext, PLUGINS.DIRECTORIES
/// `allowed`.
fn pool(dir: &std::path::Path, allowed: &str) -> Arc<Pool> {
    let limits = Arc::new(Limits::default());
    limits.tell(&Told { directories: allowed.to_owned(), ..Told::default() });
    let host = (limits, MemoryBudget::new(1 << 20));
    Arc::new(Pool::new("E", (dir.join("ext"), PathBuf::from("missing")), vec![1], host))
}

/// A call of a scalar function of no rows.
fn call(p: &Arc<Pool>) -> Result<Vec<u8>> {
    p.call(&Request { kind: 1, op: 1, object: "F", payload: vec![0, 0, 0, 0] }, &mut CallContext { interrupt: None, sql: None })
}

/// The SQLSTATE of `made`'s failure.
fn state<T>(made: Result<T>) -> String {
    made.err().and_then(|e| e.state).map(|s| s.as_str().to_owned()).unwrap_or_default()
}

/// Verifies: [DESIGN] RULES.md D173: a pool whose directory is under none
/// of PLUGINS.DIRECTORIES starts nothing and fails with 38T05; nor does
/// one whose check of its directory fails, which fails with the check's
/// error; a program that cannot be started is 38T05, its place freed, so
/// the next call fails the same and none runs.
#[test]
fn a_pool_starts_nothing_it_may_not() {
    let dir = scratch("refused");
    assert_eq!(state(call(&pool(&dir, ""))), "38T05", "no directory allowed");
    let checked = pool(&dir, &dir.display().to_string());
    let refusing: crate::plugin::host::Check = Arc::new(|_| Err(violation("refused by its check")));
    let checked = Arc::new(Arc::into_inner(checked).unwrap().with_check(refusing));
    assert_eq!(state(call(&checked)), "38T03");
    let p = pool(&dir, &dir.display().to_string());
    for _ in 0..2_u8 {
        assert_eq!(state(call(&p)), "38T05", "missing");
    }
    let info = p.info();
    assert_eq!((info.running, info.started, info.calls), (0, 0, 0));
    assert_eq!(p.extension(), "E");
    let _ = std::fs::remove_dir_all(&dir);
}
