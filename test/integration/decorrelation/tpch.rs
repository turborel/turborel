//! The correlated subqueries of the TPC-H queries 2, 4, 17, 20, 21 and 22,
//! over small tables of their columns, dates as day numbers.

use turborel::db::Database;

use super::{both, unnested};
use crate::common::Rng;

/// Schema H: the TPC-H tables, of the columns the queries read.
const TABLES: &str = "CREATE SCHEMA AUTHORIZATION H
    CREATE TABLE REGION (R_REGIONKEY INTEGER, R_NAME CHAR(10))
    CREATE TABLE NATION (N_NATIONKEY INTEGER, N_NAME CHAR(10), N_REGIONKEY INTEGER)
    CREATE TABLE SUPPLIER (S_SUPPKEY INTEGER, S_NAME CHAR(10), S_NATIONKEY INTEGER, S_ACCTBAL DECIMAL(12, 2))
    CREATE TABLE PART (P_PARTKEY INTEGER, P_NAME VARCHAR(20), P_BRAND CHAR(5), P_SIZE INTEGER)
    CREATE TABLE PARTSUPP (PS_PARTKEY INTEGER, PS_SUPPKEY INTEGER, PS_AVAILQTY INTEGER, PS_SUPPLYCOST DECIMAL(12, 2))
    CREATE TABLE CUSTOMER (C_CUSTKEY INTEGER, C_PHONE CHAR(15), C_ACCTBAL DECIMAL(12, 2))
    CREATE TABLE ORDERS (O_ORDERKEY INTEGER, O_CUSTKEY INTEGER, O_ORDERSTATUS CHAR(1), O_ORDERDATE INTEGER, O_ORDERPRIORITY CHAR(8))
    CREATE TABLE LINEITEM (L_ORDERKEY INTEGER, L_PARTKEY INTEGER, L_SUPPKEY INTEGER, L_QUANTITY DECIMAL(12, 2),
        L_EXTENDEDPRICE DECIMAL(12, 2), L_SHIPDATE INTEGER, L_COMMITDATE INTEGER, L_RECEIPTDATE INTEGER)";

/// The tables, filled from `seed`.
fn tpch(seed: u64) -> Database {
    let db = Database::new();
    db.schema(TABLES).unwrap();
    let s = db.session("H");
    let mut rng = Rng::new(seed);
    let insert = |table: &str, rows: Vec<String>| {
        for row in rows {
            s.run(&format!("INSERT INTO {table} VALUES ({row})")).unwrap();
        }
    };
    insert("REGION", ["'EUROPE'", "'ASIA'"].iter().enumerate().map(|(k, n)| format!("{k}, {n}")).collect());
    insert("NATION", (0..4).map(|k| format!("{k}, '{}', {}", ["CANADA", "FRANCE", "SAUDI", "JAPAN"][k], k % 2)).collect());
    let mut rows = |n: usize, f: &mut dyn FnMut(&mut Rng, usize) -> String| (0..n).map(|k| f(&mut rng, k)).collect::<Vec<_>>();
    let suppliers = rows(8, &mut |r, k| format!("{k}, 'S{k}', {}, {}.50", r.below(4), r.below(900)));
    let parts = rows(12, &mut |r, k| format!("{k}, '{}{k}', 'B{}', {}", ["forest", "lemon"][r.below(2)], r.below(3), 10 + r.below(10)));
    let partsupp = rows(30, &mut |r, _| format!("{}, {}, {}, {}.25", r.below(12), r.below(8), r.below(60), r.below(20)));
    let customers = rows(10, &mut |r, k| format!("{k}, '{}-555', {}.00", 13 + r.below(20), r.below(900) as i64 - 100));
    let orders = rows(25, &mut |r, k| format!("{k}, {}, '{}', {}, '{}-URG'", r.below(10), ["F", "O"][r.below(2)], r.below(200), r.below(5)));
    let lineitems = rows(80, &mut |r, _| {
        let (ship, commit) = (r.below(200), r.below(200));
        format!("{}, {}, {}, {}.00, {}.00, {ship}, {commit}, {}", r.below(25), r.below(12), r.below(8), 1 + r.below(30), r.below(500), ship + r.below(20))
    });
    insert("SUPPLIER", suppliers);
    insert("PART", parts);
    insert("PARTSUPP", partsupp);
    insert("CUSTOMER", customers);
    insert("ORDERS", orders);
    insert("LINEITEM", lineitems);
    s.commit().unwrap();
    db
}

