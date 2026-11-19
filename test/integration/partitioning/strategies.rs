//! LIST and HASH partitioning, trees of several levels, keys of
//! expressions, and rows no partition holds.

use turborel::error::Code;

use crate::common::{exec, sorted};

use super::plan;

/// Verifies: [EXTENSION] RULES.md D154, D155, D156: a LIST partitioned
/// table routes each value to the partition listing it, NULL among them,
/// refuses a value none lists with SQLSTATE 23T01, and a read of listed
/// values reads their partitions alone; its partition partitioned again by
/// HASH routes each row to the remainder of its key's hash.
#[test]
fn list_and_hash_route_and_prune() {
    let db = turborel::db::Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    for sql in [
        "CREATE TABLE T (REGION CHAR(2), K INTEGER) PARTITION BY LIST (REGION)",
        "CREATE TABLE T_EU PARTITION OF T FOR VALUES IN ('FR', 'DE') PARTITION BY HASH (K)",
        "CREATE TABLE T_EU0 PARTITION OF T_EU FOR VALUES WITH (MODULUS 2, REMAINDER 0)",
        "CREATE TABLE T_EU1 PARTITION OF T_EU FOR VALUES WITH (MODULUS 2, REMAINDER 1)",
        "CREATE TABLE T_US PARTITION OF T FOR VALUES IN ('US', NULL)",
    ] {
        exec(&s, sql).unwrap();
    }
    for k in 0..20 {
        exec(&s, &format!("INSERT INTO T VALUES ('FR', {k})")).unwrap();
    }
    exec(&s, "INSERT INTO T VALUES (NULL, 1), ('US', 2)").unwrap();
    let e = s.execute("INSERT INTO T VALUES ('JP', 3)", &mut turborel::params::Params::new()).unwrap_err();
    assert_eq!((e.code, e.sqlstate().as_str()), (Code::Constraint, "23T01"));
    assert_eq!(sorted(&s, "SELECT COUNT(*) FROM T_EU0").len() + sorted(&s, "SELECT COUNT(*) FROM T_EU1").len(), 2);
    let both: usize = ["T_EU0", "T_EU1"].iter().map(|t| sorted(&s, &format!("SELECT K FROM {t}")).len()).sum();
    assert_eq!(both, 20);
    assert_eq!(sorted(&s, "SELECT K FROM T WHERE REGION IS NULL"), ["1"]);
    let shown = plan(&s, "SELECT K FROM T WHERE REGION = 'US'");
    assert!(shown.contains("T_US") && !shown.contains("T_EU"), "{shown}");
    let shown = plan(&s, "SELECT K FROM T WHERE REGION = 'FR' AND K = 4");
    assert!(!shown.contains("T_US") && (shown.contains("T_EU0") != shown.contains("T_EU1")), "{shown}");
    assert_eq!(sorted(&s, "SELECT K FROM T WHERE REGION = 'FR' AND K = 4"), ["4"]);
}

/// Verifies: [EXTENSION] RULES.md D153: PARTITION BY refuses a column
/// the table lacks and a strategy not registered; FOR VALUES refuses a
/// bound overlapping another's, a second DEFAULT and a DEFAULT of HASH.
#[test]
fn definitions_are_checked() {
    let db = turborel::db::Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    assert_eq!(exec(&s, "CREATE TABLE A (K INTEGER) PARTITION BY RANGE (J)"), Err(Code::SyntaxRule));
    assert_eq!(exec(&s, "CREATE TABLE A (K INTEGER) PARTITION BY BUCKETS (K)"), Err(Code::SyntaxRule));
    exec(&s, "CREATE TABLE R (K INTEGER) PARTITION BY RANGE (K)").unwrap();
    exec(&s, "CREATE TABLE R1 PARTITION OF R FOR VALUES FROM (0) TO (10)").unwrap();
    assert_eq!(exec(&s, "CREATE TABLE R2 PARTITION OF R FOR VALUES FROM (5) TO (20)"), Err(Code::SyntaxRule));
    exec(&s, "CREATE TABLE RD PARTITION OF R DEFAULT").unwrap();
    assert_eq!(exec(&s, "CREATE TABLE RE PARTITION OF R DEFAULT"), Err(Code::SyntaxRule));
    exec(&s, "CREATE TABLE H (K INTEGER) PARTITION BY HASH (K)").unwrap();
    assert_eq!(exec(&s, "CREATE TABLE HD PARTITION OF H DEFAULT"), Err(Code::SyntaxRule));
    assert_eq!(exec(&s, "CREATE TABLE H0 PARTITION OF H FOR VALUES WITH (MODULUS 2, REMAINDER 2)"), Err(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] RULES.md D153, D155: a key of an expression
/// routes each row by its value, and an UPDATE changing it moves the row;
/// one that can raise an error is refused.
#[test]
fn expression_keys_route_rows() {
    let db = turborel::db::Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    assert_eq!(exec(&s, "CREATE TABLE BAD (K INTEGER) PARTITION BY RANGE ((K / 10))"), Err(Code::SyntaxRule));
    exec(&s, "CREATE TABLE E (C VARCHAR(5)) PARTITION BY LIST ((UPPER(C)))").unwrap();
    exec(&s, "CREATE TABLE EA PARTITION OF E FOR VALUES IN ('A')").unwrap();
    exec(&s, "CREATE TABLE EB PARTITION OF E FOR VALUES IN ('B')").unwrap();
    exec(&s, "INSERT INTO E VALUES ('a'), ('B')").unwrap();
    assert_eq!(sorted(&s, "SELECT C FROM EA"), ["a"]);
    exec(&s, "UPDATE E SET C = 'b' WHERE C = 'a'").unwrap();
    assert_eq!(sorted(&s, "SELECT C FROM EB"), ["B", "b"]);
    assert_eq!(sorted(&s, "SELECT C FROM EA"), Vec::<String>::new());
}
