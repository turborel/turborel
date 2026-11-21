//! How fast correlated subqueries run unnested (RULES.md D310 to D319)
//! and nested, each run for each binding of its outer references, the
//! rules OPTIMIZER.SEMI_JOINS, OPTIMIZER.MARK_JOINS and
//! OPTIMIZER.UNNEST_SUBQUERIES on and off: over TPC-H-shaped tables of
//! 2,000 orders and 8,000 line items, the subqueries of queries 2, 4, 17,
//! 21 and 22, a join beside a correlated subquery, a scalar subquery of a
//! select list, NOT IN, a SQL function whose body is a correlated
//! subquery, and EXISTS under OR, IN and ALL in a select list, which mark
//! joins take (RULES.md D641, R-BENCH-1).

use super::harness::{Bench, Case, scatter};
use turborel::db::{Database, Session};
use turborel::optimizer::{INLINE_FUNCTIONS, MARK_JOINS, SEMI_JOINS, UNNEST_SUBQUERIES, switch_name};
use turborel::value::Value;

/// Orders.
const ORDERS: u64 = 2_000;
/// Line items: four for each order.
const LINES: u64 = 4 * ORDERS;
/// Parts and suppliers.
const PARTS: u64 = 400;
/// Customers.
const CUSTOMERS: u64 = 500;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 500;

/// The tables, filled.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
         CREATE TABLE PARTSUPP (PS_PARTKEY INTEGER, PS_SUPPKEY INTEGER, PS_SUPPLYCOST INTEGER)
         CREATE TABLE CUSTOMER (C_CUSTKEY INTEGER, C_ACCTBAL INTEGER)
         CREATE TABLE ORDERS (O_ORDERKEY INTEGER, O_CUSTKEY INTEGER, O_ORDERDATE INTEGER)
         CREATE TABLE LINEITEM (L_ORDERKEY INTEGER, L_PARTKEY INTEGER, L_SUPPKEY INTEGER, L_QUANTITY INTEGER,
             L_COMMITDATE INTEGER, L_RECEIPTDATE INTEGER)
         CREATE FUNCTION ORDERED (C INTEGER) RETURNS INTEGER DETERMINISTIC READS SQL DATA
             RETURN (SELECT COUNT(*) FROM ORDERS WHERE O_CUSTKEY = C)",
    )
    .unwrap();
    let s = db.session("B");
    let fill = |table: &str, count: u64, row: &dyn Fn(u64) -> String| {
        for start in (0..count).step_by(PER_INSERT as usize) {
            let rows: Vec<String> = (start..count.min(start + PER_INSERT)).map(row).collect();
            s.run(&format!("INSERT INTO {table} VALUES {}", rows.join(", "))).unwrap();
        }
    };
    fill("PARTSUPP", 4 * PARTS, &|n| format!("({}, {}, {})", n % PARTS, scatter(n, PARTS), scatter(n, 1000)));
    fill("CUSTOMER", CUSTOMERS, &|n| format!("({n}, {})", i64::try_from(scatter(n, 2000)).unwrap() - 500));
    fill("ORDERS", ORDERS, &|n| format!("({n}, {}, {})", scatter(n, CUSTOMERS * 2 / 3), scatter(n, 365)));
    fill("LINEITEM", LINES, &|n| {
        let commit = scatter(n, 365);
        format!(
            "({}, {}, {}, {}, {commit}, {})",
            n / 4,
            scatter(n, PARTS),
            scatter(n + 7, PARTS),
            1 + scatter(n, 50),
            i64::try_from(commit + scatter(n, 30)).unwrap() - 10
        )
    });
    s.commit().unwrap();
    db
}