/// The queries, their correlated subqueries as TPC-H writes them.
const QUERIES: &[(&str, &str)] = &[
    (
        "Q2",
        "SELECT S_ACCTBAL, S_NAME, N_NAME, P_PARTKEY FROM PART, SUPPLIER, PARTSUPP, NATION, REGION
         WHERE P_PARTKEY = PS_PARTKEY AND S_SUPPKEY = PS_SUPPKEY AND P_SIZE > 12 AND S_NATIONKEY = N_NATIONKEY
         AND N_REGIONKEY = R_REGIONKEY AND R_NAME = 'EUROPE'
         AND PS_SUPPLYCOST = (SELECT MIN(PS_SUPPLYCOST) FROM PARTSUPP, SUPPLIER, NATION, REGION
             WHERE P_PARTKEY = PS_PARTKEY AND S_SUPPKEY = PS_SUPPKEY AND S_NATIONKEY = N_NATIONKEY
             AND N_REGIONKEY = R_REGIONKEY AND R_NAME = 'EUROPE')",
    ),
    (
        "Q4",
        "SELECT O_ORDERPRIORITY, COUNT(*) FROM ORDERS WHERE O_ORDERDATE >= 20 AND O_ORDERDATE < 150
         AND EXISTS (SELECT * FROM LINEITEM WHERE L_ORDERKEY = O_ORDERKEY AND L_COMMITDATE < L_RECEIPTDATE)
         GROUP BY O_ORDERPRIORITY",
    ),
    (
        "Q17",
        "SELECT SUM(L_EXTENDEDPRICE) / 7.0 FROM LINEITEM, PART WHERE P_PARTKEY = L_PARTKEY AND P_BRAND = 'B1'
         AND L_QUANTITY < (SELECT 0.5 * AVG(L_QUANTITY) FROM LINEITEM WHERE L_PARTKEY = P_PARTKEY)",
    ),
    (
        "Q20",
        "SELECT S_NAME FROM SUPPLIER, NATION WHERE S_SUPPKEY IN (SELECT PS_SUPPKEY FROM PARTSUPP
             WHERE PS_PARTKEY IN (SELECT P_PARTKEY FROM PART WHERE P_NAME LIKE 'forest%')
             AND PS_AVAILQTY > (SELECT 0.5 * SUM(L_QUANTITY) FROM LINEITEM WHERE L_PARTKEY = PS_PARTKEY
                 AND L_SUPPKEY = PS_SUPPKEY AND L_SHIPDATE >= 10 AND L_SHIPDATE < 150))
         AND S_NATIONKEY = N_NATIONKEY AND N_NAME = 'CANADA'",
    ),
    (
        "Q21",
        "SELECT S_NAME, COUNT(*) FROM SUPPLIER, LINEITEM L1, ORDERS, NATION
         WHERE S_SUPPKEY = L1.L_SUPPKEY AND O_ORDERKEY = L1.L_ORDERKEY AND O_ORDERSTATUS = 'F'
         AND L1.L_RECEIPTDATE > L1.L_COMMITDATE
         AND EXISTS (SELECT * FROM LINEITEM L2 WHERE L2.L_ORDERKEY = L1.L_ORDERKEY AND L2.L_SUPPKEY <> L1.L_SUPPKEY)
         AND NOT EXISTS (SELECT * FROM LINEITEM L3 WHERE L3.L_ORDERKEY = L1.L_ORDERKEY AND L3.L_SUPPKEY <> L1.L_SUPPKEY
             AND L3.L_RECEIPTDATE > L3.L_COMMITDATE)
         AND S_NATIONKEY = N_NATIONKEY AND N_NAME = 'SAUDI'
         GROUP BY S_NAME",
    ),
    (
        "Q22",
        "SELECT CNTRYCODE, COUNT(*), SUM(C_ACCTBAL) FROM (SELECT SUBSTRING(C_PHONE FROM 1 FOR 2) AS CNTRYCODE, C_ACCTBAL
             FROM CUSTOMER WHERE SUBSTRING(C_PHONE FROM 1 FOR 2) IN ('13', '17', '18', '23', '29', '30', '31')
             AND C_ACCTBAL > (SELECT AVG(C_ACCTBAL) FROM CUSTOMER WHERE C_ACCTBAL > 0.00
                 AND SUBSTRING(C_PHONE FROM 1 FOR 2) IN ('13', '17', '18', '23', '29', '30', '31'))
             AND NOT EXISTS (SELECT * FROM ORDERS WHERE O_CUSTKEY = C_CUSTKEY)) AS CUSTSALE
         GROUP BY CNTRYCODE",
    ),
];

/// Verifies: [X3.135-1986 §5.11 GR1, §5.13 GR2, §5.17 GR2, §5.21 GR3]
/// (RULES.md D312 to D314) the TPC-H queries' correlated subqueries are
/// unnested, each query's rows the same as when they run for each
/// binding: Q2's and Q17's scalar set functions, Q4's EXISTS, Q20's IN of
/// a subquery whose own subquery is correlated, Q21's EXISTS and NOT
/// EXISTS correlated by an inequality, and Q22's NOT EXISTS beside an
/// uncorrelated scalar subquery.
#[test]
fn tpch_subqueries_are_unnested() {
    for seed in [0x5EED_7C40_0000_0001, 0x5EED_7C40_0000_0002] {
        let db = tpch(seed);
        let s = db.session("H");
        for (name, q) in QUERIES {
            assert!(unnested(&s, q), "{name}: {}", s.explain(q, &turborel::params::Params::new()).unwrap());
            let (on, off) = both(&db, &s, q);
            assert!(on.is_ok(), "{name}: {on:?}");
            assert_eq!(on, off, "{name}");
        }
    }
}
