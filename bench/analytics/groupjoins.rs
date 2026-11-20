//! How fast groupings of joins on a join's key run (RULES.md D490) and
//! index joins of a unique index (RULES.md D491): over TPC-H-shaped tables
//! of 15,000 customers, 150,000 orders and 600,000 line items, the
//! shapes of queries 3, 13, 17 and 18, and an index join of a few thousand
//! scattered order keys, each on the workers and on one thread. Run alone
//! with `ONLY_VAR` set to `groupjoins` (RULES.md R-BENCH-1).

use super::harness::{Bench, Case, scatter};
use turborel::db::{Database, Session};

/// Customers.
const CUSTOMERS: u64 = 15_000;
/// Orders: ten for each customer, a third of whom order nothing.
const ORDERS: u64 = 10 * CUSTOMERS;
/// Line items: four for each order.
const LINES: u64 = 4 * ORDERS;
/// Parts.
const PARTS: u64 = 20_000;
/// Order keys picked, some twice, joined by the orders' index.
const PICKED: u64 = 20_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 1_000;

/// The tables, filled.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
         CREATE TABLE CUSTOMER (C_CUSTKEY INTEGER NOT NULL PRIMARY KEY, C_NAME VARCHAR(25))
         CREATE TABLE PART (P_PARTKEY INTEGER NOT NULL PRIMARY KEY, P_BRAND INTEGER)
         CREATE TABLE ORDERS (O_ORDERKEY INTEGER NOT NULL PRIMARY KEY, O_CUSTKEY INTEGER NOT NULL,
             O_TOTALPRICE DECIMAL(15,2) NOT NULL, O_ORDERDATE INTEGER, O_COMMENT VARCHAR(40))
         CREATE TABLE LINEITEM (L_ORDERKEY INTEGER NOT NULL, L_PARTKEY INTEGER NOT NULL, L_QUANTITY DECIMAL(15,2) NOT NULL,
             L_EXTENDEDPRICE DECIMAL(15,2) NOT NULL, L_SHIPDATE INTEGER)
         CREATE TABLE PICK (K INTEGER, V INTEGER)",
    )
    .unwrap();
    let s = db.session("B");
    let fill = |table: &str, count: u64, row: &dyn Fn(u64) -> String| {
        for start in (0..count).step_by(PER_INSERT as usize) {
            let rows: Vec<String> = (start..count.min(start + PER_INSERT)).map(row).collect();
            s.run(&format!("INSERT INTO {table} VALUES {}", rows.join(", "))).unwrap();
        }
    };
    fill("CUSTOMER", CUSTOMERS, &|n| format!("({n}, 'Customer#{n}')"));
    fill("PART", PARTS, &|n| format!("({n}, {})", n % 25));
    fill("ORDERS", ORDERS, &|n| {
        let note = if n % 9 == 0 { "special packages requests" } else { "regular" };
        format!("({n}, {}, {}.{:02}, {}, '{note}')", scatter(n, CUSTOMERS * 2 / 3) * 3 / 2, scatter(n, 500_000), n % 100, scatter(n, 2_400))
    });
    fill("LINEITEM", LINES, &|n| {
        format!("({}, {}, {}, {}.{:02}, {})", n / 4, scatter(n, PARTS), 1 + n % 50, scatter(n, 90_000), n % 100, scatter(n, 2_400))
    });
    fill("PICK", PICKED, &|n| format!("({}, {})", scatter(n, ORDERS), n % 10));
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// The queries measured: each case's name, its text and the rows it reads.
const QUERIES: [(&str, &str, u64); 5] = [
    (
        "Q13 shape, customers counted by their orders, a LEFT join grouped",
        "SELECT N, COUNT(*) FROM (SELECT C_CUSTKEY, COUNT(O_ORDERKEY) FROM CUSTOMER LEFT JOIN ORDERS
           ON C_CUSTKEY = O_CUSTKEY AND O_COMMENT NOT LIKE '%special%requests%' GROUP BY C_CUSTKEY) AS C (K, N) GROUP BY N",
        CUSTOMERS + ORDERS,
    ),
    (
        "Q18 shape, line items summed by order, grouped by the order's columns",
        "SELECT O_ORDERKEY, O_TOTALPRICE, SUM(L_QUANTITY) FROM ORDERS JOIN LINEITEM ON O_ORDERKEY = L_ORDERKEY
           GROUP BY O_ORDERKEY, O_TOTALPRICE HAVING SUM(L_QUANTITY) > 150",
        ORDERS + LINES,
    ),
    (
        "Q3 shape, revenue of filtered orders by the line item's key",
        "SELECT L_ORDERKEY, O_ORDERDATE, SUM(L_EXTENDEDPRICE) FROM LINEITEM JOIN ORDERS ON L_ORDERKEY = O_ORDERKEY
           WHERE O_ORDERDATE < 1200 AND L_SHIPDATE > 1200 GROUP BY L_ORDERKEY, O_ORDERDATE",
        ORDERS + LINES,
    ),
    (
        "Q17 shape, a correlated average of each part's line items",
        "SELECT SUM(L_EXTENDEDPRICE) FROM LINEITEM, PART WHERE P_PARTKEY = L_PARTKEY AND P_BRAND = 3
           AND L_QUANTITY < (SELECT 0.2 * AVG(L_QUANTITY) FROM LINEITEM WHERE L_PARTKEY = P_PARTKEY)",
        PARTS + 2 * LINES,
    ),
    (
        "an index join of 2,000 scattered order keys",
        "SELECT COUNT(*), SUM(O_TOTALPRICE) FROM PICK JOIN ORDERS ON PICK.K = O_ORDERKEY WHERE PICK.V = 3",
        PICKED,
    ),
];

/// Measures each of `QUERIES` on the workers and on one thread.
pub fn workloads(bench: &Bench) {
    let db = database();
    let s = db.session("B");
    for (degree, label) in [("0", ""), ("1", " (one thread)")] {
        s.run(&format!("SET EXECUTOR.DEGREE = {degree}")).unwrap();
        for (name, sql, read) in QUERIES {
            bench.measure(Case::new(format!("{name}{label}"), "rows"), || {
                fetched(&s, sql);
                read
            });
        }
    }
}

/// The rows of `sql`, counted.
fn fetched(s: &Session, sql: &str) -> u64 {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut rows = 0;
    while cursor.fetch().unwrap().is_some() {
        rows += 1;
    }
    s.commit().unwrap();
    rows
}
