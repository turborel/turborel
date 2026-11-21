//! The plans of TPC-H's queries 9 and 21 over a scaled-down database, as
//! the statistics of its tables are gathered one at a time (RULES.md
//! D655, D657, D658): a filter of a table with rows is never estimated to keep
//! none, the one nation of query 21 is joined before its orders, and the
//! green parts of query 9 are filtered and hashed, never probed for each
//! line. These are `[DESIGN]` requirements on the plan; the standard
//! governs only the rows.

use turborel::db::{Database, Session};
use turborel::params::Params;

mod data;

use data::{ALL, Scale};

/// Query 9 as the benchmark writes it.
const Q09: &str = "SELECT NATION, O_YEAR, SUM(AMOUNT) AS SUM_PROFIT FROM (
      SELECT N_NAME AS NATION, EXTRACT(YEAR FROM O_ORDERDATE) AS O_YEAR,
             L_EXTENDEDPRICE * (1 - L_DISCOUNT) - PS_SUPPLYCOST * L_QUANTITY AS AMOUNT
      FROM PART, SUPPLIER, LINEITEM, PARTSUPP, ORDERS, NATION
      WHERE S_SUPPKEY = L_SUPPKEY AND PS_SUPPKEY = L_SUPPKEY AND PS_PARTKEY = L_PARTKEY AND P_PARTKEY = L_PARTKEY
        AND O_ORDERKEY = L_ORDERKEY AND S_NATIONKEY = N_NATIONKEY AND P_NAME LIKE '%green%') AS PROFIT
    GROUP BY NATION, O_YEAR ORDER BY NATION, O_YEAR DESC";

/// Query 21 as the benchmark writes it.
const Q21: &str = "SELECT S_NAME, COUNT(*) AS NUMWAIT FROM SUPPLIER, LINEITEM L1, ORDERS, NATION
    WHERE S_SUPPKEY = L1.L_SUPPKEY AND O_ORDERKEY = L1.L_ORDERKEY AND O_ORDERSTATUS = 'F'
      AND L1.L_RECEIPTDATE > L1.L_COMMITDATE
      AND EXISTS (SELECT * FROM LINEITEM L2 WHERE L2.L_ORDERKEY = L1.L_ORDERKEY AND L2.L_SUPPKEY <> L1.L_SUPPKEY)
      AND NOT EXISTS (SELECT * FROM LINEITEM L3 WHERE L3.L_ORDERKEY = L1.L_ORDERKEY AND L3.L_SUPPKEY <> L1.L_SUPPKEY
                      AND L3.L_RECEIPTDATE > L3.L_COMMITDATE)
      AND S_NATIONKEY = N_NATIONKEY AND N_NAME = 'SAUDI ARABIA'
    GROUP BY S_NAME ORDER BY NUMWAIT DESC, S_NAME FETCH FIRST 100 ROWS ONLY";

/// A thousandth of the benchmark's first scale, of more suppliers, so a
/// nation holds several.
const SCALE: Scale = Scale { suppliers: 100, parts: 2_000, orders: 3_000 };

/// The line of `plan` at which a join, an operator whose name ends in
/// Join or Loop, first names `key`, counted from its top.
fn join_naming(plan: &str, key: &str) -> Option<usize> {
    plan.lines().position(|l| {
        let op = l.split_whitespace().next().unwrap_or("");
        (op.ends_with("Join") || op.ends_with("Loop")) && l.contains(key)
    })
}

/// The filters of `plan` estimated to keep no rows.
fn kept_none(plan: &str) -> Vec<&str> {
    plan.lines().filter(|l| l.trim_start().starts_with("Filter") && l.contains("(est. rows=0)")).collect()
}

/// Checks the plans of queries 9 and 21 of `s`, the statistics of the
/// tables `gathered`.
fn check(s: &Session, gathered: &[&str]) {
    let q21 = s.physical_plan(Q21, &Params::new()).unwrap();
    assert!(kept_none(&q21).is_empty(), "q21, {gathered:?} gathered:\n{q21}");
    let nation = join_naming(&q21, "N_NATIONKEY").unwrap_or_else(|| panic!("q21 joins NATION:\n{q21}"));
    let orders = join_naming(&q21, "O_ORDERKEY").unwrap_or_else(|| panic!("q21 joins ORDERS:\n{q21}"));
    assert!(nation > orders, "q21, {gathered:?} gathered, joins NATION before ORDERS:\n{q21}");
    let q09 = s.physical_plan(Q09, &Params::new()).unwrap();
    assert!(kept_none(&q09).is_empty(), "q09, {gathered:?} gathered:\n{q09}");
    let probed = ["IndexNestedLoop TPCH.PART ", "IndexNestedLoop TPCH.PARTSUPP "];
    assert!(!probed.iter().any(|p| q09.contains(p)), "q09, {gathered:?} gathered, hashes PART and PARTSUPP:\n{q09}");
    let part = join_naming(&q09, "P_PARTKEY").unwrap_or_else(|| panic!("q09 joins PART:\n{q09}"));
    assert!(q09.lines().nth(part).is_some_and(|l| l.contains("HashJoin")), "q09, {gathered:?} gathered:\n{q09}");
}

/// Gathers the statistics of `db`'s tables one at a time in `order`,
/// checking the plans before the first and after each.
fn gathered_in(db: &Database, order: &[&str]) {
    let s = db.session("TPCH");
    check(&s, &[]);
    for (at, table) in order.iter().enumerate() {
        db.analyze(&format!("TPCH.{table}")).unwrap();
        check(&s, &order[..=at]);
    }
}

/// Verifies: [DESIGN] RULES.md D655: of no statistics, of some tables'
/// and of all, no filter of a table with rows is estimated to keep none,
/// the nation of query 21 is joined before the orders, and query 9 hashes
/// its green parts and their suppliers' offers. Regression: the filter of
/// one nation of 25 was estimated to keep no rows with no statistics, and
/// joined last. The tables are gathered in the order they are filled.
#[test]
fn tpch_plans_hold_as_statistics_are_gathered() {
    gathered_in(&data::load(&SCALE), ALL);
}

/// Verifies: [DESIGN] RULES.md D655: as
/// `tpch_plans_hold_as_statistics_are_gathered`, the tables gathered the
/// other way round, the largest first.
#[test]
fn tpch_plans_hold_as_statistics_are_gathered_largest_first() {
    let order: Vec<&str> = ALL.iter().rev().copied().collect();
    gathered_in(&data::load(&SCALE), &order);
}
