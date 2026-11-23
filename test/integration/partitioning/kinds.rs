//! Partitioned tables that are system-versioned, temporary, of the
//! column store, read through views, under privileges, and through
//! prepared statements.

use turborel::error::Code;
use turborel::value::Value;

use crate::common::{exec, sorted};

use super::plan;

/// Verifies: [EXTENSION] RULES.md D159: a system-versioned partitioned
/// table keeps each partition's historical rows in its own history; FOR
/// SYSTEM_TIME reads them all, a row an UPDATE moved to another partition
/// among them, as of before it moved; a table attached must be versioned
/// as the tree is.
#[test]
fn system_versioned_trees_keep_each_partitions_history() {
    let db = turborel::db::Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    for sql in [
        "CREATE TABLE A (ID INTEGER, R CHAR(1), S TIMESTAMP(6) GENERATED ALWAYS AS ROW START, \
         E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, PERIOD FOR SYSTEM_TIME (S, E)) WITH SYSTEM VERSIONING PARTITION BY LIST (R)",
        "CREATE TABLE AX PARTITION OF A FOR VALUES IN ('X')",
        "CREATE TABLE AY PARTITION OF A FOR VALUES IN ('Y')",
        "INSERT INTO A (ID, R) VALUES (1, 'X'), (2, 'Y')",
        "COMMIT",
    ] {
        exec(&s, sql).unwrap();
    }
    let before = sorted(&s, "SELECT CAST(S AS VARCHAR(40)) FROM AX").remove(0);
    exec(&s, "UPDATE A SET R = 'Y' WHERE ID = 1").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM AY"), ["1", "2"]);
    let at = format!("SELECT CAST(ID AS VARCHAR(2)) || R FROM A FOR SYSTEM_TIME AS OF TIMESTAMP '{before}'");
    assert_eq!(sorted(&s, &at), ["1X", "2Y"]);
    assert_eq!(
        sorted(
            &s,
            "SELECT CAST(ID AS VARCHAR(2)) || R FROM A FOR SYSTEM_TIME FROM TIMESTAMP '2000-01-01 00:00:00' TO TIMESTAMP '9999-01-01 00:00:00'"
        ),
        ["1X", "1Y", "2Y"]
    );
    exec(&s, "CREATE TABLE AZ (ID INTEGER, R CHAR(1), S TIMESTAMP(6), E TIMESTAMP(6))").unwrap();
    assert_eq!(exec(&s, "ALTER TABLE A ATTACH PARTITION AZ FOR VALUES IN ('Z')"), Err(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] RULES.md D153: a global temporary table is
/// partitioned with partitions of its own kind, each session's rows its
/// own; a permanent table is no partition of it.
#[test]
fn temporary_tables_are_partitioned() {
    let db = turborel::db::Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    exec(&s, "CREATE GLOBAL TEMPORARY TABLE G (K INTEGER) ON COMMIT PRESERVE ROWS PARTITION BY RANGE (K)").unwrap();
    s.execute(
        "CREATE GLOBAL TEMPORARY TABLE G0 PARTITION OF G FOR VALUES FROM (0) TO (10) ON COMMIT PRESERVE ROWS",
        &mut turborel::params::Params::new(),
    )
    .unwrap();
    exec(&s, "CREATE TABLE P (K INTEGER)").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(exec(&s, "ALTER TABLE G ATTACH PARTITION P FOR VALUES FROM (10) TO (20)"), Err(Code::SyntaxRule));
    exec(&s, "ROLLBACK").unwrap();
    exec(&s, "INSERT INTO G VALUES (3)").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(sorted(&s, "SELECT K FROM G"), ["3"]);
    let other = db.session("S");
    assert_eq!(sorted(&other, "SELECT K FROM G"), Vec::<String>::new());
}

/// Verifies: [EXTENSION] RULES.md D157: a temporary tree has no global
/// index, and a UNIQUE key without its partition key is checked across its
/// partitions.
#[test]
fn temporary_trees_check_keys_across_partitions() {
    let db = turborel::db::Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    for sql in [
        "CREATE GLOBAL TEMPORARY TABLE G (ID INTEGER, K INTEGER, UNIQUE (ID)) ON COMMIT PRESERVE ROWS PARTITION BY RANGE (K)",
        "CREATE GLOBAL TEMPORARY TABLE G0 PARTITION OF G FOR VALUES FROM (0) TO (10) ON COMMIT PRESERVE ROWS",
        "CREATE GLOBAL TEMPORARY TABLE G1 PARTITION OF G FOR VALUES FROM (10) TO (20) ON COMMIT PRESERVE ROWS",
        "COMMIT",
        "INSERT INTO G VALUES (1, 3)",
    ] {
        exec(&s, sql).unwrap();
    }
    assert_eq!(exec(&s, "INSERT INTO G VALUES (1, 13)"), Err(Code::Constraint));
    exec(&s, "INSERT INTO G VALUES (2, 13)").unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM G"), ["1", "2"]);
}

/// Verifies: [EXTENSION] RULES.md D153: a partition is of its own engine,
/// the column store among them, its rows read through its tree.
#[test]
fn partitions_choose_their_engine() {
    let db = turborel::db::Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    exec(&s, "CREATE TABLE T (K INTEGER, V INTEGER) PARTITION BY RANGE (K)").unwrap();
    exec(&s, "CREATE TABLE T0 PARTITION OF T FOR VALUES FROM (0) TO (100)").unwrap();
    exec(&s, "CREATE TABLE T1 PARTITION OF T FOR VALUES FROM (100) TO (200) USING columnstore").unwrap();
    exec(&s, "INSERT INTO T VALUES (1, 1), (150, 2)").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(sorted(&s, "SELECT SUM(V) FROM T"), ["3"]);
    assert_eq!(
        sorted(&s, "SELECT ENGINE_NAME FROM INFORMATION_SCHEMA.TABLE_OPTIONS WHERE TABLE_NAME = 'T1' FETCH FIRST 1 ROW ONLY"),
        ["columnstore"]
    );
}

/// Verifies: [EXTENSION] RULES.md D153, D156: a view of a partitioned
/// table reads its partitions, pruned by the view's and the query's
/// conditions; a user granted SELECT on the partitioned table reads its
/// rows through it, not the partitions; a prepared statement reads the
/// partitions its parameters' values need at each execution.
#[test]
fn views_privileges_and_parameters_reach_the_partitions() {
    let (db, s) = super::sales();
    exec(&s, "INSERT INTO SALES VALUES (1, DATE '2025-03-01', 10), (2, DATE '2026-03-01', 20)").unwrap();
    exec(&s, "CREATE VIEW RECENT AS SELECT ID, SOLD FROM SALES WHERE SOLD >= DATE '2026-01-01'").unwrap();
    exec(&s, "GRANT SELECT ON SALES TO U").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(sorted(&s, "SELECT ID FROM RECENT"), ["2"]);
    let shown = plan(&s, "SELECT ID FROM RECENT");
    assert!(shown.contains("S2026") && !shown.contains("S2025"), "{shown}");
    let u = db.session("U");
    assert_eq!(sorted(&u, "SELECT ID FROM S.SALES"), ["1", "2"]);
    assert!(exec(&u, "SELECT ID FROM S.S2025").is_err());
    let p = s.prepare("SELECT ID FROM SALES WHERE SOLD = CAST(? AS DATE)").unwrap();
    for (day, want) in [("2025-03-01", "1"), ("2026-03-01", "2"), ("2025-03-01", "1"), ("2026-03-01", "2"), ("2024-01-01", "")] {
        let mut c = s.open_prepared(&p, &[Value::Char(day.into())]).unwrap();
        let got = c.fetch().unwrap().map(|r| r[0].to_string()).unwrap_or_default();
        assert_eq!(got, want, "{day}");
    }
}
