//! SUM and AVG judged as totals [ISO/IEC 9075:1992 §6.5 GR2b ii, iv]:
//! whether one raises does not depend on the order its rows are read in,
//! which is the plan's (RULES.md D15): the store, the batch or the row, an
//! index, the order written, a grouping spilled (RULES.md D38).

use std::fmt::Write as _;

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;

use crate::common::{Counted, both_ways, rows};
use crate::spilling::{SMALL, bytes};

/// 10^18 - 1, the most NUMERIC(18) holds.
const MOST: &str = "999999999999999999";
/// Three values whose sum is `MOST`, and three approximate ones whose sum
/// is the most a double holds, though the first two overflow either: each
/// order of the three is a row of `ORDERS`.
const ORDERS: [[usize; 3]; 6] = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];

/// The row `k` of a table of the values at `order`: N exact, F
/// approximate, G its group.
fn row(k: usize, at: usize) -> String {
    let n = [MOST, "1", "-1"][at];
    let f = ["1.7976931348623157E308", "1.7976931348623157E308", "-1.7976931348623157E308"][at];
    format!("({k}, {n}, {f}, 0)")
}

/// Tables R0-R5 of the row store and C0-C5 of the column store, rows
/// frozen, each the three values in one of `ORDERS`, an index on N and
/// on F; and T, of 10^18 - 1 and 1, whose sum does not fit.
fn tables(db: &Database) -> Session {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let mut schema = String::from("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N NUMERIC(18), F DOUBLE PRECISION, G INTEGER)");
    for n in 0..ORDERS.len() {
        let columns = "(K INTEGER, N NUMERIC(18), F DOUBLE PRECISION, G INTEGER)";
        write!(schema, " CREATE TABLE R{n} {columns} CREATE TABLE C{n} {columns} USING columnstore").unwrap();
    }
    db.schema(&schema).unwrap();
    let a = db.session("A");
    for (n, order) in ORDERS.iter().enumerate() {
        let values: Vec<String> = order.iter().enumerate().map(|(k, &at)| row(k, at)).collect();
        for table in [format!("R{n}"), format!("C{n}")] {
            db.create_index(&format!("{table}_N"), &format!("A.{table}"), &["N"], "btree").unwrap();
            db.create_index(&format!("{table}_F"), &format!("A.{table}"), &["F"], "btree").unwrap();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
            a.commit().unwrap();
        }
    }
    a.run(&format!("INSERT INTO T VALUES (0, {MOST}, 1E308, 0), (1, 1, 1E308, 0)")).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    a
}

/// The SQLSTATE and SQLCODE of `sql`, which must fail as its rows are read.
fn refused(s: &Session, sql: &str) -> (String, i32) {
    let read = s.open_cursor(sql).and_then(|mut c| {
        while c.fetch()?.is_some() {}
        Ok(())
    });
    let failed = read.expect_err(sql);
    (failed.sqlstate().as_str().to_owned(), failed.sqlcode())
}

/// Verifies: [DESIGN] RULES.md D15 [ISO/IEC 9075:1992 §6.5 GR2b ii, iv]:
/// SUM of 10^18 - 1, 1 and -1 is 10^18 - 1, and SUM of the most double,
/// twice, and its negative is that double, whatever order the rows were
/// written in, of the row store or the column store, read by batches or
/// row by row, by an index or a scan, grouped or not; AVG is their mean.
/// A sum that does not fit is 22003, SQLCODE -301, alike. Regression test
/// (R-TEST-7): a running sum was judged, so the orders whose first two
/// values were 10^18 - 1 and 1, or the two doubles, raised 22003.
#[test]
fn a_sum_is_judged_whatever_the_order_of_its_rows() {
    let db = Database::new();
    let a = tables(&db);
    let most_double = "1.7976931348623157E308";
    for n in 0..ORDERS.len() {
        for table in [format!("R{n}"), format!("C{n}")] {
            let queries = [
                (format!("SELECT SUM(N), SUM(F) FROM {table}"), format!("{MOST},{most_double}")),
                (format!("SELECT SUM(N) FROM {table} WHERE N >= -1"), MOST.to_owned()),
                (format!("SELECT SUM(F) FROM {table} WHERE F >= -1.7976931348623157E308"), most_double.to_owned()),
                (format!("SELECT G, SUM(N + 0), SUM(F) FROM {table} GROUP BY G"), format!("0,{MOST},{most_double}")),
                (format!("SELECT AVG(F) FROM {table} WHERE N <> 0"), format!("{:E}", f64::MAX / 3.0_f64)),
            ];
            for (query, expected) in queries {
                let (batched, by_rows) = both_ways(&a, &query);
                assert_eq!((batched, by_rows), (vec![expected.clone()], vec![expected]), "{query}");
            }
        }
    }
    for query in ["SELECT SUM(N) FROM T", "SELECT SUM(F) FROM T", "SELECT G, SUM(N) FROM T GROUP BY G"] {
        assert_eq!(refused(&a, query), ("22003".to_owned(), -301), "{query}");
    }
}

