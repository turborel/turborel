//! \[EXTENSION\] A partitioning strategy of an extension (RULES.md D169),
//! the example plug-in's MODULUS (`plugin/examples/plugin_example`),
//! asked of bounds of every form, and asked which partitions a read must
//! read under restrictions of every form.

use turborel::params::Params;

use crate::common::sorted;
use crate::plugins::fixture::plugged;

/// Verifies: [EXTENSION] RULES.md D169: a read restricted by a range,
/// open or closed, inclusive or not, or by IS NULL, asks the strategy
/// which partitions it admits, each bound and restriction carried whole,
/// and returns the rows a read of every partition returns, a DEFAULT
/// partition read whatever the strategy says; a read of keys equal to
/// values is routed to their partitions alone.
#[test]
fn a_strategy_is_asked_of_each_restriction() {
    let (_scratch, db) = plugged("plugin-partitions-admits");
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    s.run("CREATE TABLE T (K INTEGER, V VARCHAR(10)) PARTITION BY MODULUS (K)").unwrap();
    for r in 0..4 {
        s.run(&format!("CREATE TABLE T{r} PARTITION OF T FOR VALUES WITH (MODULUS 4, REMAINDER {r})")).unwrap();
    }
    s.run("CREATE TABLE TD PARTITION OF T DEFAULT").unwrap();
    s.run("INSERT INTO T VALUES (1, 'a'), (2, 'b'), (4, 'c'), (5, 'd'), (NULL, 'n')").unwrap();
    let cases: [(&str, &[&str]); 5] = [
        ("K > 2", &["4", "5"]),
        ("K >= 2 AND K < 5", &["2", "4"]),
        ("K <= 2", &["1", "2"]),
        ("K BETWEEN 1 AND 4", &["1", "2", "4"]),
        ("K IS NULL", &["NULL"]),
    ];
    for (cond, want) in cases {
        let query = format!("SELECT K FROM T WHERE {cond}");
        assert_eq!(sorted(&s, &query), want, "{cond}");
        let plan = s.physical_plan(&query, &Params::new()).unwrap();
        assert!(plan.contains("TD"), "the default partition is read: {plan}");
    }
    let routed = "SELECT K FROM T WHERE K IN (1, 5) OR K = 4";
    assert_eq!(sorted(&s, routed), ["1", "4", "5"]);
    let plan = s.physical_plan(routed, &Params::new()).unwrap();
    assert!(plan.contains("T0") && plan.contains("T1") && !plan.contains("TD"), "keys routed, not admitted: {plan}");
}

/// Verifies: [EXTENSION] RULES.md D169, D172: a partition's bound of a
/// range of values, MINVALUE and MAXVALUE, or of a list, is carried to
/// the strategy, which refuses each as no bound of its own, and the
/// partition is not made.
#[test]
fn a_strategy_refuses_bounds_of_other_forms() {
    let (_scratch, db) = plugged("plugin-partitions-bounds");
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    s.run("CREATE TABLE T (K INTEGER) PARTITION BY MODULUS (K)").unwrap();
    for bound in ["FROM (MINVALUE) TO (5)", "FROM (5) TO (MAXVALUE)", "IN (1, 2)"] {
        let made = s.run(&format!("CREATE TABLE P PARTITION OF T FOR VALUES {bound}"));
        assert!(made.is_err(), "{bound}");
    }
    let count = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'P'";
    assert_eq!(sorted(&s, count), ["0"]);
}
