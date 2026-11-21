//! How fast groupings over joins run (RULES.md D660): of tables shaped as
//! TPC-H's, dates as day numbers, of 600,000 line items of 150,000 orders
//! of 15,000 customers, and 1,000 suppliers of 25 nations of 5 regions:
//! queries shaped as TPC-H's Q3, Q5, Q10 and Q18, the line items summed
//! by the nation of their supplier, and the suppliers' balances summed by
//! nation over the line items, as eager aggregation plans them and with
//! OPTIMIZER.EAGER_AGGREGATION OFF. Run with `cargo xtask bench`, on the
//! harness (RULES.md R-BENCH-1); with `ONLY_VAR` of `analytics.rs` set to
//! `eager`, these alone.

use super::harness::{Bench, Case, scatter};
use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;

/// Orders; each has `PER_ORDER` line items.
const ORDERS: u64 = 150_000;
/// Line items of each order.
const PER_ORDER: u64 = 4;
/// Customers.
const CUSTOMERS: u64 = 15_000;
/// Suppliers.
const SUPPLIERS: u64 = 1_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 1_000;

/// The queries, each with the name it is reported by.
const QUERIES: [(&str, &str); 7] = [
    (
        "Q3-shaped, revenue of unshipped orders",
        "SELECT L_ORDERKEY, SUM(L_EXTENDEDPRICE * (1 - L_DISCOUNT)) AS REVENUE, O_ORDERDATE, O_SHIPPRIORITY
           FROM CUSTOMER, ORDERS, LINEITEM
          WHERE C_MKTSEGMENT = 'BUILDING' AND C_CUSTKEY = O_CUSTKEY AND L_ORDERKEY = O_ORDERKEY
            AND O_ORDERDATE < 1200 AND L_SHIPDATE > 1200
          GROUP BY L_ORDERKEY, O_ORDERDATE, O_SHIPPRIORITY ORDER BY REVENUE DESC, O_ORDERDATE FETCH FIRST 10 ROWS ONLY",
    ),
    (
        "Q5-shaped, revenue by nation of a region",
        "SELECT N_NAME, SUM(L_EXTENDEDPRICE * (1 - L_DISCOUNT)) AS REVENUE
           FROM CUSTOMER, ORDERS, LINEITEM, SUPPLIER, NATION, REGION
          WHERE C_CUSTKEY = O_CUSTKEY AND L_ORDERKEY = O_ORDERKEY AND L_SUPPKEY = S_SUPPKEY
            AND C_NATIONKEY = S_NATIONKEY AND S_NATIONKEY = N_NATIONKEY AND N_REGIONKEY = R_REGIONKEY
            AND R_NAME = 'ASIA' AND O_ORDERDATE >= 400 AND O_ORDERDATE < 765
          GROUP BY N_NAME ORDER BY REVENUE DESC",
    ),
    (
        "Q10-shaped, returned revenue by customer",
        "SELECT C_CUSTKEY, C_NAME, SUM(L_EXTENDEDPRICE * (1 - L_DISCOUNT)) AS REVENUE, C_ACCTBAL, N_NAME
           FROM CUSTOMER, ORDERS, LINEITEM, NATION
          WHERE C_CUSTKEY = O_CUSTKEY AND L_ORDERKEY = O_ORDERKEY AND O_ORDERDATE >= 900 AND O_ORDERDATE < 990
            AND L_RETURNFLAG = 'R' AND C_NATIONKEY = N_NATIONKEY
          GROUP BY C_CUSTKEY, C_NAME, C_ACCTBAL, N_NAME ORDER BY REVENUE DESC FETCH FIRST 20 ROWS ONLY",
    ),
    (
        "Q18-shaped, quantity of each order",
        "SELECT C_NAME, C_CUSTKEY, O_ORDERKEY, O_ORDERDATE, O_TOTALPRICE, SUM(L_QUANTITY)
           FROM CUSTOMER, ORDERS, LINEITEM
          WHERE C_CUSTKEY = O_CUSTKEY AND O_ORDERKEY = L_ORDERKEY
          GROUP BY C_NAME, C_CUSTKEY, O_ORDERKEY, O_ORDERDATE, O_TOTALPRICE
          ORDER BY O_TOTALPRICE DESC, O_ORDERDATE FETCH FIRST 100 ROWS ONLY",
    ),
    (
        "line items by the nation of their supplier",
        "SELECT N_NAME, SUM(L_QUANTITY), COUNT(*), MAX(L_EXTENDEDPRICE)
           FROM LINEITEM, SUPPLIER, NATION WHERE L_SUPPKEY = S_SUPPKEY AND S_NATIONKEY = N_NATIONKEY GROUP BY N_NAME",
    ),
    (
        "suppliers' balances by nation over the line items",
        "SELECT S_NATIONKEY, SUM(S_ACCTBAL), COUNT(*) FROM LINEITEM JOIN SUPPLIER ON L_SUPPKEY = S_SUPPKEY GROUP BY S_NATIONKEY",
    ),
    (
        "line items to few groups before a selective join",
        "SELECT N_NAME, SUM(L_QUANTITY), COUNT(*) FROM LINEITEM, SUPPLIER, NATION
          WHERE L_SUPPKEY = S_SUPPKEY AND S_NATIONKEY = N_NATIONKEY AND S_ACCTBAL > 8000 GROUP BY N_NAME",
    ),
];

