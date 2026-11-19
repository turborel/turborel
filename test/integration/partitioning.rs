//! \[EXTENSION\] Partitioned tables (RULES.md D153 to D159): their
//! definitions, rows routed to the partitions that hold them, reads pruned
//! to the partitions they need, keys and indexes across the partitions,
//! ATTACH and DETACH, and the strategies an embedding program registers.

mod attach;
mod basics;
mod constraints;
mod contract;
mod cursors;
mod drops;
mod global;
mod hybrid;
mod joins;
mod keys;
mod kinds;
mod methods;
mod recovery;
mod strategies;

use turborel::db::{Database, Session};
use turborel::params::Params;

use crate::common::exec;

/// A database whose schema S owns SALES, partitioned by RANGE of its
/// SOLD, of partitions S2025 and S2026 and a DEFAULT, SOTHER; and the
/// session of S.
fn sales() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    for sql in [
        "CREATE TABLE SALES (ID INTEGER NOT NULL, SOLD DATE NOT NULL, AMOUNT DECIMAL(9,2), PRIMARY KEY (ID, SOLD)) PARTITION BY RANGE (SOLD)",
        "CREATE TABLE S2025 PARTITION OF SALES FOR VALUES FROM (DATE '2025-01-01') TO (DATE '2026-01-01')",
        "CREATE TABLE S2026 PARTITION OF SALES FOR VALUES FROM (DATE '2026-01-01') TO (DATE '2027-01-01')",
        "CREATE TABLE SOTHER PARTITION OF SALES DEFAULT",
    ] {
        exec(&s, sql).unwrap();
    }
    exec(&s, "COMMIT").unwrap();
    (db, s)
}

/// The physical plan of `sql`, as EXPLAIN shows it.
fn plan(s: &Session, sql: &str) -> String {
    let shown = s.explain(sql, &Params::new()).unwrap();
    shown.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}
