//! Tests of the price of a fetch as the pool warms: a table the pool holds
//! is priced as held, from its first reads after a start (RULES.md D1230).

use std::sync::Arc;

use crate::db::{Database, OpenOptions, Session};
use crate::disk::io::FileSystem;
use crate::disk::memory::MemoryFileSystem;
use crate::params::Params;
use crate::value::{Decimal, Value};

/// The database of schema W on `fs`, opened with a pool of 64 MiB, its
/// statistics gathered only as a test asks.
fn opened(fs: &Arc<dyn FileSystem>) -> Database {
    let db = Database::new();
    db.set_job_policy(crate::threads::JobPolicy::Manual).unwrap();
    db.set_setting("STORAGE.POOL_BYTES", &Value::Exact(Decimal::new(64 << 20, 0))).unwrap();
    db.open_files(fs, std::path::Path::new("/w"), &OpenOptions::default()).unwrap();
    db
}

/// T of 100,000 rows of a key, a column G indexed by T_G and a string of
/// 98 characters, a few megabytes the pool holds many times over, made on
/// `fs` and closed.
fn made(fs: &Arc<dyn FileSystem>) {
    let db = opened(fs);
    db.schema("CREATE SCHEMA AUTHORIZATION W CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, G INTEGER NOT NULL, S VARCHAR(100) NOT NULL)")
        .unwrap();
    let s = db.session("W");
    let pad = "x".repeat(90);
    for chunk in (0..100_000_usize).collect::<Vec<_>>().chunks(1_000) {
        let rows = chunk.iter().map(|i| format!("({i}, {}, '{pad}{i:08}')", (i * 7_919) % 100_000)).collect::<Vec<_>>().join(", ");
        s.run(&format!("INSERT INTO T VALUES {rows}")).unwrap();
    }
    s.commit().unwrap();
    db.create_index("T_G", "W.T", &["G"], "btree").unwrap();
    db.close().unwrap();
}

/// How `s` reads T for a range of G of about 4,400 rows: the line of
/// its scan.
fn read(s: &Session) -> String {
    let plan = s.physical_plan("SELECT S FROM T WHERE G < 5000", &Params::new()).unwrap();
    plan.lines().filter(|l| l.contains("Scan")).map(|l| l.trim().split("  (").next().unwrap_or("").to_owned()).collect()
}

/// Verifies: [DESIGN] RULES.md D1230, D35: a range of a table the pool
/// holds is read through its index after a start, after the sampled
/// statistics pass that first reads the table into the empty pool, and
/// once warm, the same plan in each. Regression test (R-TEST-7): the
/// pass's first reads were counted as misses, and the range was planned
/// as a scan of the whole table until a few thousand lookups later.
#[test]
fn a_table_the_pool_holds_is_planned_the_same_cold_and_warm() {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    made(&fs);
    let db = opened(&fs);
    let opened_plan = read(&db.session("W"));
    assert!(opened_plan.contains("using T_G"), "{opened_plan}");
    db.analyze("W.T").unwrap();
    let s = db.session("W");
    assert_eq!(read(&s), opened_plan, "after the statistics pass");
    let rows = |sql: &str| {
        let mut cursor = s.open_cursor(sql).unwrap();
        while cursor.fetch().unwrap().is_some() {}
    };
    rows("SELECT COUNT(*) FROM T WHERE S > ''");
    for k in 0_i32..2_000_i32 {
        rows(&format!("SELECT S FROM T WHERE G = {}", k * 41_i32));
    }
    assert_eq!(read(&s), opened_plan, "warm");
}