/// The tables of schema H, filled.
fn database() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION H
           CREATE TABLE REGION (R_REGIONKEY INTEGER NOT NULL, R_NAME CHAR(25) NOT NULL)
           CREATE TABLE NATION (N_NATIONKEY INTEGER NOT NULL, N_NAME CHAR(25) NOT NULL, N_REGIONKEY INTEGER NOT NULL)
           CREATE TABLE SUPPLIER (S_SUPPKEY INTEGER NOT NULL, S_NATIONKEY INTEGER NOT NULL, S_ACCTBAL DECIMAL(15, 2) NOT NULL)
           CREATE TABLE CUSTOMER (C_CUSTKEY INTEGER NOT NULL, C_NAME VARCHAR(25) NOT NULL, C_NATIONKEY INTEGER NOT NULL,
             C_ACCTBAL DECIMAL(15, 2) NOT NULL, C_MKTSEGMENT CHAR(10) NOT NULL)
           CREATE TABLE ORDERS (O_ORDERKEY INTEGER NOT NULL, O_CUSTKEY INTEGER NOT NULL, O_TOTALPRICE DECIMAL(15, 2) NOT NULL,
             O_ORDERDATE INTEGER NOT NULL, O_SHIPPRIORITY INTEGER NOT NULL)
           CREATE TABLE LINEITEM (L_ORDERKEY INTEGER NOT NULL, L_SUPPKEY INTEGER NOT NULL, L_QUANTITY DECIMAL(15, 2) NOT NULL,
             L_EXTENDEDPRICE DECIMAL(15, 2) NOT NULL, L_DISCOUNT DECIMAL(15, 2) NOT NULL, L_RETURNFLAG CHAR(1) NOT NULL,
             L_SHIPDATE INTEGER NOT NULL)",
    )
    .unwrap();
    let s = db.session("H");
    let regions = ["AFRICA", "AMERICA", "ASIA", "EUROPE", "MIDDLE EAST"];
    insert(&s, "REGION", regions.iter().enumerate().map(|(r, name)| format!("({r}, '{name}')")));
    insert(&s, "NATION", (0..25_u64).map(|n| format!("({n}, 'NATION{n}', {})", n % 5)));
    insert(&s, "SUPPLIER", (0..SUPPLIERS).map(|k| format!("({k}, {}, {}.{:02})", scatter(k, 25), scatter(k, 9_000), k % 100)));
    let segments = ["AUTOMOBILE", "BUILDING", "FURNITURE", "HOUSEHOLD", "MACHINERY"];
    insert(
        &s,
        "CUSTOMER",
        (0..CUSTOMERS).map(|k| {
            format!("({k}, 'Customer#{k}', {}, {}.00, '{}')", k % 25, scatter(k, 9_000), segments[usize::try_from(k % 5).unwrap()])
        }),
    );
    insert(
        &s,
        "ORDERS",
        (0..ORDERS).map(|k| format!("({k}, {}, {}.00, {}, {})", scatter(k, CUSTOMERS), scatter(k, 400_000), scatter(k, 2_400), k % 3)),
    );
    let item = |n: u64| {
        let order = n / PER_ORDER;
        let flag = ["A", "N", "R"][usize::try_from(n % 3).unwrap()];
        format!(
            "({order}, {}, {}.00, {}.00, 0.0{}, '{flag}', {})",
            scatter(n, SUPPLIERS),
            1 + n % 50,
            scatter(n, 100_000),
            n % 10,
            scatter(n, 2_500)
        )
    };
    insert(&s, "LINEITEM", (0..ORDERS * PER_ORDER).map(item));
    db.maintain().unwrap();
    (db, s)
}

/// Inserts `rows` into `table`, `PER_INSERT` at a time.
fn insert(s: &Session, table: &str, rows: impl Iterator<Item = String>) {
    let rows: Vec<String> = rows.collect();
    for chunk in rows.chunks(PER_INSERT as usize) {
        s.run(&format!("INSERT INTO {table} VALUES {}", chunk.join(", "))).unwrap();
    }
    s.commit().unwrap();
}

/// Rows `sql` gives, each fetched.
fn fetched(s: &Session, sql: &str) -> u64 {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut rows = 0;
    while cursor.fetch().unwrap().is_some() {
        rows += 1;
    }
    s.commit().unwrap();
    rows
}

/// Each query at one thread, as planned, as written and grouped first.
pub fn workloads(bench: &Bench) {
    let (_db, s) = database();
    s.run("SET EXECUTOR.DEGREE = 1").unwrap();
    // As planned, planned as written, and grouped first wherever it is
    // estimated within tenfold of the plan as written, so the costs' choice
    // can be judged against both. A plan estimated far worse is left out of
    // the forced runs, as it would not finish. A build without the settings
    // runs them as planned.
    for (switch, gain, label) in [("ON", "TO DEFAULT", ""), ("OFF", "TO DEFAULT", " (eager off)"), ("ON", "= 10", " (eager forced)")] {
        let _ = s.run(&format!("SET OPTIMIZER.EAGER_AGGREGATION = '{switch}'"));
        let _ = s.run(&format!("SET PLANNER.EAGER_GAIN {gain}"));
        for (name, sql) in QUERIES {
            bench.measure(Case::new(format!("{name}{label}"), "rows"), || {
                fetched(&s, sql);
                ORDERS * PER_ORDER
            });
        }
    }
}