/// Verifies: [DESIGN] RULES.md D15, D38 [ISO/IEC 9075:1992 §6.5 GR2b iv]:
/// a grouping that spills, its groups' rows partitioned and read again
/// in passes, sums each group's 10^18 - 1, 1 and -1, in an order of its
/// own, to 10^18 - 1, as in memory, by batches and row by row.
/// Regression test (R-TEST-7): the groups whose rows came 10^18 - 1 then
/// 1 raised 22003.
#[test]
fn a_spilled_grouping_judges_each_total() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER, G INTEGER, N NUMERIC(18), S CHARACTER(60))").unwrap();
    let s = db.session("S");
    for pass in 0..3_usize {
        let values: Vec<String> = (0..1000_usize)
            .map(|g| {
                let k = pass * 1000 + g;
                let n = [MOST, "1", "-1"][(pass + g) % 3];
                format!("({k}, {g}, {n}, 'S{k:05}')")
            })
            .collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    let query = "SELECT G, SUM(N), MIN(S) FROM T GROUP BY G";
    let in_memory = rows(&s, query).unwrap();
    assert_eq!(in_memory.len(), 1000_usize);
    assert!(in_memory.iter().all(|r| r.split(',').nth(1) == Some(MOST)), "every group's total");
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    db.set_setting("MEMORY.WORK_BYTES", &bytes(SMALL)).unwrap();
    let (batched, by_rows) = both_ways(&s, query);
    let mut sorted = in_memory;
    sorted.sort();
    assert_eq!((batched, by_rows), (sorted.clone(), sorted), "spilled");
    assert!(counted.taken() > 0, "the grouping spilled");
}

/// Verifies: [DESIGN] RULES.md D15 [ISO/IEC 9075:1992 §6.5 GR2b iv, SR9e]:
/// SUM of an INTERVAL DAY(2) column is of its fields: 99, 1 and -1 days
/// sum to 99 in every order written, by batches and row by row; a total
/// of 100 days is out of the range of the result, "data exception-numeric
/// value out of range", 22003 and SQLCODE -301, as for any SUM, not the
/// 22015 of interval arithmetic.
#[test]
fn an_interval_sum_out_of_range_is_22003() {
    let db = Database::new();
    let mut schema = String::from("CREATE SCHEMA AUTHORIZATION A CREATE TABLE P (D INTERVAL DAY(2))");
    for n in 0..ORDERS.len() {
        write!(schema, " CREATE TABLE I{n} (D INTERVAL DAY(2))").unwrap();
    }
    db.schema(&schema).unwrap();
    let a = db.session("A");
    for (n, order) in ORDERS.iter().enumerate() {
        let values: Vec<String> = order.iter().map(|&at| format!("(INTERVAL '{}' DAY)", ["99", "1", "-1"][at])).collect();
        a.run(&format!("INSERT INTO I{n} VALUES {}", values.join(", "))).unwrap();
        let (batched, by_rows) = both_ways(&a, &format!("SELECT SUM(D) FROM I{n}"));
        assert_eq!((batched, by_rows), (vec!["99".to_owned()], vec!["99".to_owned()]), "order {order:?}");
    }
    a.run("INSERT INTO P VALUES (INTERVAL '99' DAY), (INTERVAL '1' DAY)").unwrap();
    assert_eq!(refused(&a, "SELECT SUM(D) FROM P"), ("22003".to_owned(), -301));
}
