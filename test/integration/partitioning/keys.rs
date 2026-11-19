//! Keys and indexes across partitions.

use turborel::error::Code;

use crate::common::{exec, sorted};

use super::{plan, sales};

/// Verifies: [EXTENSION] RULES.md D157: a PRIMARY KEY holding the
/// partition key is enforced by each partition's unique index; a UNIQUE
/// constraint without it across the partitions; a duplicate in either is
/// refused, SQLCODE -201.
#[test]
fn keys_hold_across_partitions() {
    let (_db, s) = sales();
    exec(&s, "ALTER TABLE SALES ADD CONSTRAINT ONE_ID UNIQUE (ID)").unwrap();
    exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 10)").unwrap();
    assert_eq!(exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 11)"), Err(Code::Constraint));
    assert_eq!(exec(&s, "INSERT INTO SALES VALUES (1, DATE '2026-03-01', 12)"), Err(Code::Constraint));
    exec(&s, "INSERT INTO SALES VALUES (2, DATE '2026-03-01', 12)").unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM SALES"), ["1", "2"]);
}

/// Verifies: [EXTENSION] RULES.md D158: CREATE INDEX on a partitioned
/// table makes the index on each partition, and on one made after; a read
/// of a partition uses it; DROP INDEX drops each.
#[test]
fn indexes_are_made_on_each_partition() {
    let (db, s) = sales();
    exec(&s, "CREATE INDEX BY_AMOUNT ON SALES (AMOUNT)").unwrap();
    exec(&s, "CREATE TABLE S2027 PARTITION OF SALES FOR VALUES FROM (DATE '2027-01-01') TO (DATE '2028-01-01')").unwrap();
    for i in 0..200 {
        exec(&s, &format!("INSERT INTO SALES VALUES ({i}, DATE '2027-02-01', {i})")).unwrap();
    }
    exec(&s, "COMMIT").unwrap();
    let shown = plan(&s, "SELECT ID FROM SALES WHERE AMOUNT = 7 AND SOLD = DATE '2027-02-01'");
    assert!(shown.contains("BY_AMOUNT"), "{shown}");
    assert_eq!(sorted(&s, "SELECT ID FROM SALES WHERE AMOUNT = 7"), ["7"]);
    assert_eq!(db.index_entries("BY_AMOUNT\u{0}S\u{0}S2027").unwrap(), 200);
    assert_eq!(db.index_entries("BY_AMOUNT\u{0}S\u{0}S2025").unwrap(), 0);
    exec(&s, "COMMIT").unwrap();
    exec(&s, "DROP INDEX BY_AMOUNT").unwrap();
    assert!(db.index_entries("BY_AMOUNT\u{0}S\u{0}S2027").is_err());
}

/// Verifies: [EXTENSION] RULES.md D156: a read in an order merges its
/// partitions' rows, each partition's in that order, by its own index or
/// its own sort of its first rows alone, no sort over the merge; the rows
/// come in the order a sort of them all would give.
#[test]
fn ordered_reads_merge_partitions() {
    let (_db, s) = sales();
    let rows: Vec<String> = (0..4000).map(|k| format!("({k}, DATE '{}-02-01', {})", 2025 + (k % 2), (k * 7) % 4000)).collect();
    exec(&s, &format!("INSERT INTO SALES VALUES {}", rows.join(", "))).unwrap();
    exec(&s, "COMMIT").unwrap();
    exec(&s, "CREATE INDEX BY_AMOUNT ON SALES (AMOUNT)").unwrap();
    let q = "SELECT AMOUNT FROM SALES WHERE AMOUNT IS NOT NULL ORDER BY AMOUNT FETCH FIRST 3 ROWS ONLY";
    let shown = plan(&s, q);
    let above = shown.split("MergeAppend").next().unwrap_or_default();
    assert!(shown.contains("MergeAppend") && !above.contains("Sort"), "{shown}");
    assert_eq!(crate::common::rows(&s, q), crate::common::expected(&["0.00", "1.00", "2.00"]));
    let mut by_amount: Vec<i64> = (0..4000).collect();
    by_amount.sort_by_key(|k| std::cmp::Reverse((k * 7) % 4000));
    let ids: Vec<String> = by_amount.iter().map(i64::to_string).collect();
    assert_eq!(crate::common::rows(&s, "SELECT ID FROM SALES ORDER BY AMOUNT DESC"), Ok(ids));
}

/// Verifies: [EXTENSION] RULES.md D153, D158: the Information Schema shows
/// a partitioned table, its partitions with their bounds, and each index a
/// template made on them.
#[test]
fn the_information_schema_shows_trees() {
    let (_db, s) = sales();
    exec(&s, "COMMIT").unwrap();
    exec(&s, "CREATE INDEX BY_AMOUNT ON SALES (AMOUNT)").unwrap();
    assert_eq!(
        sorted(
            &s,
            "SELECT TABLE_NAME || ' ' || STRATEGY || ' ' || KEY_PARTS || ' ' || CAST(PARTITION_COUNT AS VARCHAR(2)) FROM INFORMATION_SCHEMA.PARTITIONED_TABLES"
        ),
        ["SALES RANGE SOLD 3"]
    );
    assert_eq!(
        sorted(&s, "SELECT TABLE_NAME || ' ' || PARENT_TABLE_NAME || ' ' || BOUND FROM INFORMATION_SCHEMA.PARTITIONS"),
        [
            "S2025 SALES FOR VALUES FROM (DATE '2025-01-01') TO (DATE '2026-01-01')",
            "S2026 SALES FOR VALUES FROM (DATE '2026-01-01') TO (DATE '2027-01-01')",
            "SOTHER SALES DEFAULT"
        ]
    );
    assert_eq!(
        sorted(&s, "SELECT PARTITION_NAME FROM INFORMATION_SCHEMA.PARTITION_INDEXES WHERE TEMPLATE_NAME = 'BY_AMOUNT'"),
        ["S2025", "S2026", "SOTHER"]
    );
}
