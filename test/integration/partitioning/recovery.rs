//! Partitioned tables reopened and recovered after a crash.

use std::path::Path;

use turborel::db::{Database, OpenOptions};

use crate::common::{Scratch, after_crash, exec, os, sorted};

/// The database in `dir`, opened as after a crash if `crashed`.
pub(super) fn opened(dir: &Path, crashed: bool) -> Database {
    let db = Database::new();
    let files = if crashed { after_crash() } else { os() };
    db.open_files(&files, dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D159: a partitioned table, its
/// partitions, a template's indexes, a partition attached and one detached
/// are the same after recovery from the log alone and after a checkpoint,
/// their rows routed and read as before.
#[test]
fn trees_are_recovered() {
    let scratch = Scratch::new("partitioning-recovery");
    let dir = scratch.path("db");
    let db = opened(&dir, true);
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    for sql in [
        "CREATE TABLE T (K INTEGER NOT NULL, V INTEGER, PRIMARY KEY (K)) PARTITION BY RANGE (K)",
        "CREATE TABLE T0 PARTITION OF T FOR VALUES FROM (0) TO (100)",
        "CREATE TABLE T1 PARTITION OF T FOR VALUES FROM (100) TO (200)",
        "CREATE TABLE LOOSE (K INTEGER NOT NULL, V INTEGER)",
        "INSERT INTO T VALUES (1, 1), (150, 2)",
        "INSERT INTO LOOSE VALUES (250, 3)",
        "COMMIT",
        "CREATE INDEX BY_V ON T (V)",
        "ALTER TABLE T ATTACH PARTITION LOOSE FOR VALUES FROM (200) TO (300)",
        "ALTER TABLE T DETACH PARTITION T0",
        "COMMIT",
    ] {
        exec(&s, sql).unwrap();
    }
    drop((s, db));
    for _ in 0..2 {
        let db = opened(&dir, true);
        let s = db.session("S");
        assert_eq!(sorted(&s, "SELECT K FROM T"), ["150", "250"]);
        assert_eq!(sorted(&s, "SELECT K FROM T0"), ["1"]);
        assert_eq!(db.index_entries("BY_V\u{0}S\u{0}LOOSE").unwrap(), 1);
        exec(&s, "INSERT INTO T VALUES (210, 4)").unwrap();
        assert_eq!(sorted(&s, "SELECT K FROM LOOSE"), ["210", "250"]);
        exec(&s, "ROLLBACK").unwrap();
        db.checkpoint().unwrap();
    }
}