/// The queries measured, and the rows each counts its rate by.
const QUERIES: &[(&str, &str, u64)] = &[
    (
        "Q2 scalar MIN of the row's part",
        "SELECT COUNT(*) FROM PARTSUPP P WHERE PS_SUPPLYCOST = (SELECT MIN(PS_SUPPLYCOST) FROM PARTSUPP WHERE PS_PARTKEY = P.PS_PARTKEY)",
        4 * PARTS,
    ),
    (
        "Q4 EXISTS of the order's late lines",
        "SELECT COUNT(*) FROM ORDERS WHERE EXISTS (SELECT * FROM LINEITEM WHERE L_ORDERKEY = O_ORDERKEY AND L_COMMITDATE < L_RECEIPTDATE)",
        ORDERS,
    ),
    (
        "Q17 below the part's mean quantity",
        "SELECT COUNT(*) FROM LINEITEM L WHERE L_QUANTITY < (SELECT 0.5 * AVG(L_QUANTITY) FROM LINEITEM WHERE L_PARTKEY = L.L_PARTKEY)",
        LINES,
    ),
    (
        "Q21 EXISTS and NOT EXISTS by <>",
        "SELECT COUNT(*) FROM LINEITEM L1 WHERE EXISTS (SELECT * FROM LINEITEM L2 WHERE L2.L_ORDERKEY = L1.L_ORDERKEY AND L2.L_SUPPKEY <> L1.L_SUPPKEY)
         AND NOT EXISTS (SELECT * FROM LINEITEM L3 WHERE L3.L_ORDERKEY = L1.L_ORDERKEY AND L3.L_SUPPKEY <> L1.L_SUPPKEY AND L3.L_RECEIPTDATE > L3.L_COMMITDATE)",
        LINES,
    ),
    ("Q22 NOT EXISTS of the customer's orders", "SELECT COUNT(*) FROM CUSTOMER WHERE NOT EXISTS (SELECT * FROM ORDERS WHERE O_CUSTKEY = C_CUSTKEY)", CUSTOMERS),
    ("scalar COUNT of a select list", "SELECT O_ORDERKEY, (SELECT COUNT(*) FROM LINEITEM WHERE L_ORDERKEY = O_ORDERKEY) FROM ORDERS", ORDERS),
    (
        "NOT IN by the order's own lines",
        "SELECT COUNT(*) FROM ORDERS WHERE O_ORDERDATE NOT IN (SELECT L_COMMITDATE FROM LINEITEM WHERE L_ORDERKEY = O_ORDERKEY)",
        ORDERS,
    ),
    (
        "join beside a correlated AVG, Q17's shape",
        "SELECT COUNT(*) FROM CUSTOMER, ORDERS WHERE C_CUSTKEY = O_CUSTKEY AND C_ACCTBAL > 0
         AND O_ORDERDATE < (SELECT AVG(O2.O_ORDERDATE) FROM ORDERS O2 WHERE O2.O_CUSTKEY = C_CUSTKEY)",
        ORDERS,
    ),
    ("SQL function of a correlated COUNT", "SELECT C_CUSTKEY, ORDERED(C_CUSTKEY) FROM CUSTOMER", CUSTOMERS),
    (
        "EXISTS of the order's late lines, under OR",
        "SELECT COUNT(*) FROM ORDERS WHERE EXISTS (SELECT * FROM LINEITEM WHERE L_ORDERKEY = O_ORDERKEY AND L_COMMITDATE < L_RECEIPTDATE)
         OR O_ORDERDATE < 30",
        ORDERS,
    ),
    (
        "IN of the order's commit dates, in a select list",
        "SELECT O_ORDERKEY, (O_ORDERDATE IN (SELECT L_COMMITDATE FROM LINEITEM WHERE L_ORDERKEY = O_ORDERKEY)) FROM ORDERS",
        ORDERS,
    ),
    (
        "ALL of the part's costs, in a select list",
        "SELECT PS_PARTKEY, (PS_SUPPLYCOST <= ALL (SELECT PS_SUPPLYCOST FROM PARTSUPP P2 WHERE P2.PS_PARTKEY = P.PS_PARTKEY)) FROM PARTSUPP P",
        4 * PARTS,
    ),
];

/// Switches unnesting and inlining `on` or off.
fn switched(db: &Database, on: bool) {
    let value = Value::Char(if on { "ON" } else { "OFF" }.to_owned());
    for rule in [SEMI_JOINS, MARK_JOINS, UNNEST_SUBQUERIES, INLINE_FUNCTIONS] {
        db.set_setting(&switch_name(rule), &value).unwrap();
    }
}

/// Reads every row of `sql`.
fn fetched(s: &Session, sql: &str) {
    let mut cursor = s.open_cursor(sql).unwrap();
    while cursor.fetch().unwrap().is_some() {}
}

/// Measures each query unnested, then nested.
pub fn workloads(bench: &Bench) {
    let db = database();
    let s = db.session("B");
    for (on, label) in [(true, "unnested"), (false, "nested")] {
        switched(&db, on);
        for (name, sql, rows) in QUERIES {
            bench.measure(Case::new(format!("{name} ({label})"), "rows"), || {
                fetched(&s, sql);
                *rows
            });
        }
    }
    switched(&db, true);
}
